use crate::vfs::Location;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

// ── Progress model v2 ──
// ponytail: enum replaces u8; Bytes/Items/Phase cover transfer jobs.
// Indeterminate covers search/checksum. From<u8> keeps old call sites working.

#[derive(Debug, Clone)]
pub enum Progress {
    Indeterminate,
    Percent(u8),
    Bytes {
        done: u64,
        total: u64,
        /// Transfer rate in bytes/sec (last-second sample)
        rate: u64,
    },
    Items {
        done: usize,
        total: usize,
    },
    Phase {
        phase: String,
        /// Sub-progress within the phase (e.g. "transferring" → 45%)
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

#[allow(clippy::derivable_impls)]
impl From<u8> for Progress {
    fn from(p: u8) -> Self {
        Self::Percent(p)
    }
}

#[allow(clippy::derivable_impls)]
impl Default for Progress {
    fn default() -> Self {
        Self::Indeterminate
    }
}

impl std::fmt::Display for Progress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Indeterminate => write!(f, "…"),
            Self::Percent(p) => write!(f, "{}%", p),
            Self::Bytes { done, total, rate } => {
                let dp = human_bytes(*done);
                let tp = human_bytes(*total);
                let rs = human_bytes(*rate);
                write!(f, "{dp}/{tp} ({rs}/s)")
            }
            Self::Items { done, total } => write!(f, "{}/{}", done, total),
            Self::Phase { phase, percent } => match percent {
                Some(p) => write!(f, "{} {}%", phase, p),
                None => write!(f, "{}", phase),
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

// ── Job model ──

#[derive(Debug, Clone)]
pub struct Job {
    pub id: String,
    pub description: String,
    pub kind: JobKind,
    pub status: JobStatus,
    pub progress: Progress,
    pub source: Option<Location>,
    pub destination: Option<Location>,
    /// Cancellation flag — set to true to abort the job.
    pub cancel: Arc<AtomicBool>,
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
    Custom(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Pending,
    Running,
    Cancelling,
    Paused,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone)]
pub enum JobEvent {
    Running { id: String },
    Progress { id: String, progress: Progress },
    Done { id: String, message: String },
    Failed { id: String, error: String },
    Paused { id: String },
    Cancelled { id: String, completed: usize },
}

impl Job {
    pub fn status_icon(&self) -> &str {
        match self.status {
            JobStatus::Pending => "⏳",
            JobStatus::Running => "⚡",
            JobStatus::Cancelling => "⏹",
            JobStatus::Paused => "⏸",
            JobStatus::Done => "✅",
            JobStatus::Failed => "❌",
            JobStatus::Cancelled => "🚫",
        }
    }

    /// ETA string from progress
    pub fn eta(&self) -> Option<String> {
        match &self.progress {
            Progress::Bytes { done, total, rate } if *rate > 0 && *total > 0 => {
                let remaining = total.saturating_sub(*done);
                let secs = remaining / rate;
                if secs < 60 {
                    Some(format!("{}s", secs))
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

/// Create a cancellation flag for a job.
pub fn job_token() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

// ── Job Queue / Transfer Manager ──
// ponytail: simple FIFO queue with N parallel workers + ETA

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Instant;

#[derive(Default)]
pub struct JobQueue {
    pending: Mutex<VecDeque<Job>>,
    active: Mutex<Vec<Job>>,
    max_workers: usize,
    done: Mutex<Vec<Job>>,
}

impl JobQueue {
    pub fn new(max_workers: usize) -> Self {
        Self {
            max_workers,
            ..Default::default()
        }
    }

    /// Add a job to the pending queue.
    pub fn enqueue(&self, job: Job) {
        self.pending.lock().unwrap().push_back(job);
    }

    /// Cancel a pending or active job by id. Returns true if found.
    pub fn cancel(&self, id: &str) -> bool {
        // Pending work has not started, so cancellation is immediately
        // terminal and the job must never be promoted to a worker.
        let mut pending = self.pending.lock().unwrap();
        if let Some(pos) = pending.iter().position(|job| job.id == id) {
            let mut job = pending.remove(pos).expect("pending position disappeared");
            job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            job.status = JobStatus::Cancelled;
            drop(pending);
            self.done.lock().unwrap().push(job);
            return true;
        }
        drop(pending);

        // Cancel in active
        if let Some(job) = self
            .active
            .lock()
            .unwrap()
            .iter_mut()
            .find(|job| job.id == id)
        {
            job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            if job.status == JobStatus::Running {
                job.status = JobStatus::Cancelling;
            }
            return true;
        }

        // Idempotent cancellation: cancelling an already-cancelled job is a
        // successful no-op.
        if self
            .done
            .lock()
            .unwrap()
            .iter()
            .any(|job| job.id == id && job.status == JobStatus::Cancelled)
        {
            return true;
        }
        false
    }

    /// Take up to N pending jobs and promote them to active.
    /// Returns the jobs that were promoted (caller spawns workers).
    pub fn promote(&self) -> Vec<Job> {
        let mut active = self.active.lock().unwrap();
        let available = self.max_workers.saturating_sub(active.len());
        if available == 0 {
            return vec![];
        }
        let mut pending = self.pending.lock().unwrap();
        let mut promoted = Vec::new();
        for _ in 0..available {
            while let Some(mut job) = pending.pop_front() {
                if job.cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    job.status = JobStatus::Cancelled;
                    self.done.lock().unwrap().push(job);
                    continue;
                }
                job.status = JobStatus::Running;
                promoted.push(job.clone());
                active.push(job);
                break;
            }
        }
        promoted
    }

    /// Move a completed job from active to done.
    pub fn complete(&self, id: &str) {
        let mut active = self.active.lock().unwrap();
        if let Some(pos) = active.iter().position(|j| j.id == id) {
            let mut job = active.remove(pos);
            if job.cancel.load(std::sync::atomic::Ordering::Relaxed) {
                job.status = JobStatus::Cancelled;
            } else {
                job.status = JobStatus::Done;
            }
            self.done.lock().unwrap().push(job);
        }
    }

    pub fn active_count(&self) -> usize {
        self.active.lock().unwrap().len()
    }

    pub fn pending_count(&self) -> usize {
        self.pending.lock().unwrap().len()
    }

    /// Snapshot of all job statuses (for UI).
    pub fn snapshot(&self) -> Vec<Job> {
        let mut all = self
            .pending
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        all.extend(self.active.lock().unwrap().iter().cloned());
        all.extend(self.done.lock().unwrap().iter().cloned());
        all
    }
}
#[cfg(test)]
mod queue_tests {
    use super::*;

    fn job(id: &str) -> Job {
        Job {
            id: id.into(),
            description: id.into(),
            kind: JobKind::Transfer,
            status: JobStatus::Pending,
            progress: Progress::Indeterminate,
            source: None,
            destination: None,
            cancel: job_token(),
        }
    }

    #[test]
    fn pending_cancel_never_promotes_job() {
        let queue = JobQueue::new(1);
        queue.enqueue(job("a"));
        assert!(queue.cancel("a"));
        assert!(queue.promote().is_empty());
        let snapshot = queue.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].status, JobStatus::Cancelled);
    }

    #[test]
    fn active_cancel_sets_shared_token_and_cancelling_state() {
        let queue = JobQueue::new(1);
        let original = job("a");
        let token = original.cancel.clone();
        queue.enqueue(original);
        let promoted = queue.promote();
        assert_eq!(promoted.len(), 1);

        assert!(queue.cancel("a"));
        assert!(token.load(std::sync::atomic::Ordering::Relaxed));
        assert_eq!(queue.snapshot()[0].status, JobStatus::Cancelling);
    }

    #[test]
    fn cancel_is_idempotent_after_terminal_cancel() {
        let queue = JobQueue::new(1);
        queue.enqueue(job("a"));
        assert!(queue.cancel("a"));
        assert!(queue.cancel("a"));
    }
}

#[derive(Debug, Clone, Default)]
pub struct TransferStats {
    pub bytes_total: u64,
    pub bytes_done: u64,
    pub started: Option<Instant>,
    pub speed: f64, // bytes/sec
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

    /// ETA in seconds — simple: remaining_bytes / speed.
    /// ponytail: no EWMA; single-sample speed from TransferStats.
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
