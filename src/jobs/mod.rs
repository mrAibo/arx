use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use tokio::sync::mpsc;

use crate::vfs::Location;
use crate::workspace_sync_executor::{
    CompiledSyncPlan, PhysicalStepId, SyncExecutionEvent, SyncExecutionOutcome, SyncTerminalState,
    WorkspaceSyncExecutor,
};
use crate::workspace_sync_verification::{
    SyncVerificationCoordinator, SyncVerificationEvent, SyncVerificationSnapshot,
    SyncVerificationStatus,
};

// ── Generic progress model ──

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Progress {
    #[default]
    Indeterminate,
    Percent(u8),
    Bytes {
        done: u64,
        total: u64,
        /// Transfer rate in bytes/sec (last-second sample).
        rate: u64,
    },
    Items {
        done: usize,
        total: usize,
    },
    Phase {
        phase: String,
        /// Sub-progress within the phase (e.g. "transferring" → 45%).
        percent: Option<u8>,
    },
}

impl Progress {
    pub fn percent(&self) -> Option<u8> {
        match self {
            Self::Percent(p) => Some(*p),
            Self::Bytes { done, total, .. } if *total > 0 => {
                Some(((((*done as u128) * 100) / (*total as u128)).min(100)) as u8)
            }
            Self::Items { done, total } if *total > 0 => {
                Some(((done * 100) / total).min(100) as u8)
            }
            Self::Phase { percent, .. } => *percent,
            Self::Indeterminate => None,
            _ => None,
        }
    }
}

impl From<u8> for Progress {
    fn from(p: u8) -> Self {
        Self::Percent(p)
    }
}

impl std::fmt::Display for Progress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Indeterminate => write!(f, "…"),
            Self::Percent(p) => write!(f, "{p}%"),
            Self::Bytes { done, total, rate } => {
                let dp = human_bytes(*done);
                let tp = human_bytes(*total);
                let rs = human_bytes(*rate);
                write!(f, "{dp}/{tp} ({rs}/s)")
            }
            Self::Items { done, total } => write!(f, "{done}/{total}"),
            Self::Phase { phase, percent } => match percent {
                Some(p) => write!(f, "{phase} {p}%"),
                None => write!(f, "{phase}"),
            },
        }
    }
}

fn human_bytes(n: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = n as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", n, UNITS[unit])
    } else {
        format!("{:.1} {}", size, UNITS[unit])
    }
}

// ── Structured job progress/results ──

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SyncJobProgress {
    pub completed_steps: usize,
    pub total_steps: usize,
    pub transferred_bytes: u64,
    pub total_bytes: u64,
    pub current_step: Option<PhysicalStepId>,
    pub current_path: Option<String>,
    /// Present only when the executor can truthfully report per-file bytes.
    /// Current transfer adapters are item-granular, so this is normally
    /// `None` until a transport exposes byte-level progress.
    pub current_file_bytes: Option<(u64, u64)>,
}

