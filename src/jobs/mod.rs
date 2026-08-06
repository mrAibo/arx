use crate::vfs::Location;

#[derive(Debug, Clone)]
pub struct Job {
    pub id: String,
    pub description: String,
    pub source: Location,
    pub destination: Location,
    pub status: JobStatus,
    pub progress: u8, // 0-100 percent
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Pending,
    Running,
    Done,
    Failed,
}

/// Background thread completion notification.
#[derive(Debug, Clone)]
pub enum JobEvent {
    Running { id: String },
    Done { id: String, message: String },
    Failed { id: String, error: String },
}

impl Job {
    pub fn status_icon(&self) -> &str {
        match self.status {
            JobStatus::Pending => "⏳",
            JobStatus::Running => "⚡",
            JobStatus::Done => "✅",
            JobStatus::Failed => "❌",
        }
    }
}

impl std::fmt::Display for Job {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let pct = if self.status == JobStatus::Running {
            format!("{}%", self.progress)
        } else {
            String::new()
        };
        write!(f, "{} {} {}", self.status_icon(), self.description, pct)
    }
}
