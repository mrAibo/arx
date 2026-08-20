//! Background transfer queue runtime built on the existing JobManager and
//! transfer executor.
//!
//! The runtime deliberately keeps retry fail-closed for now: current executor
//! errors do not carry enough mutation-certainty information to safely replay
//! remote writes. Provider-specific retry classification is a later PACK G
//! integration step. Until then, every execution failure is one-shot.

use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tokio::sync::mpsc;

use crate::jobs::{JobEvent, JobKind, JobManager, JobProgress, JobResult, Progress};
use crate::transfer::executor::{TransferExecutionError, TransferProgress, execute_transfer};
use crate::transfer::{TransferMethod, TransferPlan};
use crate::transfer_queue::{
    CancelAction, QueueError, RetryDisposition, TransferQueueConfig, TransferQueueCore,
    TransferRateEstimator,
};
use crate::vfs::ProviderRegistry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferQueueSummary {
    pub running: usize,
    pub waiting: usize,
    pub paused: usize,
}

#[derive(Debug, Clone)]
struct TransferWork {
    plan: TransferPlan,
    names: Vec<String>,
}

#[derive(Debug)]
struct RuntimeState {
    core: TransferQueueCore,
    work: BTreeMap<String, TransferWork>,
}

/// In-process transfer queue runtime.
///
/// `JobManager` remains the lifecycle source of truth. This runtime only owns
/// frozen executable transfer payloads and scheduler state keyed by the same
/// JobId.
#[derive(Debug, Clone)]
pub struct TransferQueueRuntime {
    manager: JobManager,
    events: mpsc::UnboundedSender<JobEvent>,
    registry: ProviderRegistry,
    state: Arc<Mutex<RuntimeState>>,
}

impl TransferQueueRuntime {
    pub fn new(
        manager: JobManager,
        events: mpsc::UnboundedSender<JobEvent>,
        registry: ProviderRegistry,
        config: TransferQueueConfig,
    ) -> Self {
        Self {
            manager,
            events,
            registry,
            state: Arc::new(Mutex::new(RuntimeState {
                core: TransferQueueCore::new(config),
                work: BTreeMap::new(),
            })),
        }
    }

    /// Register one frozen transfer and make it eligible for background
    /// execution. Source/destination presentation stays in JobManager; the
    /// exact TransferPlan/spec stays here for execution.
    pub fn enqueue(&self, plan: TransferPlan, names: Vec<String>) -> Result<String, QueueError> {
        let description = format!("Transfer {} → {}", plan.source, plan.destination);
        let job = self.manager.create_job(
            "transfer",
            JobKind::Transfer,
            description,
            Some(plan.source.clone()),
            Some(plan.destination.clone()),
        );
        let id = job.id.clone();

        {
            let mut state = self.lock_state();
            state.core.enqueue(id.clone())?;
            state.work.insert(id.clone(), TransferWork { plan, names });
        }

        self.pump();
        Ok(id)
    }

    /// Cancel one queue job without affecting unrelated work.
    ///
    /// Queued/parked work is terminalized without provider execution. Active
    /// work receives JobManager's existing cancellation token and remains in a
    /// worker slot until the executor reports its truthful terminal outcome.
    pub fn cancel(&self, job_id: &str) -> Result<CancelAction, QueueError> {
        let action = {
            let mut state = self.lock_state();
            let action = state.core.request_cancel(job_id)?;
            if action == CancelAction::TerminalizeWithoutExecution {
                state.work.remove(job_id);
            }
            action
        };

        match action {
            CancelAction::TerminalizeWithoutExecution => {
                let _ = self.manager.cancel(job_id);
                let _ = self.manager.publish_event(
                    &self.events,
                    JobEvent::Cancelled {
                        id: job_id.to_string(),
                        result: JobResult::generic("transfer cancelled before execution", 0),
                    },
                );
                self.pump();
            }
            CancelAction::SignalActiveExecution => {
                let _ = self.manager.cancel(job_id);
            }
            CancelAction::AlreadyFinished => {}
        }

        Ok(action)
    }

    pub fn summary(&self) -> TransferQueueSummary {
        let state = self.lock_state();
        TransferQueueSummary {
            running: state.core.active_count(),
            waiting: state.core.waiting_count(),
            paused: state.core.parked_count(),
        }
    }

