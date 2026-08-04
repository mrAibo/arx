use crate::vfs::Location;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExecutorId(String);

impl ExecutorId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferOperation {
    Copy,
    Move,
    Synchronize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OverwritePolicy {
    #[default]
    Ask,
    Never,
    Replace,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum VerificationPolicy {
    #[default]
    Metadata,
    Checksum,
    None,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PreservationPolicy {
    pub permissions: bool,
    pub timestamps: bool,
    pub symlinks: bool,
    pub ownership: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferRequest {
    pub source: Location,
    pub destination: Location,
    pub operation: TransferOperation,
    pub overwrite: OverwritePolicy,
    pub verification: VerificationPolicy,
    pub preservation: PreservationPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferPlan {
    pub request: TransferRequest,
    pub executor: ExecutorId,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::{NamespaceId, VfsPath};

    #[test]
    fn plan_uses_opaque_executor_id() {
        let request = TransferRequest {
            source: Location::new(NamespaceId::new("local"), VfsPath::Native("/tmp/a".into())),
            destination: Location::new(
                NamespaceId::new("host:backup"),
                VfsPath::Bytes(b"/data/a".to_vec()),
            ),
            operation: TransferOperation::Copy,
            overwrite: OverwritePolicy::Ask,
            verification: VerificationPolicy::Metadata,
            preservation: PreservationPolicy::default(),
        };
        let plan = TransferPlan {
            request,
            executor: ExecutorId::new("rsync"),
        };

        assert_eq!(plan.executor.as_str(), "rsync");
    }
}
