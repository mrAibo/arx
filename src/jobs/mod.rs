use crate::vfs::Location;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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
                Some(((done * 100) / total).min(100) as u8)
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
}

impl Job {
    pub fn status_icon(&self) -> &str {
        match self.status {
            JobStatus::Pending => "⏳",
            JobStatus::Running => "⚡",
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
                let remaining = total - done;
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
        if self.status == JobStatus::Running || self.status == JobStatus::Paused {
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
