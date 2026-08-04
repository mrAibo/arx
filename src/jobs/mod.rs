#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JobId(u64);

impl JobId {
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

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
    Transfer,
    Synchronize,
    Archive,
    Search,
    RemoteCommand,
    CapabilityProbe,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobSpec {
    pub kind: JobKind,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Progress {
    Indeterminate,
    Bytes {
        completed: u64,
        total: Option<u64>,
        bytes_per_second: Option<f64>,
    },
    Items {
        completed: u64,
        total: Option<u64>,
    },
    Phase {
        name: String,
        completed: u64,
        total: Option<u64>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct JobRecord {
    pub id: JobId,
    pub spec: JobSpec,
    pub state: JobState,
    pub progress: Progress,
}

#[derive(Debug, Clone, PartialEq)]
pub enum JobEvent {
    Started(JobId),
    Progress(JobId, Progress),
    Output(JobId, String),
    Warning(JobId, String),
    Finished(JobId),
    Failed(JobId, String),
    Cancelled(JobId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobCommand {
    Cancel(JobId),
    Retry(JobId),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_job_does_not_require_transfer_locations() {
        let job = JobRecord {
            id: JobId::new(7),
            spec: JobSpec {
                kind: JobKind::RemoteCommand,
                label: "uptime on prod-db".into(),
            },
            state: JobState::Queued,
            progress: Progress::Indeterminate,
        };

        assert_eq!(job.id.get(), 7);
        assert_eq!(job.state, JobState::Queued);
    }
}
