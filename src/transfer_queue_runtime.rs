//! Background transfer queue runtime built on the existing JobManager and
//! transfer executor.
//!
//! The runtime deliberately keeps retry fail-closed for now: current executor
//! errors do not carry enough mutation-certainty information to safely replay
//! remote writes. Provider-specific retry classification is a later PACK G
//! integration step. Until then, every execution failure is one-shot.

use std::collections::BTreeMap;
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::jobs::{JobEvent, JobKind, JobManager, JobProgress, JobResult, Progress};
use crate::transfer::TransferPlan;
use crate::transfer::executor::{TransferExecutionError, execute_transfer};
use crate::transfer_queue::{
    CancelAction, PauseAction, PauseGate, QueueError, RetryDecision, RetryDisposition,
    RunnableAction, SchedulerState, TransferProgressTracker, TransferQueueConfig,
    TransferQueueCore, TransferRateEstimator, TypedTransferProgress,
};
use crate::transfer_retry;
use crate::vfs::ProviderRegistry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferQueueSummary {
    pub running: usize,
    pub waiting: usize,
    pub paused: usize,
}

/// Narrow read-only view of one job's scheduler truth, exposed to the Transfer
/// Center detail pane. Derived entirely from existing `TransferQueueCore` state;
/// never mutates the queue or pumps the scheduler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferRuntimeJobInfo {
    pub scheduler_state: SchedulerState,
    pub attempts_started: u8,
}