impl SyncJobProgress {
    pub fn percent(&self) -> Option<u8> {
        if self.total_bytes > 0 {
            return Some(
                ((((self.transferred_bytes as u128) * 100) / (self.total_bytes as u128)).min(100))
                    as u8,
            );
        }
        self.completed_steps
            .saturating_mul(100)
            .checked_div(self.total_steps)
            .map(|percent| percent.min(100) as u8)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobProgress {
    Generic(Progress),
    WorkspaceSync(SyncJobProgress),
}

impl JobProgress {
    pub fn percent(&self) -> Option<u8> {
        match self {
            Self::Generic(progress) => progress.percent(),
            Self::WorkspaceSync(progress) => progress.percent(),
        }
    }
}

impl Default for JobProgress {
    fn default() -> Self {
        Self::Generic(Progress::Indeterminate)
    }
}

impl From<Progress> for JobProgress {
    fn from(progress: Progress) -> Self {
        Self::Generic(progress)
    }
}

impl std::fmt::Display for JobProgress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Generic(progress) => progress.fmt(f),
            Self::WorkspaceSync(progress) => write!(
                f,
                "{}/{} steps · {}/{}",
                progress.completed_steps,
                progress.total_steps,
                human_bytes(progress.transferred_bytes),
                human_bytes(progress.total_bytes)
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobResult {
    Generic {
        message: Option<String>,
        completed_items: Option<usize>,
    },
    WorkspaceSync(SyncExecutionOutcome),
}

impl JobResult {
    pub fn generic(message: impl Into<String>, completed_items: usize) -> Self {
        Self::Generic {
            message: Some(message.into()),
            completed_items: Some(completed_items),
        }
    }

    pub fn generic_message(message: impl Into<String>) -> Self {
        Self::Generic {
            message: Some(message.into()),
            completed_items: None,
        }
    }

    pub fn message(&self) -> Option<&str> {
        match self {
            Self::Generic { message, .. } => message.as_deref(),
            Self::WorkspaceSync(_) => None,
        }
    }
}

// ── Job model ──

#[derive(Debug, Clone)]
pub struct Job {
    pub id: String,
    pub description: String,
    pub kind: JobKind,
    pub status: JobStatus,
    pub progress: JobProgress,
    pub source: Option<Location>,
    pub destination: Option<Location>,
    /// The one cancellation flag owned by this job and shared with executors.
    pub cancel: Arc<AtomicBool>,
    pub result: Option<JobResult>,
    pub error: Option<String>,
    /// Post-execution verification is a separate fact from terminal job status.
    pub verification: Option<SyncVerificationSnapshot>,
}

impl Job {
    fn new(
        id: String,
        description: String,
        kind: JobKind,
        source: Option<Location>,
        destination: Option<Location>,
    ) -> Self {
        Self {
            id,
            description,
            kind,
            status: JobStatus::Pending,
            progress: JobProgress::default(),
            source,
            destination,
            cancel: job_token(),
            result: None,
            error: None,
            verification: None,
        }
    }

    pub fn status_icon(&self) -> &str {
        match self.status {
            JobStatus::Pending => "⏳",
            JobStatus::Running => "⚡",
            JobStatus::Cancelling => "⏹",
            JobStatus::Paused => "⏸",
            JobStatus::Completed => "✅",
            JobStatus::Failed => "❌",
            JobStatus::Cancelled => "🚫",
        }
    }

    /// ETA string from generic byte progress. Sync ETA is intentionally left
    /// for the later Transfer Center layer once rate sampling is available.
    pub fn eta(&self) -> Option<String> {
        match &self.progress {
            JobProgress::Generic(Progress::Bytes { done, total, rate })
                if *rate > 0 && *total > 0 =>
            {
                let remaining = total.saturating_sub(*done);
                let secs = remaining / rate;
                if secs < 60 {
                    Some(format!("{secs}s"))
                } else if secs < 3600 {
                    Some(format!("{}m {}s", secs / 60, secs % 60))
                } else {
                    Some(format!("{}h {}m", secs / 3600, (secs % 3600) / 60))
                }
            }
            _ => None,
        }
    }
}

impl std::fmt::Display for Job {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.status_icon(), self.description)?;
        if matches!(
            self.status,
            JobStatus::Running | JobStatus::Cancelling | JobStatus::Paused
        ) {
            write!(f, " {}", self.progress)?;
        }
        if let Some(eta) = self.eta() {
            write!(f, " ETA {eta}")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobKind {
    Copy,
    Move,
    Delete,
    Search,
    Archive,
    Rsync,
    Checksum,
    RemoteCommand,
    Transfer,
    Synchronize,
    Custom(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Pending,
    Running,
    Cancelling,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl JobStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobEvent {
    Running {
        id: String,
    },
    Progress {
        id: String,
        progress: JobProgress,
    },
    Completed {
        id: String,
        result: JobResult,
    },
    Failed {
        id: String,
        error: String,
        result: Option<JobResult>,
    },
    Paused {
        id: String,
    },
    Cancelled {
        id: String,
        result: JobResult,
    },
}

impl JobEvent {
    pub fn id(&self) -> &str {
        match self {
            Self::Running { id }
            | Self::Progress { id, .. }
            | Self::Completed { id, .. }
            | Self::Failed { id, .. }
            | Self::Paused { id }
            | Self::Cancelled { id, .. } => id,
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed { .. } | Self::Failed { .. } | Self::Cancelled { .. }
        )
    }
}

/// Create a cancellation flag for a job.
pub fn job_token() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

// ── Job Manager ──

#[derive(Debug, Default)]
struct JobManagerState {
    jobs: BTreeMap<String, Job>,
    order: Vec<String>,
}

/// Runtime source of truth for job lifecycle, cancellation and terminal data.
///
/// The TUI may keep `snapshot()` results for rendering, but it must not own or
/// independently advance a job's lifecycle.
#[derive(Debug, Clone, Default)]
pub struct JobManager {
    state: Arc<Mutex<JobManagerState>>,
    next_id: Arc<AtomicU64>,
}

impl JobManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create_job(
        &self,
        prefix: &str,
        kind: JobKind,
        description: impl Into<String>,
        source: Option<Location>,
        destination: Option<Location>,
    ) -> Job {
        let sequence = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let id = format!("{prefix}-{sequence}");
        let job = Job::new(id.clone(), description.into(), kind, source, destination);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.order.push(id.clone());
        state.jobs.insert(id, job.clone());
        job
    }

    pub fn cancel(&self, id: &str) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(job) = state.jobs.get_mut(id) else {
            return false;
        };
        if job.status.is_terminal() {
            return job.status == JobStatus::Cancelled;
        }

        job.cancel.store(true, Ordering::Relaxed);
        if matches!(
            job.status,
            JobStatus::Pending | JobStatus::Running | JobStatus::Paused
        ) {
            job.status = JobStatus::Cancelling;
        }
        true
    }

    pub fn cancel_token(&self, id: &str) -> Option<Arc<AtomicBool>> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .jobs
            .get(id)
            .map(|job| job.cancel.clone())
    }

    /// Apply an executor/runtime event. Returns false when the event is stale,
    /// invalid for the current state, or tries to mutate an already-terminal job.
    pub fn apply_event(&self, event: &JobEvent) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(job) = state.jobs.get_mut(event.id()) else {
            return false;
        };
        if job.status.is_terminal() {
            return false;
        }

        match event {
            JobEvent::Running { .. } => {
                if job.status == JobStatus::Pending {
                    job.status = JobStatus::Running;
                    true
                } else {
                    // A cancellation request may race the worker start. Do not
                    // regress Cancelling back to Running, but let execution
                    // continue with the already-set shared token.
                    job.status == JobStatus::Cancelling
                }
            }
            JobEvent::Progress { progress, .. } => {
                if matches!(job.status, JobStatus::Running | JobStatus::Cancelling) {
                    job.progress = progress.clone();
                    true
                } else {
                    false
                }
            }
            JobEvent::Paused { .. } => {
                if job.status == JobStatus::Running {
                    job.status = JobStatus::Paused;
                    true
                } else {
                    false
                }
            }
            JobEvent::Completed { result, .. } => {
                job.status = JobStatus::Completed;
                job.result = Some(result.clone());
                job.error = None;
                true
            }
            JobEvent::Cancelled { result, .. } => {
                job.cancel.store(true, Ordering::Relaxed);
                job.status = JobStatus::Cancelled;
                job.result = Some(result.clone());
                job.error = None;
                true
            }
            JobEvent::Failed { error, result, .. } => {
                job.status = JobStatus::Failed;
                job.error = Some(error.clone());
                job.result = result.clone();
                true
            }
        }
    }

    /// Attach verification to a terminal sync job without mutating the
    /// executor-owned terminal status/result/error.
    pub fn apply_sync_verification(
        &self,
        id: &str,
        verification: &SyncVerificationSnapshot,
    ) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(job) = state.jobs.get_mut(id) else {
            return false;
        };
        if job.kind != JobKind::Synchronize || !job.status.is_terminal() {
            return false;
        }
        let Some(JobResult::WorkspaceSync(outcome)) = &job.result else {
            return false;
        };
        if outcome.plan_id != verification.plan_id
            || job.source.as_ref() != Some(&verification.left_root)
            || job.destination.as_ref() != Some(&verification.right_root)
        {
            return false;
        }

        match &job.verification {
            None => {
                if !matches!(verification.status, SyncVerificationStatus::Pending) {
                    return false;
                }
            }
            Some(current) if verification.id < current.id => return false,
            Some(current) if verification.id > current.id => {
                if !matches!(verification.status, SyncVerificationStatus::Pending) {
                    return false;
                }
            }
            Some(current) => {
                if current.plan_id != verification.plan_id
                    || current.left_root != verification.left_root
                    || current.right_root != verification.right_root
                    || !current.status.can_transition_to(&verification.status)
                {
                    return false;
                }
            }
        }

        job.verification = Some(verification.clone());
        true
    }

    pub fn snapshot(&self) -> Vec<Job> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state
            .order
            .iter()
            .filter_map(|id| state.jobs.get(id).cloned())
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<Job> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .jobs
            .get(id)
            .cloned()
    }

