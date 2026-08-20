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
    CancelAction, QueueError, RetryDecision, RetryDisposition, SchedulerState, TransferQueueConfig,
    TransferQueueCore, TransferRateEstimator,
};
use crate::transfer_retry;
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

    /// Read-only access to the shared `JobManager` for product-path tests and
    /// observers. The runtime remains the only place that mutates transfer
    /// jobs; this never hands out a second lifecycle owner.
    pub fn manager(&self) -> &JobManager {
        &self.manager
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
            Err(TransferExecutionError::Io {
                source: error,
                disposition,
            }) => {
                if error.kind() == std::io::ErrorKind::Interrupted && cancel.load(Ordering::Acquire)
                {
                    self.finish_scheduler(&job_id);
                    let _ = self.manager.publish_event(
                        &self.events,
                        JobEvent::Cancelled {
                            id: job_id,
                            result: JobResult::generic_message("transfer cancelled"),
                        },
                    );
                } else {
                    self.classify_failure(&job_id, disposition, &error);
                }
            }
            Err(error) => {
                // Fail closed on every other error kind. The executor attaches
                // a typed disposition where it knows mutation certainty; a
                // default error is never auto-replayed.
                let disposition = error.retry_disposition();
                self.classify_failure(&job_id, disposition, &error);
            }
        }
    }

    /// Truthfully terminalize or safely retry a failed attempt.
    ///
    /// `SafeToRetry` (read-side / staged-local failures) may be replayed up to
    /// `MAX_TOTAL_TRANSFER_ATTEMPTS`, honoring bounded backoff and a
    /// `RetryWaiting` status. Every other disposition — including ambiguous
    /// remote mutations — stops at exactly one attempt.
    fn classify_failure(
        &self,
        job_id: &str,
        disposition: RetryDisposition,
        error: &dyn std::fmt::Display,
    ) {
        // `core.failure` is the single authoritative transition: it moves
        // Active -> RetryWaiting (safe retry within bounds) or Active ->
        // Finished (stop). We must not call it twice, so subsequent handling
        // only removes the work entry and publishes the truthful event.
        let decision = {
            let mut state = self.lock_state();
            match state.core.state(job_id) {
                Some(SchedulerState::Active | SchedulerState::PausePending) => {
                    state.core.failure(job_id, disposition)
                }
                _ => Ok(RetryDecision::Stop {
                    disposition,
                    attempts_started: state.core.attempts_started(job_id).unwrap_or(1),
                }),
            }
        };

        match decision {
            Ok(RetryDecision::RetryAfterBackoff {
                next_attempt,
                max_total_attempts,
            }) => {
                // Truthful RetryWaiting status, then bounded backoff, then
                // re-enter the scheduler with the SAME JobId.
                let _ = self.manager.publish_event(
                    &self.events,
                    JobEvent::RetryWaiting {
                        id: job_id.to_string(),
                    },
                );
                if let Some(delay) = transfer_retry::retry_backoff(next_attempt, max_total_attempts)
                {
                    let runtime = self.clone();
                    let job_id = job_id.to_string();
                    tokio::spawn(async move {
                        tokio::time::sleep(delay).await;
                        // §8: a cancel during RetryWaiting must not resurrect
                        // the job. Only re-arm if the scheduler still holds it
                        // in RetryWaiting; otherwise the work is gone/cancelled.
                        let rearm = {
                            let state = runtime.lock_state();
                            matches!(
                                state.core.state(&job_id),
                                Some(SchedulerState::RetryWaiting)
                            )
                        };
                        if rearm {
                            {
                                let mut state = runtime.lock_state();
                                let _ = state.core.release_retry(&job_id);
                            }
                            runtime.pump();
                        }
                    });
                }
                self.pump();
            }
            Ok(RetryDecision::Stop {
                disposition: _disposition,
                attempts_started,
            }) => {
                let _ = self.manager.publish_event(
                    &self.events,
                    JobEvent::Failed {
                        id: job_id.to_string(),
                        error: format!(
                            "transfer failed after {attempts_started} attempt(s): {error}"
                        ),
                        result: None,
                    },
                );
                self.terminalize(job_id);
            }
            Err(_) => {
                self.terminalize(job_id);
            }
        }
    }

    /// Remove a job from the runtime scheduler state without re-applying a
    /// transition (the transition already happened in `classify_failure`).
    fn terminalize(&self, job_id: &str) {
        let mut state = self.lock_state();
        let _ = state.core.finish(job_id);
        state.work.remove(job_id);
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