#[derive(Debug, Clone)]
struct TransferWork {
    plan: TransferPlan,
    names: Vec<String>,
    gate: PauseGate,
    progress: Arc<Mutex<(TransferProgressTracker, TransferRateEstimator)>>,
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
    tasks: Arc<Mutex<BTreeMap<String, JoinHandle<()>>>>,
    timers: Arc<Mutex<BTreeMap<String, JoinHandle<()>>>>,
    closed: Arc<AtomicBool>,
    shutdown: Arc<tokio::sync::Mutex<()>>,
    #[cfg(test)]
    worker_spawns: Arc<AtomicUsize>,
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
            tasks: Arc::new(Mutex::new(BTreeMap::new())),
            timers: Arc::new(Mutex::new(BTreeMap::new())),
            closed: Arc::new(AtomicBool::new(false)),
            shutdown: Arc::new(tokio::sync::Mutex::new(())),
            #[cfg(test)]
            worker_spawns: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Register one frozen transfer and make it eligible for background
    /// execution. Source/destination presentation stays in JobManager; the
    /// exact TransferPlan/spec stays here for execution.
    pub fn enqueue(&self, plan: TransferPlan, names: Vec<String>) -> Result<String, QueueError> {
        let id = {
            // Runtime state is the enqueue/shutdown linearization point. If
            // shutdown closes first, no JobManager job is created. If enqueue
            // owns the lock first, shutdown's subsequent snapshot includes the
            // fully registered work item.
            let mut state = self.lock_state();
            if self.closed.load(Ordering::Acquire) {
                return Err(QueueError::Closed);
            }
            let description = format!("Transfer {} → {}", plan.source, plan.destination);
            let job = self.manager.create_job(
                "transfer",
                JobKind::Transfer,
                description,
                Some(plan.source.clone()),
                Some(plan.destination.clone()),
            );
            let id = job.id.clone();
            state.core.enqueue(id.clone())?;
            let cancel = self
                .manager
                .cancel_token(&id)
                .ok_or_else(|| QueueError::UnknownJob(id.clone()))?;
            state.work.insert(
                id.clone(),
                TransferWork {
                    plan,
                    names,
                    gate: PauseGate::new(cancel),
                    progress: Arc::new(Mutex::new((
                        TransferProgressTracker::default(),
                        TransferRateEstimator::default(),
                    ))),
                },
            );
            id
        };

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
                if let Some(timer) = self.lock_timers().remove(job_id) {
                    timer.abort();
                }
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
                if let Some(work) = self.lock_state().work.get(job_id) {
                    work.gate.wake_cancelled();
                }
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

    pub fn config(&self) -> TransferQueueConfig {
        self.lock_state().core.config()
    }

    /// Read-only snapshot of a job's scheduler truth. Never mutates the queue,
    /// pumps the scheduler, touches the cancel token, or publishes events.
    pub fn inspect_job(&self, job_id: &str) -> Option<TransferRuntimeJobInfo> {
        let state = self.lock_state();
        Some(TransferRuntimeJobInfo {
            scheduler_state: state.core.state(job_id)?,
            attempts_started: state.core.attempts_started(job_id)?,
        })
    }

    /// Read-only access to the shared `JobManager` for product-path tests and
    /// observers. The runtime remains the only place that mutates transfer
    /// jobs; this never hands out a second lifecycle owner.
    pub fn manager(&self) -> &JobManager {
        &self.manager
    }

    pub fn request_pause(&self, job_id: &str) -> Result<PauseAction, QueueError> {
        let (action, gate) = {
            let mut state = self.lock_state();
            let action = state.core.request_pause(job_id)?;
            (action, state.work.get(job_id).map(|work| work.gate.clone()))
        };
        match action {
            PauseAction::ParkedBeforeExecution => {
                let _ = self
                    .manager
                    .publish_event(&self.events, JobEvent::Paused { id: job_id.into() });
                self.pump();
            }
            PauseAction::AwaitSafeCheckpoint => {
                let _ = self
                    .manager
                    .publish_event(&self.events, JobEvent::PausePending { id: job_id.into() });
                if let Some(gate) = gate {
                    gate.request();
                    let runtime = self.clone();
                    let id = job_id.to_string();
                    let key = format!("pause:{id}");
                    {
                        let mut tasks = self.lock_tasks();
                        if tasks.get(&key).is_some_and(JoinHandle::is_finished) {
                            tasks.remove(&key);
                        }
                        if tasks.contains_key(&key) {
                            return Ok(action);
                        }
                    }
                    let handle = tokio::spawn(async move {
                        gate.wait_checkpoint().await;
                        if gate.checkpoint_reached() {
                            let confirmed =
                                { runtime.lock_state().core.confirm_paused(&id).is_ok() };
                            if confirmed {
                                let _ = runtime
                                    .manager
                                    .publish_event(&runtime.events, JobEvent::Paused { id });
                                runtime.pump();
                            }
                        }
                    });
                    self.lock_tasks().insert(key, handle);
                }
            }
            _ => {}
        }
        Ok(action)
    }

    pub fn resume(&self, job_id: &str) -> Result<(), QueueError> {
        {
            self.lock_state().core.resume(job_id)?;
        }
        self.pump();
        Ok(())
    }

    fn pump(&self) {
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        self.reap_finished();
        let actions = {
            let mut state = self.lock_state();
            let mut actions = Vec::new();

            while let Some(action) = state.core.next_action() {
                let job_id = match &action {
                    RunnableAction::Start(id) | RunnableAction::Resume(id) => id,
                };
                let Some(work) = state.work.get(job_id).cloned() else {
                    let _ = state.core.finish(job_id);
                    continue;
                };
                actions.push((action, work));
            }

            actions
        };

        for (action, work) in actions {
            let job_id = match action {
                RunnableAction::Start(id) => {
                    work.gate.reset_attempt();
                    id
                }
                RunnableAction::Resume(id) => {
                    if let Ok(mut progress) = work.progress.lock() {
                        progress.1.reset_attempt();
                    }
                    work.gate.resume();
                    let _ = self
                        .manager
                        .publish_event(&self.events, JobEvent::Running { id: id.clone() });
                    continue;
                }
            };
            let _ = self
                .manager
                .publish_event(&self.events, JobEvent::Running { id: job_id.clone() });
            let _ = self.manager.publish_event(
                &self.events,
                JobEvent::Progress {
                    id: job_id.clone(),
                    progress: JobProgress::Generic(Progress::Indeterminate),
                },
            );

            let runtime = self.clone();
            let task_key = format!("worker:{job_id}");
            #[cfg(test)]
            self.worker_spawns.fetch_add(1, Ordering::AcqRel);
            let handle = tokio::spawn(async move {
                runtime.run_one(job_id, work).await;
                runtime.pump();
            });
            self.lock_tasks().insert(task_key, handle);
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
        if let Ok(mut progress) = work.progress.lock() {
            progress.0.reset_attempt();
            progress.1.reset_attempt();
        }
        let shared_progress = work.progress.clone();

        let result = execute_transfer(
            &work.plan,
            &work.names,
            &self.registry,
            cancel.clone(),
            work.gate.clone(),
            move |event| {
                let Ok(mut progress_state) = shared_progress.lock() else {
                    return;
                };
                if progress_state.0.observe(event).is_err() {
                    return;
                }
                let progress = job_progress(event, &mut progress_state.1);
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
        work.gate.settle();

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
                if error.kind() == std::io::ErrorKind::Interrupted
                    && cancel.load(Ordering::Acquire)
                    && matches!(
                        disposition,
                        RetryDisposition::SafeToRetry | RetryDisposition::NeverRetry
                    )
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
                    let timer_key = job_id.clone();
                    let handle = tokio::spawn(async move {
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
                    self.lock_timers().insert(timer_key, handle);
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

    fn lock_tasks(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, JoinHandle<()>>> {
        self.tasks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_timers(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, JoinHandle<()>>> {
        self.timers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn reap_finished(&self) {
        self.lock_tasks().retain(|_, handle| !handle.is_finished());
        self.lock_timers().retain(|_, handle| !handle.is_finished());
    }

    pub async fn shutdown(&self) {
        // Concurrent callers wait for the same complete shutdown boundary;
        // none may return while an earlier caller still owns mutable tasks.
        let _shutdown = self.shutdown.lock().await;
        self.closed.store(true, Ordering::Release);
        let ids = {
            let state = self.lock_state();
            state.work.keys().cloned().collect::<Vec<_>>()
        };
        for id in ids {
            let _ = self.cancel(&id);
        }
        let timers = {
            let mut timers = self.lock_timers();
            std::mem::take(&mut *timers)
        };
        for (_, timer) in timers {
            timer.abort();
            let _ = timer.await;
        }
        loop {
            let tasks = {
                let mut tasks = self.lock_tasks();
                std::mem::take(&mut *tasks)
            };
            if tasks.is_empty() {
                break;
            }
            for (_, task) in tasks {
                let _ = task.await;
            }
        }
        // A settling worker can schedule SafeToRetry after the initial timer
        // drain. Once all workers/watchers above are joined, no timer producer
        // remains, so drain to a stable empty set before returning.
        loop {
            let timers = {
                let mut timers = self.lock_timers();
                std::mem::take(&mut *timers)
            };
            if timers.is_empty() {
                break;
            }
            for (_, timer) in timers {
                timer.abort();
                let _ = timer.await;
            }
        }
    }
}

fn job_progress(progress: TypedTransferProgress, rate: &mut TransferRateEstimator) -> JobProgress {
    match progress {
        TypedTransferProgress::Bytes { completed, total } => {
            let bytes_per_second = rate.observe_at(Instant::now(), completed).unwrap_or(0);
            JobProgress::Generic(Progress::Bytes {
                done: completed,
                total,
                rate: bytes_per_second,
            })
        }
        TypedTransferProgress::Items { completed, total } => match total {
            Some(total) => JobProgress::Generic(Progress::Items {
                done: usize::try_from(completed).unwrap_or(usize::MAX),
                total: usize::try_from(total).unwrap_or(usize::MAX),
            }),
            None => JobProgress::Generic(Progress::Indeterminate),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transfer::{TransferIntent, TransferMethod};
    use crate::vfs::Location;
    use std::path::PathBuf;
    use tokio::sync::Notify;

    fn runtime() -> TransferQueueRuntime {
        let (events, _receiver) = mpsc::unbounded_channel();
        TransferQueueRuntime::new(
            JobManager::new(),
            events,
            ProviderRegistry::new(),
            TransferQueueConfig::new(1).unwrap(),
        )
    }

    fn plan() -> TransferPlan {
        TransferPlan {
            source: Location::Local(PathBuf::from("/enqueue-race-source")),
            destination: Location::Local(PathBuf::from("/enqueue-race-destination")),
            intent: TransferIntent::Copy,
            method: TransferMethod::Native,
            archive_spec: None,
            s3_spec: None,
            webdav_spec: None,
        }
    }

    #[test]
    fn enqueue_closed_while_waiting_for_state_creates_no_job() {
        let runtime = runtime();
        let state = runtime.lock_state();
        let enqueue_runtime = runtime.clone();
        let enqueue = std::thread::spawn(move || enqueue_runtime.enqueue(plan(), vec!["x".into()]));

        // Enqueue is blocked at the runtime-state linearization point. Closing
        // before releasing it must prevent even the JobManager insertion.
        runtime.closed.store(true, Ordering::Release);
        drop(state);

        assert_eq!(enqueue.join().unwrap(), Err(QueueError::Closed));
        assert!(runtime.manager.snapshot().is_empty());
        assert!(runtime.lock_state().work.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn resume_wakes_one_worker_without_starting_an_attempt() {
        let runtime = runtime();
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let destination = dir.path().join("destination");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&destination).unwrap();
        std::fs::write(source.join("a"), b"a").unwrap();
        std::fs::write(source.join("b"), b"b").unwrap();
        let mut transfer = plan();
        transfer.source = Location::Local(source);
        transfer.destination = Location::Local(destination);
        let id = runtime
            .enqueue(transfer, vec!["a".into(), "b".into()])
            .unwrap();
        assert_eq!(
            runtime.request_pause(&id),
            Ok(PauseAction::AwaitSafeCheckpoint)
        );

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while runtime.manager.get(&id).unwrap().status != crate::jobs::JobStatus::Paused {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("worker reaches pause checkpoint");
        assert_eq!(runtime.worker_spawns.load(Ordering::Acquire), 1);
        assert_eq!(runtime.lock_state().core.attempts_started(&id), Some(1));

        runtime.resume(&id).unwrap();
        assert_eq!(runtime.worker_spawns.load(Ordering::Acquire), 1);
        assert_eq!(runtime.lock_state().core.attempts_started(&id), Some(1));
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !runtime.manager.get(&id).unwrap().status.is_terminal() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("same worker completes after resume");
        assert_eq!(runtime.worker_spawns.load(Ordering::Acquire), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn shutdown_wakes_and_joins_paused_worker() {
        let runtime = runtime();
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let destination = dir.path().join("destination");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&destination).unwrap();
        std::fs::write(source.join("a"), b"a").unwrap();
        let mut transfer = plan();
        transfer.source = Location::Local(source);
        transfer.destination = Location::Local(destination);
        let id = runtime.enqueue(transfer, vec!["a".into()]).unwrap();
        runtime.request_pause(&id).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while runtime.manager.get(&id).unwrap().status != crate::jobs::JobStatus::Paused {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("worker reaches pause checkpoint");

        tokio::time::timeout(std::time::Duration::from_secs(1), runtime.shutdown())
            .await
            .expect("shutdown joins paused worker");
        assert_eq!(
            runtime.manager.get(&id).unwrap().status,
            crate::jobs::JobStatus::Cancelled
        );
        assert!(runtime.lock_tasks().is_empty());
        assert!(runtime.lock_timers().is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn shutdown_drains_timer_created_by_settling_worker() {
        let runtime = runtime();
        let release_worker = Arc::new(Notify::new());
        let worker_runtime = runtime.clone();
        let worker_release = release_worker.clone();
        let worker = tokio::spawn(async move {
            worker_release.notified().await;
            let timer = tokio::spawn(std::future::pending());
            worker_runtime.lock_timers().insert("late".into(), timer);
        });
        runtime.lock_tasks().insert("worker:test".into(), worker);

        let shutdown_runtime = runtime.clone();
        let shutdown = tokio::spawn(async move { shutdown_runtime.shutdown().await });
        tokio::task::yield_now().await;
        release_worker.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(1), shutdown)
            .await
            .expect("shutdown joins worker and aborts its late timer")
            .expect("shutdown task completes");

        assert!(runtime.lock_tasks().is_empty());
        assert!(runtime.lock_timers().is_empty());
    }

    #[test]
    fn config_accessor_reports_configured_truth() {
        let runtime = runtime();
        assert_eq!(runtime.config().concurrency(), 1);
        assert!(runtime.config().max_total_attempts() >= 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn inspect_job_is_read_only_and_reports_core_truth() {
        let runtime = runtime();
        let id = runtime.enqueue(plan(), vec!["a".into()]).unwrap();

        // Unknown id is absent; no crash.
        assert_eq!(runtime.inspect_job("does-not-exist"), None);

        // Known id resolves to existing scheduler truth.
        let first = runtime.inspect_job(&id);
        assert!(first.is_some());
        // Read-only + deterministic: repeated calls agree, core unchanged.
        assert_eq!(first, runtime.inspect_job(&id));
        assert_eq!(
            runtime.lock_state().core.attempts_started(&id),
            Some(first.unwrap().attempts_started)
        );
    }
}