    /// Publish an event only after this manager has accepted the state
    /// transition. Rejected stale/duplicate terminal events never reach UI.
    pub fn publish_event(&self, events: &mpsc::UnboundedSender<JobEvent>, event: JobEvent) -> bool {
        if !self.apply_event(&event) {
            return false;
        }
        let _ = events.send(event);
        true
    }

    /// Spawn one workspace-sync job without a post-run verification pass.
    /// Remote Workspace uses the verified adapter below; compilation/planning
    /// remains outside the Job layer in both paths.
    pub fn spawn_workspace_sync(
        &self,
        compiled_plan: CompiledSyncPlan,
        executor: WorkspaceSyncExecutor,
        events: mpsc::UnboundedSender<JobEvent>,
    ) -> String {
        self.spawn_workspace_sync_inner(compiled_plan, executor, events, None)
    }

    pub fn spawn_workspace_sync_with_verification(
        &self,
        compiled_plan: CompiledSyncPlan,
        executor: WorkspaceSyncExecutor,
        events: mpsc::UnboundedSender<JobEvent>,
        verification: SyncVerificationCoordinator,
        verification_events: mpsc::UnboundedSender<SyncVerificationEvent>,
    ) -> String {
        self.spawn_workspace_sync_inner(
            compiled_plan,
            executor,
            events,
            Some((verification, verification_events)),
        )
    }

