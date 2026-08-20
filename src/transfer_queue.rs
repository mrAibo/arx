//! Transfer Queue control-plane primitives.
//!
//! This module deliberately does **not** execute transfers and does not own the
//! user-visible job lifecycle. `JobManager` remains the lifecycle source of
//! truth. The queue core only tracks scheduler eligibility, bounded concurrency,
//! pause checkpoints and retry policy for an existing JobId.
//!
//! Provider/executor code must classify retry safety explicitly. The default
//! for an unclassified failure is `NeverRetry`; ambiguous mutations and
//! recovery-required outcomes are never replayed automatically.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

pub const DEFAULT_TRANSFER_CONCURRENCY: usize = 2;
pub const MAX_TRANSFER_CONCURRENCY: usize = 8;
pub const MAX_TOTAL_TRANSFER_ATTEMPTS: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferQueueConfig {
    concurrency: usize,
    max_total_attempts: u8,
}

impl Default for TransferQueueConfig {
    fn default() -> Self {
        Self {
            concurrency: DEFAULT_TRANSFER_CONCURRENCY,
            max_total_attempts: MAX_TOTAL_TRANSFER_ATTEMPTS,
        }
    }
}

impl TransferQueueConfig {
    pub fn new(concurrency: usize) -> Result<Self, QueueError> {
        if !(1..=MAX_TRANSFER_CONCURRENCY).contains(&concurrency) {
            return Err(QueueError::InvalidConcurrency(concurrency));
        }
        Ok(Self {
            concurrency,
            ..Self::default()
        })
    }

    pub fn concurrency(self) -> usize {
        self.concurrency
    }

    pub fn max_total_attempts(self) -> u8 {
        self.max_total_attempts
    }
}

/// Explicit retry classification supplied by the executor/provider boundary.
///
/// `NeverRetry` is intentionally the safe default. Queue code must never infer
/// retry safety from error text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RetryDisposition {
    SafeToRetry,
    #[default]
    NeverRetry,
    AmbiguousMutation,
    RecoveryRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryDecision {
    RetryAfterBackoff {
        next_attempt: u8,
        max_total_attempts: u8,
    },
    Stop {
        disposition: RetryDisposition,
        attempts_started: u8,
    },
}

/// Scheduler-only state. This is not a replacement for `jobs::JobStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulerState {
    Waiting,
    Active,
    PausePending,
    Parked,
    RetryWaiting,
    Cancelling,
    Finished,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueError {
    InvalidConcurrency(usize),
    DuplicateJob(String),
    UnknownJob(String),
    InvalidTransition {
        job_id: String,
        from: SchedulerState,
        action: &'static str,
    },
    ProgressRegressed {
        previous: u64,
        current: u64,
    },
    ProgressUnitChanged,
    TimeRegressed,
}

impl std::fmt::Display for QueueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConcurrency(value) => write!(
                f,
                "transfer concurrency must be between 1 and {MAX_TRANSFER_CONCURRENCY}, got {value}"
            ),
            Self::DuplicateJob(id) => write!(f, "transfer job already queued: {id}"),
            Self::UnknownJob(id) => write!(f, "unknown transfer job: {id}"),
            Self::InvalidTransition {
                job_id,
                from,
                action,
            } => write!(
                f,
                "cannot {action} transfer job {job_id} from scheduler state {from:?}"
            ),
            Self::ProgressRegressed { previous, current } => write!(
                f,
                "transfer progress regressed from {previous} to {current}"
            ),
            Self::ProgressUnitChanged => {
                write!(f, "transfer progress changed units within one attempt")
            }
            Self::TimeRegressed => write!(f, "monotonic transfer sample time regressed"),
        }
    }
}