    fn pump(&self) {
        let starts = {
            let mut state = self.lock_state();
            let mut starts = Vec::new();

            while let Some(job_id) = state.core.next_runnable() {
                let Some(work) = state.work.get(&job_id).cloned() else {
                    let _ = state.core.finish(&job_id);
                    continue;
                };
                starts.push((job_id, work));
            }

            starts
        };

        for (job_id, work) in starts {
            let _ = self
                .manager
                .publish_event(&self.events, JobEvent::Running { id: job_id.clone() });

            let runtime = self.clone();
            tokio::spawn(async move {
                runtime.run_one(job_id, work).await;
                runtime.pump();
            });
        }
    }

    async fn run_one(&self, job_id: String, work: TransferWork) {
        let Some(cancel) = self.manager.cancel_token(&job_id) else {
            self.fail_scheduler(&job_id, RetryDisposition::NeverRetry);
            return;
        };

        let progress_manager = self.manager.clone();
        let progress_events = self.events.clone();
        let progress_id = job_id.clone();
        let method = work.plan.method;
        let mut rate = TransferRateEstimator::default();

        let result = execute_transfer(
            &work.plan,
            &work.names,
            &self.registry,
            cancel.clone(),
            move |event| {
                let progress = job_progress(method, event, &mut rate);
                let _ = progress_manager.publish_event(
                    &progress_events,
                    JobEvent::Progress {
                        id: progress_id.clone(),
                        progress,
                    },
                );
            },
        )
        .await;

        match result {
            Ok(outcome) => {
                self.finish_scheduler(&job_id);
                let _ = self.manager.publish_event(
                    &self.events,
                    JobEvent::Completed {
                        id: job_id,
                        result: JobResult::generic("transfer completed", outcome.completed),
                    },
                );
            }
            Err(TransferExecutionError::Cancelled { completed }) => {
                self.finish_scheduler(&job_id);
                let _ = self.manager.publish_event(
                    &self.events,
                    JobEvent::Cancelled {
                        id: job_id,
                        result: JobResult::generic("transfer cancelled", completed),
                    },
                );
            }
            Err(TransferExecutionError::Io(error))
                if error.kind() == std::io::ErrorKind::Interrupted
                    && cancel.load(Ordering::Acquire) =>
            {
                self.finish_scheduler(&job_id);
                let _ = self.manager.publish_event(
                    &self.events,
                    JobEvent::Cancelled {
                        id: job_id,
                        result: JobResult::generic_message("transfer cancelled"),
                    },
                );
            }
            Err(error) => {
                // Current TransferExecutionError does not yet prove whether a
                // remote mutation was dispatched. Fail closed: exactly one
                // attempt until providers expose typed retry disposition.
                self.fail_scheduler(&job_id, RetryDisposition::NeverRetry);
                let _ = self.manager.publish_event(
                    &self.events,
                    JobEvent::Failed {
                        id: job_id,
                        error: error.to_string(),
                        result: None,
                    },
                );
            }
        }
    }

    fn finish_scheduler(&self, job_id: &str) {
        let mut state = self.lock_state();
        let _ = state.core.finish(job_id);
        state.work.remove(job_id);
    }

    fn fail_scheduler(&self, job_id: &str, disposition: RetryDisposition) {
        let mut state = self.lock_state();

        if matches!(
            state.core.state(job_id),
            Some(
                crate::transfer_queue::SchedulerState::Active
                    | crate::transfer_queue::SchedulerState::PausePending
            )
        ) {
            let _ = state.core.failure(job_id, disposition);
        } else {
            let _ = state.core.finish(job_id);
        }
        state.work.remove(job_id);
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, RuntimeState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn job_progress(
    method: TransferMethod,
    progress: TransferProgress,
    rate: &mut TransferRateEstimator,
) -> JobProgress {
    match method {
        // Current WebDAV upload callbacks report bytes. Downloads do not yet
        // expose streaming progress and therefore stay indeterminate until the
        // executor/provider progress seam is widened later in PACK G.
        TransferMethod::WebDav => {
            let done = u64::try_from(progress.completed).unwrap_or(u64::MAX);
            let total = u64::try_from(progress.total).unwrap_or(u64::MAX);
            let bytes_per_second = rate.observe_at(Instant::now(), done).unwrap_or(0);
            JobProgress::Generic(Progress::Bytes {
                done,
                total,
                rate: bytes_per_second,
            })
        }
        // Native/rsync/SFTP callbacks are item counts. S3 upload callbacks are
        // currently part counts (including 1/1 for a single PUT), not bytes.
        // Do not fabricate byte speed/ETA from those values.
        TransferMethod::Native
        | TransferMethod::Rsync
        | TransferMethod::Sftp
        | TransferMethod::Scp
        | TransferMethod::S3 => JobProgress::Generic(Progress::Items {
            done: progress.completed,
            total: progress.total,
        }),
    }
}