    fn spawn_workspace_sync_inner(
        &self,
        compiled_plan: CompiledSyncPlan,
        executor: WorkspaceSyncExecutor,
        events: mpsc::UnboundedSender<JobEvent>,
        verification: Option<(
            SyncVerificationCoordinator,
            mpsc::UnboundedSender<SyncVerificationEvent>,
        )>,
    ) -> String {
        let verification_plan_id = compiled_plan.plan_id();
        let verification_left_root = compiled_plan.left_root().clone();
        let verification_right_root = compiled_plan.right_root().clone();
        let description = format!(
            "Sync {} → {}",
            compiled_plan.left_root(),
            compiled_plan.right_root()
        );
        let job = self.create_job(
            "sync",
            JobKind::Synchronize,
            description,
            Some(compiled_plan.left_root().clone()),
            Some(compiled_plan.right_root().clone()),
        );
        let id = job.id.clone();
        let worker_id = id.clone();
        let manager = self.clone();
        let cancel = job.cancel.clone();
        let total_steps = compiled_plan.steps().len();
        let total_bytes = compiled_plan.total_bytes();
        let step_bytes = compiled_plan
            .steps()
            .iter()
            .map(|compiled| (compiled.id, compiled.step.bytes()))
            .collect::<BTreeMap<_, _>>();

        tokio::spawn(async move {
            publish(
                &manager,
                &events,
                JobEvent::Running {
                    id: worker_id.clone(),
                },
            );

            let mut progress = SyncJobProgress {
                total_steps,
                total_bytes,
                ..SyncJobProgress::default()
            };
            let (sync_tx, mut sync_rx) = mpsc::unbounded_channel();
            let mut execution = Box::pin(executor.execute(compiled_plan, cancel, sync_tx));

            let terminal = loop {
                tokio::select! {
                    Some(event) = sync_rx.recv() => {
                        if let Some(job_event) = sync_progress_event(
                            &worker_id,
                            &mut progress,
                            &step_bytes,
                            event,
                        ) {
                            publish(&manager, &events, job_event);
                        }
                    }
                    result = &mut execution => break result,
                }
            };

            while let Ok(event) = sync_rx.try_recv() {
                if let Some(job_event) =
                    sync_progress_event(&worker_id, &mut progress, &step_bytes, event)
                {
                    publish(&manager, &events, job_event);
                }
            }

            match terminal {
                Ok(outcome) => {
                    let should_verify = sync_outcome_needs_verification(&outcome);
                    let terminal_event = job_event_from_sync_outcome(worker_id.clone(), outcome);
                    let accepted = publish(&manager, &events, terminal_event);
                    if accepted
                        && should_verify
                        && let Some((coordinator, verification_events)) = verification
                    {
                        spawn_sync_verification_bridge(
                            manager.clone(),
                            worker_id,
                            verification_plan_id,
                            verification_left_root,
                            verification_right_root,
                            coordinator,
                            verification_events,
                        );
                    }
                }
                Err(error) => {
                    publish(
                        &manager,
                        &events,
                        JobEvent::Failed {
                            id: worker_id,
                            error: error.to_string(),
                            result: None,
                        },
                    );
                }
            }
        });

        id
    }
}