impl std::error::Error for QueueError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelAction {
    /// The job never started (or is genuinely parked) and can be terminalized
    /// without invoking provider code.
    TerminalizeWithoutExecution,
    /// Active provider/executor work must receive the existing cancellation
    /// signal and later publish the truthful terminal outcome.
    SignalActiveExecution,
    AlreadyFinished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseAction {
    /// Active work must reach a provider-safe checkpoint before JobManager can
    /// truthfully publish `Paused`.
    AwaitSafeCheckpoint,
    /// A not-yet-running job can be parked immediately.
    ParkedBeforeExecution,
    AlreadyParked,
    AlreadyFinished,
}

#[derive(Debug, Clone)]
struct QueueEntry {
    state: SchedulerState,
    attempts_started: u8,
    /// True only when the next activation begins a fresh retry/initial attempt.
    /// Resume after a genuine pause keeps this false so pause/resume never
    /// inflates the retry-attempt counter.
    start_new_attempt: bool,
}

impl QueueEntry {
    fn new() -> Self {
        Self {
            state: SchedulerState::Waiting,
            attempts_started: 0,
            start_new_attempt: true,
        }
    }
}

/// Deterministic queue/scheduler state machine.
///
/// The caller owns execution tasks, provider handles, JobManager events and
/// backoff timers. This core decides only which existing JobId may run next and
/// whether a failed attempt may be scheduled again.
#[derive(Debug, Clone)]
pub struct TransferQueueCore {
    config: TransferQueueConfig,
    entries: BTreeMap<String, QueueEntry>,
    waiting: VecDeque<String>,
}

impl Default for TransferQueueCore {
    fn default() -> Self {
        Self::new(TransferQueueConfig::default())
    }
}

impl TransferQueueCore {
    pub fn new(config: TransferQueueConfig) -> Self {
        Self {
            config,
            entries: BTreeMap::new(),
            waiting: VecDeque::new(),
        }
    }

    pub fn config(&self) -> TransferQueueConfig {
        self.config
    }

    pub fn enqueue(&mut self, job_id: impl Into<String>) -> Result<(), QueueError> {
        let job_id = job_id.into();
        if self.entries.contains_key(&job_id) {
            return Err(QueueError::DuplicateJob(job_id));
        }
        self.waiting.push_back(job_id.clone());
        self.entries.insert(job_id, QueueEntry::new());
        Ok(())
    }

    pub fn state(&self, job_id: &str) -> Option<SchedulerState> {
        self.entries.get(job_id).map(|entry| entry.state)
    }

    pub fn attempts_started(&self, job_id: &str) -> Option<u8> {
        self.entries.get(job_id).map(|entry| entry.attempts_started)
    }

    pub fn active_count(&self) -> usize {
        self.entries
            .values()
            .filter(|entry| {
                matches!(
                    entry.state,
                    SchedulerState::Active
                        | SchedulerState::PausePending
                        | SchedulerState::Cancelling
                )
            })
            .count()
    }

    pub fn waiting_count(&self) -> usize {
        self.entries
            .values()
            .filter(|entry| {
                matches!(
                    entry.state,
                    SchedulerState::Waiting | SchedulerState::RetryWaiting
                )
            })
            .count()
    }

    pub fn parked_count(&self) -> usize {
        self.entries
            .values()
            .filter(|entry| entry.state == SchedulerState::Parked)
            .count()
    }

    /// Start the oldest eligible job if a worker slot is available.
    ///
    /// Attempts are counted when execution actually starts, not when a job is
    /// enqueued or enters backoff.
    pub fn next_runnable(&mut self) -> Option<String> {
        if self.active_count() >= self.config.concurrency {
            return None;
        }

        while let Some(job_id) = self.waiting.pop_front() {
            let Some(entry) = self.entries.get_mut(&job_id) else {
                continue;
            };
            if entry.state != SchedulerState::Waiting {
                continue;
            }
            entry.state = SchedulerState::Active;
            if entry.start_new_attempt {
                entry.attempts_started = entry.attempts_started.saturating_add(1);
                entry.start_new_attempt = false;
            }
            return Some(job_id);
        }

        None
    }

