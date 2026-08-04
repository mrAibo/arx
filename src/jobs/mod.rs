use crate::vfs::Location;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Queued,
    Starting,
    Running,
    Cancelling,
    Cancelled,
    Failed,
    Finished,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobKind {
    Copy,
    Move,
    Sync,
    Archive,
    Search,
    RemoteCommand,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    pub completed_bytes: u64,
    pub total_bytes: Option<u64>,
    pub bytes_per_second: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub id: u64,
    pub kind: JobKind,
    pub state: JobState,
    pub source: Location,
    pub destination: Option<Location>,
    pub progress: Option<Progress>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum JobEvent {
    Started(u64),
    Progress(u64, Progress),
    Output(u64, String),
    Warning(u64, String),
    Finished(u64),
    Failed(u64, String),
    Cancelled(u64),
}