fn publish(
    manager: &JobManager,
    events: &mpsc::UnboundedSender<JobEvent>,
    event: JobEvent,
) -> bool {
    manager.publish_event(events, event)
}

fn sync_outcome_needs_verification(outcome: &SyncExecutionOutcome) -> bool {
    match &outcome.terminal {
        SyncTerminalState::Completed => true,
        SyncTerminalState::Cancelled { .. } | SyncTerminalState::Failed { .. } => {
            !outcome.completed.is_empty()
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_sync_verification_bridge(
    manager: JobManager,
    job_id: String,
    plan_id: crate::workspace_sync_execution::SyncPlanId,
    left_root: Location,
    right_root: Location,
    coordinator: SyncVerificationCoordinator,
    verification_events: mpsc::UnboundedSender<SyncVerificationEvent>,
) {
    let mut run = coordinator.start(job_id.clone(), plan_id, left_root, right_root);
    tokio::spawn(async move {
        while let Some(verification) = run.recv().await {
            if manager.apply_sync_verification(&job_id, &verification) {
                let _ = verification_events.send(SyncVerificationEvent {
                    job_id: job_id.clone(),
                    verification,
                });
            }
        }
    });
}

fn sync_progress_event(
    job_id: &str,
    progress: &mut SyncJobProgress,
    step_bytes: &BTreeMap<PhysicalStepId, u64>,
    event: SyncExecutionEvent,
) -> Option<JobEvent> {
    match event {
        SyncExecutionEvent::Started { .. } => None,
        SyncExecutionEvent::StepStarted { id, path } => {
            progress.current_step = Some(id);
            progress.current_path = Some(path);
            // Current transfer adapters expose item completion, not live byte
            // counters. Do not manufacture per-file byte progress.
            progress.current_file_bytes = None;
            Some(JobEvent::Progress {
                id: job_id.to_string(),
                progress: JobProgress::WorkspaceSync(progress.clone()),
            })
        }
        SyncExecutionEvent::Progress {
            completed_steps,
            total_steps,
            transferred_bytes,
            total_bytes,
        } => {
            progress.completed_steps = completed_steps;
            progress.total_steps = total_steps;
            progress.transferred_bytes = transferred_bytes;
            progress.total_bytes = total_bytes;
            Some(JobEvent::Progress {
                id: job_id.to_string(),
                progress: JobProgress::WorkspaceSync(progress.clone()),
            })
        }
        SyncExecutionEvent::StepCompleted { id, .. } => {
            if progress.current_step == Some(id)
                && let Some(bytes) = step_bytes.get(&id).copied().filter(|bytes| *bytes > 0)
            {
                progress.current_file_bytes = Some((bytes, bytes));
            }
            Some(JobEvent::Progress {
                id: job_id.to_string(),
                progress: JobProgress::WorkspaceSync(progress.clone()),
            })
        }
        // The typed executor outcome is the one terminal source. These events
        // are useful to direct executor consumers, but the job adapter must not
        // race them against the final `SyncExecutionOutcome`.
        SyncExecutionEvent::Cancelled { .. }
        | SyncExecutionEvent::Failed { .. }
        | SyncExecutionEvent::Completed { .. } => None,
    }
}

fn job_event_from_sync_outcome(id: String, outcome: SyncExecutionOutcome) -> JobEvent {
    match &outcome.terminal {
        SyncTerminalState::Completed => JobEvent::Completed {
            id,
            result: JobResult::WorkspaceSync(outcome),
        },
        SyncTerminalState::Cancelled { .. } => JobEvent::Cancelled {
            id,
            result: JobResult::WorkspaceSync(outcome),
        },
        SyncTerminalState::Failed { error, .. } => JobEvent::Failed {
            id,
            error: error.to_string(),
            result: Some(JobResult::WorkspaceSync(outcome)),
        },
    }
}

#[derive(Debug, Clone, Default)]
pub struct TransferStats {
    pub bytes_total: u64,
    pub bytes_done: u64,
    pub started: Option<Instant>,
    pub speed: f64,
}

#[derive(Debug, Clone, Default)]
pub struct TransferTask {
    pub id: String,
    pub src: String,
    pub dst: String,
    pub files: Vec<String>,
    pub stats: TransferStats,
    pub cancel: Arc<AtomicBool>,
}

impl TransferTask {
    pub fn new(id: &str, src: &str, dst: &str, files: &[String]) -> Self {
        Self {
            id: id.to_string(),
            src: src.to_string(),
            dst: dst.to_string(),
            files: files.to_vec(),
            cancel: job_token(),
            ..Default::default()
        }
    }

    pub fn eta_seconds(&self) -> Option<u64> {
        if self.stats.speed <= 0.0 {
            return None;
        }
        let remaining = self.stats.bytes_total.saturating_sub(self.stats.bytes_done);
        Some((remaining as f64 / self.stats.speed) as u64)
    }

    pub fn is_done(&self) -> bool {
        self.stats.bytes_done >= self.stats.bytes_total && self.stats.bytes_total > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    fn manager_job(manager: &JobManager) -> Job {
        manager.create_job("test", JobKind::Transfer, "test", None, None)
    }

    #[test]
    fn job_starts_pending_and_manager_owns_snapshot() {
        let manager = JobManager::new();
        let job = manager_job(&manager);
        assert_eq!(job.status, JobStatus::Pending);
        let snapshot = manager.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].status, JobStatus::Pending);
    }

    #[test]
    fn active_cancel_sets_the_jobs_shared_token() {
        let manager = JobManager::new();
        let job = manager_job(&manager);
        let token = job.cancel.clone();
        assert!(manager.apply_event(&JobEvent::Running { id: job.id.clone() }));
        assert!(manager.cancel(&job.id));
        assert!(token.load(Ordering::Relaxed));
        assert_eq!(manager.get(&job.id).unwrap().status, JobStatus::Cancelling);
    }

    #[test]
    fn terminal_job_cannot_return_to_running() {
        let manager = JobManager::new();
        let job = manager_job(&manager);
        assert!(manager.apply_event(&JobEvent::Running { id: job.id.clone() }));
        assert!(manager.apply_event(&JobEvent::Completed {
            id: job.id.clone(),
            result: JobResult::generic_message("done"),
        }));
        assert!(!manager.apply_event(&JobEvent::Running { id: job.id.clone() }));
        assert_eq!(manager.get(&job.id).unwrap().status, JobStatus::Completed);
    }

    #[test]
    fn duplicate_terminal_event_is_rejected() {
        let manager = JobManager::new();
        let job = manager_job(&manager);
        assert!(manager.apply_event(&JobEvent::Running { id: job.id.clone() }));
        assert!(manager.apply_event(&JobEvent::Cancelled {
            id: job.id.clone(),
            result: JobResult::generic("cancelled", 0),
        }));
        assert!(!manager.apply_event(&JobEvent::Failed {
            id: job.id.clone(),
            error: "late failure".into(),
            result: None,
        }));
        assert_eq!(manager.get(&job.id).unwrap().status, JobStatus::Cancelled);
    }

    #[test]
    fn racing_terminal_events_leave_exactly_one_terminal_state() {
        let manager = JobManager::new();
        let job = manager_job(&manager);
        assert!(manager.apply_event(&JobEvent::Running { id: job.id.clone() }));

        let left = manager.clone();
        let left_id = job.id.clone();
        let right = manager.clone();
        let right_id = job.id.clone();
        let completed = thread::spawn(move || {
            left.apply_event(&JobEvent::Completed {
                id: left_id,
                result: JobResult::generic_message("completed"),
            })
        });
        let cancelled = thread::spawn(move || {
            right.apply_event(&JobEvent::Cancelled {
                id: right_id,
                result: JobResult::generic("cancelled", 0),
            })
        });

        let accepted =
            usize::from(completed.join().unwrap()) + usize::from(cancelled.join().unwrap());
        assert_eq!(accepted, 1);
        assert!(manager.get(&job.id).unwrap().status.is_terminal());
    }

    #[test]
    fn sync_progress_prefers_byte_percent_without_losing_step_counts() {
        let progress = SyncJobProgress {
            completed_steps: 12,
            total_steps: 31,
            transferred_bytes: 84,
            total_bytes: 220,
            current_step: Some(PhysicalStepId(13)),
            current_path: Some("assets/video.mp4".into()),
            current_file_bytes: None,
        };
        assert_eq!(progress.percent(), Some(38));
        assert_eq!(progress.completed_steps, 12);
        assert_eq!(progress.total_steps, 31);
    }
}