    pub fn request_cancel(&mut self, job_id: &str) -> Result<CancelAction, QueueError> {
        let entry = self.entry_mut(job_id)?;
        match entry.state {
            SchedulerState::Waiting
            | SchedulerState::RetryWaiting
            | SchedulerState::Parked => {
                entry.state = SchedulerState::Finished;
                Ok(CancelAction::TerminalizeWithoutExecution)
            }
            SchedulerState::Active | SchedulerState::PausePending => {
                entry.state = SchedulerState::Cancelling;
                Ok(CancelAction::SignalActiveExecution)
            }
            SchedulerState::Cancelling => Ok(CancelAction::SignalActiveExecution),
            SchedulerState::Finished => Ok(CancelAction::AlreadyFinished),
        }
    }

    pub fn request_pause(&mut self, job_id: &str) -> Result<PauseAction, QueueError> {
        let entry = self.entry_mut(job_id)?;
        match entry.state {
            SchedulerState::Active => {
                entry.state = SchedulerState::PausePending;
                Ok(PauseAction::AwaitSafeCheckpoint)
            }
            SchedulerState::PausePending => Ok(PauseAction::AwaitSafeCheckpoint),
            SchedulerState::Waiting | SchedulerState::RetryWaiting => {
                entry.state = SchedulerState::Parked;
                Ok(PauseAction::ParkedBeforeExecution)
            }
            SchedulerState::Parked => Ok(PauseAction::AlreadyParked),
            SchedulerState::Cancelling => Err(QueueError::InvalidTransition {
                job_id: job_id.to_string(),
                from: SchedulerState::Cancelling,
                action: "pause",
            }),
            SchedulerState::Finished => Ok(PauseAction::AlreadyFinished),
        }
    }

    /// Confirm that active I/O has reached a safe pause checkpoint.
    ///
    /// A caller must not invoke this merely because a pause was requested.
    pub fn confirm_paused(&mut self, job_id: &str) -> Result<(), QueueError> {
        let entry = self.entry_mut(job_id)?;
        if entry.state != SchedulerState::PausePending {
            return Err(QueueError::InvalidTransition {
                job_id: job_id.to_string(),
                from: entry.state,
                action: "confirm pause",
            });
        }
        entry.state = SchedulerState::Parked;
        Ok(())
    }

    /// Re-enter scheduler control with the same JobId.
    pub fn resume(&mut self, job_id: &str) -> Result<(), QueueError> {
        let entry = self.entry_mut(job_id)?;
        if entry.state != SchedulerState::Parked {
            return Err(QueueError::InvalidTransition {
                job_id: job_id.to_string(),
                from: entry.state,
                action: "resume",
            });
        }
        entry.state = SchedulerState::Waiting;
        // `start_new_attempt` intentionally remains unchanged:
        // - pause of active work -> false (resume same attempt)
        // - pause before first/retry execution -> true (attempt not started yet)
        self.waiting.push_back(job_id.to_string());
        Ok(())
    }

    /// Mark a successful or cancelled/failed non-retryable execution terminal
    /// from the scheduler's perspective.
    pub fn finish(&mut self, job_id: &str) -> Result<bool, QueueError> {
        let entry = self.entry_mut(job_id)?;
        if entry.state == SchedulerState::Finished {
            return Ok(false);
        }
        entry.state = SchedulerState::Finished;
        Ok(true)
    }

    /// Apply typed retry policy after one active attempt failed.
    ///
    /// This method never schedules the retry immediately. The caller must wait
    /// for bounded backoff and then call `release_retry`.
    pub fn failure(
        &mut self,
        job_id: &str,
        disposition: RetryDisposition,
    ) -> Result<RetryDecision, QueueError> {
        let max_total_attempts = self.config.max_total_attempts;
        let entry = self.entry_mut(job_id)?;
        if !matches!(
            entry.state,
            SchedulerState::Active | SchedulerState::PausePending
        ) {
            return Err(QueueError::InvalidTransition {
                job_id: job_id.to_string(),
                from: entry.state,
                action: "classify failed attempt",
            });
        }

        if disposition == RetryDisposition::SafeToRetry
            && entry.attempts_started < max_total_attempts
        {
            let next_attempt = entry.attempts_started + 1;
            entry.state = SchedulerState::RetryWaiting;
            Ok(RetryDecision::RetryAfterBackoff {
                next_attempt,
                max_total_attempts,
            })
        } else {
            entry.state = SchedulerState::Finished;
            Ok(RetryDecision::Stop {
                disposition,
                attempts_started: entry.attempts_started,
            })
        }
    }

