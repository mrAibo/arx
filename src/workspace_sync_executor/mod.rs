mod compiler;
mod runtime;

pub use compiler::{SyncCompileError, SyncExecutionCompiler, SyncExecutorMatrix};
pub use runtime::{
    CompletedSyncStep, SyncExecutionError, SyncExecutionEvent, SyncExecutionOutcome, SyncRunError,
    SyncTerminalState, WorkspaceSyncExecutor,
};

use crate::transfer::TransferPlan;
use crate::vfs::Location;
use crate::workspace_sync::WorkspaceFingerprint;
use crate::workspace_sync_execution::{ExecutableSyncPlan, PlanDigest, SyncPlanId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PhysicalStepId(pub usize);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhysicalSyncStep {
    EnsureDirectory {
        relative_path: String,
        target: Location,
        expected_target: Option<WorkspaceFingerprint>,
    },
    TransferFile {
        relative_path: String,
        source: Location,
        destination: Location,
        expected_source: WorkspaceFingerprint,
        expected_destination: Option<WorkspaceFingerprint>,
        transfer: Box<TransferPlan>,
        name: String,
        bytes: u64,
    },
    DeleteFile {
        relative_path: String,
        target: Location,
        expected_target: WorkspaceFingerprint,
    },
    RemoveDirectory {
        relative_path: String,
        target: Location,
        expected_target: WorkspaceFingerprint,
    },
}

impl PhysicalSyncStep {
    pub fn relative_path(&self) -> &str {
        match self {
            Self::EnsureDirectory { relative_path, .. }
            | Self::TransferFile { relative_path, .. }
            | Self::DeleteFile { relative_path, .. }
            | Self::RemoveDirectory { relative_path, .. } => relative_path,
        }
    }

    pub fn is_destructive(&self) -> bool {
        matches!(self, Self::DeleteFile { .. } | Self::RemoveDirectory { .. })
    }

    pub fn bytes(&self) -> u64 {
        match self {
            Self::TransferFile { bytes, .. } => *bytes,
            _ => 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledSyncStep {
    pub id: PhysicalStepId,
    pub step: PhysicalSyncStep,
}

#[derive(Debug, Clone)]
pub struct CompiledSyncPlan {
    executable: ExecutableSyncPlan,
    steps: Vec<CompiledSyncStep>,
    total_bytes: u64,
    destructive_steps: usize,
}

impl CompiledSyncPlan {
    pub(crate) fn new(
        executable: ExecutableSyncPlan,
        steps: Vec<CompiledSyncStep>,
        total_bytes: u64,
        destructive_steps: usize,
    ) -> Self {
        Self {
            executable,
            steps,
            total_bytes,
            destructive_steps,
        }
    }

    pub fn plan_id(&self) -> SyncPlanId {
        self.executable.plan().id()
    }

    pub fn digest(&self) -> PlanDigest {
        self.executable.plan().digest()
    }

    pub fn left_root(&self) -> &Location {
        self.executable.plan().left_root()
    }

    pub fn right_root(&self) -> &Location {
        self.executable.plan().right_root()
    }

    pub fn steps(&self) -> &[CompiledSyncStep] {
        &self.steps
    }

    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    pub fn destructive_steps(&self) -> usize {
        self.destructive_steps
    }

    pub(crate) fn executable(&self) -> &ExecutableSyncPlan {
        &self.executable
    }
}
