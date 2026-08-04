use crate::vfs::Location;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferMethod {
    Native,
    Rsync,
    Sftp,
    Scp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferIntent {
    Copy,
    Move,
    Synchronize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferPlan {
    pub source: Location,
    pub destination: Location,
    pub intent: TransferIntent,
    pub method: TransferMethod,
}