    /// Make a safely retryable job eligible after the backoff timer fires.
    pub fn release_retry(&mut self, job_id: &str) -> Result<(), QueueError> {
        let entry = self.entry_mut(job_id)?;
        if entry.state != SchedulerState::RetryWaiting {
            return Err(QueueError::InvalidTransition {
                job_id: job_id.to_string(),
                from: entry.state,
                action: "release retry",
            });
        }
        entry.state = SchedulerState::Waiting;
        entry.start_new_attempt = true;
        self.waiting.push_back(job_id.to_string());
        Ok(())
    }

    /// Forget a scheduler-finished job after JobManager/UI has decided to clear
    /// the corresponding terminal record.
    pub fn clear_finished(&mut self, job_id: &str) -> Result<bool, QueueError> {
        let Some(entry) = self.entries.get(job_id) else {
            return Err(QueueError::UnknownJob(job_id.to_string()));
        };
        if entry.state != SchedulerState::Finished {
            return Ok(false);
        }
        self.entries.remove(job_id);
        self.waiting.retain(|queued| queued != job_id);
        Ok(true)
    }

    fn entry_mut(&mut self, job_id: &str) -> Result<&mut QueueEntry, QueueError> {
        self.entries
            .get_mut(job_id)
            .ok_or_else(|| QueueError::UnknownJob(job_id.to_string()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressUnit {
    Items,
    Bytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedTransferProgress {
    Items {
        completed: u64,
        total: Option<u64>,
    },
    Bytes {
        completed: u64,
        total: Option<u64>,
    },
}

impl TypedTransferProgress {
    pub fn unit(self) -> ProgressUnit {
        match self {
            Self::Items { .. } => ProgressUnit::Items,
            Self::Bytes { .. } => ProgressUnit::Bytes,
        }
    }

    pub fn completed(self) -> u64 {
        match self {
            Self::Items { completed, .. } | Self::Bytes { completed, .. } => completed,
        }
    }

    pub fn total(self) -> Option<u64> {
        match self {
            Self::Items { total, .. } | Self::Bytes { total, .. } => total,
        }
    }

    pub fn percent(self) -> Option<u8> {
        let total = self.total()?;
        if total == 0 {
            return None;
        }
        Some(
            (((self.completed() as u128) * 100 / (total as u128)).min(100))
                .try_into()
                .unwrap_or(100),
        )
    }
}

/// Enforces cumulative progress monotonicity and stable units within one
/// attempt. Call `reset_attempt` before a safe retry begins.
#[derive(Debug, Default, Clone)]
pub struct TransferProgressTracker {
    unit: Option<ProgressUnit>,
    completed: u64,
}

impl TransferProgressTracker {
    pub fn observe(&mut self, progress: TypedTransferProgress) -> Result<(), QueueError> {
        if let Some(unit) = self.unit
            && unit != progress.unit()
        {
            return Err(QueueError::ProgressUnitChanged);
        }
        if progress.completed() < self.completed {
            return Err(QueueError::ProgressRegressed {
                previous: self.completed,
                current: progress.completed(),
            });
        }
        self.unit = Some(progress.unit());
        self.completed = progress.completed();
        Ok(())
    }

    pub fn reset_attempt(&mut self) {
        self.unit = None;
        self.completed = 0;
    }
}

/// Monotonic recent-window byte rate calculator.
///
/// It accepts cumulative byte counters and an injected `Instant`, making tests
/// deterministic without sleeping.
#[derive(Debug, Clone)]
pub struct TransferRateEstimator {
    window: Duration,
    samples: VecDeque<(Instant, u64)>,
}

impl Default for TransferRateEstimator {
    fn default() -> Self {
        Self::new(Duration::from_secs(2))
    }
}

impl TransferRateEstimator {
    pub fn new(window: Duration) -> Self {
        Self {
            window: window.max(Duration::from_millis(1)),
            samples: VecDeque::new(),
        }
    }

    pub fn observe_at(&mut self, now: Instant, completed_bytes: u64) -> Result<u64, QueueError> {
        if let Some((last_time, last_completed)) = self.samples.back().copied() {
            if now < last_time {
                return Err(QueueError::TimeRegressed);
            }
            if completed_bytes < last_completed {
                return Err(QueueError::ProgressRegressed {
                    previous: last_completed,
                    current: completed_bytes,
                });
            }
        }

        self.samples.push_back((now, completed_bytes));

        while self.samples.len() > 2 {
            let Some((front_time, _)) = self.samples.front().copied() else {
                break;
            };
            if now.duration_since(front_time) <= self.window {
                break;
            }
            self.samples.pop_front();
        }

        Ok(self.rate())
    }

    pub fn rate(&self) -> u64 {
        let Some((first_time, first_bytes)) = self.samples.front().copied() else {
            return 0;
        };
        let Some((last_time, last_bytes)) = self.samples.back().copied() else {
            return 0;
        };
        let elapsed = last_time.duration_since(first_time);
        if elapsed.is_zero() {
            return 0;
        }

        let delta = last_bytes.saturating_sub(first_bytes) as u128;
        let nanos = elapsed.as_nanos();
        if nanos == 0 {
            return 0;
        }

        ((delta * 1_000_000_000u128) / nanos)
            .min(u64::MAX as u128)
            .try_into()
            .unwrap_or(u64::MAX)
    }

    pub fn eta(&self, completed_bytes: u64, total_bytes: Option<u64>) -> Option<Duration> {
        let total = total_bytes?;
        let remaining = total.checked_sub(completed_bytes)?;
        if remaining == 0 {
            return Some(Duration::ZERO);
        }
        let rate = self.rate();
        if rate == 0 {
            return None;
        }
        Some(Duration::from_secs(remaining.div_ceil(rate)))
    }

    pub fn reset_attempt(&mut self) {
        self.samples.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_concurrency_is_two_and_bounded() {
        assert_eq!(
            TransferQueueConfig::default().concurrency(),
            DEFAULT_TRANSFER_CONCURRENCY
        );
        assert!(TransferQueueConfig::new(0).is_err());
        assert!(TransferQueueConfig::new(MAX_TRANSFER_CONCURRENCY + 1).is_err());
        assert_eq!(
            TransferQueueConfig::new(MAX_TRANSFER_CONCURRENCY)
                .unwrap()
                .concurrency(),
            MAX_TRANSFER_CONCURRENCY
        );
    }

    #[test]
    fn fifo_and_concurrency_two_are_enforced() {
        let mut queue = TransferQueueCore::default();
        for id in ["a", "b", "c"] {
            queue.enqueue(id).unwrap();
        }

        assert_eq!(queue.next_runnable().as_deref(), Some("a"));
        assert_eq!(queue.next_runnable().as_deref(), Some("b"));
        assert_eq!(queue.active_count(), 2);
        assert!(queue.next_runnable().is_none());

        assert!(queue.finish("a").unwrap());
        assert_eq!(queue.next_runnable().as_deref(), Some("c"));
        assert_eq!(queue.active_count(), 2);
    }

    #[test]
    fn queued_cancel_never_starts() {
        let mut queue = TransferQueueCore::default();
        queue.enqueue("a").unwrap();
        queue.enqueue("b").unwrap();

        assert_eq!(
            queue.request_cancel("a").unwrap(),
            CancelAction::TerminalizeWithoutExecution
        );
        assert_eq!(queue.next_runnable().as_deref(), Some("b"));
        assert_eq!(queue.attempts_started("a"), Some(0));
    }

    #[test]
    fn active_cancel_releases_only_after_terminal_confirmation() {
        let mut queue = TransferQueueCore::default();
        queue.enqueue("a").unwrap();
        queue.enqueue("b").unwrap();
        queue.enqueue("c").unwrap();
        assert_eq!(queue.next_runnable().as_deref(), Some("a"));
        assert_eq!(queue.next_runnable().as_deref(), Some("b"));

        assert_eq!(
            queue.request_cancel("a").unwrap(),
            CancelAction::SignalActiveExecution
        );
        assert!(queue.next_runnable().is_none());
        assert!(queue.finish("a").unwrap());
        assert_eq!(queue.next_runnable().as_deref(), Some("c"));
        assert_eq!(queue.state("b"), Some(SchedulerState::Active));
    }

    #[test]
    fn pause_is_pending_until_safe_checkpoint_then_releases_slot() {
        let mut queue = TransferQueueCore::new(TransferQueueConfig::new(1).unwrap());
        queue.enqueue("a").unwrap();
        queue.enqueue("b").unwrap();
        assert_eq!(queue.next_runnable().as_deref(), Some("a"));

        assert_eq!(
            queue.request_pause("a").unwrap(),
            PauseAction::AwaitSafeCheckpoint
        );
        assert_eq!(queue.state("a"), Some(SchedulerState::PausePending));
        assert!(queue.next_runnable().is_none());

        queue.confirm_paused("a").unwrap();
        assert_eq!(queue.state("a"), Some(SchedulerState::Parked));
        assert_eq!(queue.next_runnable().as_deref(), Some("b"));
    }

    #[test]
    fn resume_keeps_same_job_id_and_attempt() {
        let mut queue = TransferQueueCore::new(TransferQueueConfig::new(1).unwrap());
        queue.enqueue("same").unwrap();
        assert_eq!(queue.next_runnable().as_deref(), Some("same"));
        queue.request_pause("same").unwrap();
        queue.confirm_paused("same").unwrap();
        queue.resume("same").unwrap();

        assert_eq!(queue.next_runnable().as_deref(), Some("same"));
        assert_eq!(queue.attempts_started("same"), Some(1));
    }

    #[test]
    fn safe_retry_is_bounded_to_three_total_attempts() {
        let mut queue = TransferQueueCore::new(TransferQueueConfig::new(1).unwrap());
        queue.enqueue("a").unwrap();

        for expected_attempt in 1..=MAX_TOTAL_TRANSFER_ATTEMPTS {
            assert_eq!(queue.next_runnable().as_deref(), Some("a"));
            assert_eq!(queue.attempts_started("a"), Some(expected_attempt));

            let decision = queue.failure("a", RetryDisposition::SafeToRetry).unwrap();
            if expected_attempt < MAX_TOTAL_TRANSFER_ATTEMPTS {
                assert_eq!(
                    decision,
                    RetryDecision::RetryAfterBackoff {
                        next_attempt: expected_attempt + 1,
                        max_total_attempts: MAX_TOTAL_TRANSFER_ATTEMPTS,
                    }
                );
                queue.release_retry("a").unwrap();
            } else {
                assert_eq!(
                    decision,
                    RetryDecision::Stop {
                        disposition: RetryDisposition::SafeToRetry,
                        attempts_started: MAX_TOTAL_TRANSFER_ATTEMPTS,
                    }
                );
            }
        }

        assert_eq!(queue.state("a"), Some(SchedulerState::Finished));
        assert!(queue.next_runnable().is_none());
    }

    #[test]
    fn ambiguous_mutation_is_one_shot() {
        let mut queue = TransferQueueCore::new(TransferQueueConfig::new(1).unwrap());
        queue.enqueue("a").unwrap();
        assert_eq!(queue.next_runnable().as_deref(), Some("a"));

        assert_eq!(
            queue.failure("a", RetryDisposition::AmbiguousMutation)
                .unwrap(),
            RetryDecision::Stop {
                disposition: RetryDisposition::AmbiguousMutation,
                attempts_started: 1,
            }
        );
        assert_eq!(queue.attempts_started("a"), Some(1));
        assert!(queue.next_runnable().is_none());
    }

    #[test]
    fn recovery_required_is_never_retried() {
        let mut queue = TransferQueueCore::new(TransferQueueConfig::new(1).unwrap());
        queue.enqueue("a").unwrap();
        queue.next_runnable();

        assert_eq!(
            queue.failure("a", RetryDisposition::RecoveryRequired)
                .unwrap(),
            RetryDecision::Stop {
                disposition: RetryDisposition::RecoveryRequired,
                attempts_started: 1,
            }
        );
    }

    #[test]
    fn unclassified_failure_defaults_to_no_retry() {
        assert_eq!(RetryDisposition::default(), RetryDisposition::NeverRetry);

        let mut queue = TransferQueueCore::new(TransferQueueConfig::new(1).unwrap());
        queue.enqueue("a").unwrap();
        queue.next_runnable();
        assert_eq!(
            queue.failure("a", RetryDisposition::default()).unwrap(),
            RetryDecision::Stop {
                disposition: RetryDisposition::NeverRetry,
                attempts_started: 1,
            }
        );
    }

    #[test]
    fn finished_scheduler_entry_is_immutable_except_clear() {
        let mut queue = TransferQueueCore::default();
        queue.enqueue("a").unwrap();
        queue.next_runnable();
        assert!(queue.finish("a").unwrap());
        assert!(!queue.finish("a").unwrap());
        assert_eq!(
            queue.request_cancel("a").unwrap(),
            CancelAction::AlreadyFinished
        );
        assert!(queue.clear_finished("a").unwrap());
        assert_eq!(queue.state("a"), None);
    }

    #[test]
    fn typed_progress_rejects_regression_and_unit_switch() {
        let mut tracker = TransferProgressTracker::default();
        tracker
            .observe(TypedTransferProgress::Bytes {
                completed: 10,
                total: Some(100),
            })
            .unwrap();
        tracker
            .observe(TypedTransferProgress::Bytes {
                completed: 20,
                total: Some(100),
            })
            .unwrap();

        assert!(matches!(
            tracker.observe(TypedTransferProgress::Bytes {
                completed: 19,
                total: Some(100),
            }),
            Err(QueueError::ProgressRegressed { .. })
        ));
        assert_eq!(
            tracker.observe(TypedTransferProgress::Items {
                completed: 20,
                total: Some(100),
            }),
            Err(QueueError::ProgressUnitChanged)
        );
    }

    #[test]
    fn typed_progress_percent_requires_known_nonzero_total() {
        assert_eq!(
            TypedTransferProgress::Bytes {
                completed: 50,
                total: Some(100)
            }
            .percent(),
            Some(50)
        );
        assert_eq!(
            TypedTransferProgress::Bytes {
                completed: 50,
                total: None
            }
            .percent(),
            None
        );
        assert_eq!(
            TypedTransferProgress::Items {
                completed: 0,
                total: Some(0)
            }
            .percent(),
            None
        );
    }

    #[test]
    fn rate_and_eta_use_injected_monotonic_time() {
        let start = Instant::now();
        let mut rate = TransferRateEstimator::new(Duration::from_secs(5));
        assert_eq!(rate.observe_at(start, 0).unwrap(), 0);
        assert_eq!(
            rate.observe_at(start + Duration::from_secs(1), 1_000)
                .unwrap(),
            1_000
        );
        assert_eq!(rate.eta(1_000, Some(3_000)), Some(Duration::from_secs(2)));
        assert_eq!(rate.eta(1_000, None), None);
    }

    #[test]
    fn rate_rejects_progress_or_time_regression() {
        let start = Instant::now();
        let mut rate = TransferRateEstimator::default();
        rate.observe_at(start, 100).unwrap();

        assert!(matches!(
            rate.observe_at(start + Duration::from_secs(1), 99),
            Err(QueueError::ProgressRegressed { .. })
        ));
        assert_eq!(
            rate.observe_at(start - Duration::from_millis(1), 100),
            Err(QueueError::TimeRegressed)
        );
    }
}
