use std::collections::BTreeSet;
use std::io;

use tokio::sync::mpsc;

use crate::jobs::{JobEvent, JobManager};
use crate::journal::OperationJournal;
use crate::transfer::probe::{
    detect_local_tools, detect_remote_tools, local_remote_executors,
};
use crate::vfs::{Location, ProviderRegistry};
use crate::workspace_sync::{WorkspaceDiff, WorkspaceSyncPlan};
use crate::workspace_sync_execution::{
    ExecutableSyncPlan, FrozenWorkspaceSyncPlan, SyncConfirmationToken, SyncExecutionGateError,
    SyncPlanValidator, SyncValidationError,
};
use crate::workspace_sync_executor::{
    SyncCompileError, SyncExecutionCompiler, SyncExecutorMatrix, WorkspaceSyncExecutor,
};
use crate::workspace_sync_verification::{
    SyncVerificationCoordinator, SyncVerificationEvent,
};

/// Application-level wiring for the existing workspace-sync pipeline.
///
/// Presentation code may ask this controller to freeze or launch a plan, but it
/// never selects transports or performs mutations itself. The existing
/// validator/compiler/executor layers remain the source of those decisions.
#[derive(Clone)]
pub struct WorkspaceSyncController {
    registry: ProviderRegistry,
    journal: Option<OperationJournal>,
    verification: SyncVerificationCoordinator,
}

impl WorkspaceSyncController {
    pub fn new(registry: ProviderRegistry) -> Self {
        Self {
            verification: SyncVerificationCoordinator::new(registry.clone()),
            registry,
            journal: None,
        }
    }

    pub fn with_journal(registry: ProviderRegistry, journal: OperationJournal) -> Self {
        Self {
            verification: SyncVerificationCoordinator::new(registry.clone()),
            registry,
            journal: Some(journal),
        }
    }

    pub fn freeze(
        &self,
        plan: &WorkspaceSyncPlan,
        current: &WorkspaceDiff,
    ) -> Result<FrozenWorkspaceSyncPlan, SyncValidationError> {
        SyncPlanValidator::freeze(plan, current, &self.registry)
    }

    pub async fn launch(
        &self,
        frozen: FrozenWorkspaceSyncPlan,
        current: WorkspaceDiff,
        explicitly_confirmed: bool,
        jobs: JobManager,
        job_events: mpsc::UnboundedSender<JobEvent>,
        verification_events: mpsc::UnboundedSender<SyncVerificationEvent>,
    ) -> Result<String, WorkspaceSyncLaunchError> {
        // A confirmation is permission for this exact frozen plan, not a waiver
        // of stale-preview checks.
        SyncPlanValidator::validate_frozen(&frozen, &current)?;

        let confirmation = if frozen.requires_confirmation() {
            if !explicitly_confirmed {
                return Err(WorkspaceSyncLaunchError::Gate(
                    SyncExecutionGateError::ConfirmationRequired,
                ));
            }
            Some(SyncConfirmationToken::from_explicit_confirmation(&frozen))
        } else {
            None
        };
        let executable = ExecutableSyncPlan::new(frozen, confirmation)?;
        let executors = self
            .executor_matrix(executable.plan().left_root(), executable.plan().right_root())
            .await?;
        let compiled = SyncExecutionCompiler::compile(
            executable,
            &current,
            &self.registry,
            &executors,
        )?;
        let journal = match &self.journal {
            Some(journal) => journal.clone(),
            None => OperationJournal::open_default()?,
        };
        let executor = WorkspaceSyncExecutor::new(self.registry.clone(), journal);

        Ok(jobs.spawn_workspace_sync_with_verification(
            compiled,
            executor,
            job_events,
            self.verification.clone(),
            verification_events,
        ))
    }

    async fn executor_matrix(
        &self,
        left: &Location,
        right: &Location,
    ) -> Result<SyncExecutorMatrix, WorkspaceSyncLaunchError> {
        let hosts = [left, right]
            .into_iter()
            .filter_map(|location| match location {
                Location::Sftp { host, .. } => Some(host.clone()),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        if hosts.is_empty() {
            return Ok(SyncExecutorMatrix::local_only());
        }

        let local = tokio::task::spawn_blocking(detect_local_tools)
            .await
            .map_err(|error| WorkspaceSyncLaunchError::ProbeWorker(error.to_string()))?;
        let mut matrix = SyncExecutorMatrix::local_only();
        for host in hosts {
            let probe_host = host.clone();
            let remote = tokio::task::spawn_blocking(move || detect_remote_tools(&probe_host))
                .await
                .map_err(|error| WorkspaceSyncLaunchError::ProbeWorker(error.to_string()))?
                .map_err(|error| WorkspaceSyncLaunchError::Probe(error.to_string()))?;
            matrix = matrix.with_remote(host, local_remote_executors(local, remote, true));
        }
        Ok(matrix)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceSyncLaunchError {
    #[error(transparent)]
    Validation(#[from] SyncValidationError),
    #[error(transparent)]
    Gate(#[from] SyncExecutionGateError),
    #[error(transparent)]
    Compile(#[from] SyncCompileError),
    #[error("transfer capability probe failed: {0}")]
    Probe(String),
    #[error("transfer capability worker failed: {0}")]
    ProbeWorker(String),
    #[error("operation journal is unavailable: {0}")]
    Journal(#[from] io::Error),
}

impl WorkspaceSyncLaunchError {
    /// Beginner-facing explanation. All errors returned here happen before a
    /// sync Job is created, so the no-mutation statement is truthful.
    pub fn user_message(&self) -> String {
        let reason = match self {
            Self::Compile(SyncCompileError::RemoteToRemoteUnsupported { .. }) => {
                "Remote → Remote sync is not supported yet.".to_string()
            }
            Self::Compile(SyncCompileError::UnsupportedDirectoryMutation { .. }) => {
                "The destination requires a directory change that this provider cannot perform safely."
                    .to_string()
            }
            Self::Compile(SyncCompileError::UnsupportedFileMutation { .. }) => {
                "The destination requires a file deletion that this provider cannot perform safely."
                    .to_string()
            }
            Self::Compile(SyncCompileError::MissingExecutorAvailability { host }) => {
                format!("ARX could not determine a safe transfer executor for SSH host {host}.")
            }
            Self::Probe(error) | Self::ProbeWorker(error) => {
                format!("ARX could not verify the remote transfer capability: {error}")
            }
            other => other.to_string(),
        };
        format!("Plan cannot be executed\n{reason}\nNo files were changed.")
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::vfs::{EntryKind, default_registry};
    use crate::workspace_sync::{
        SyncMode, SyncPolicy, WorkspaceEntry, WorkspaceFingerprint,
    };

    fn file(path: &str, size: u64) -> WorkspaceEntry {
        WorkspaceEntry {
            relative_path: path.into(),
            fingerprint: WorkspaceFingerprint {
                kind: EntryKind::File,
                size: Some(size),
                modified_unix_ms: None,
                content_hash: None,
            },
        }
    }

    #[tokio::test]
    async fn mirror_requires_explicit_confirmation_before_job_creation() {
        let left = tempfile::tempdir().unwrap();
        let right = tempfile::tempdir().unwrap();
        tokio::fs::write(right.path().join("old.txt"), b"x")
            .await
            .unwrap();
        let diff = WorkspaceDiff::compare(
            Location::Local(left.path().to_path_buf()),
            Location::Local(right.path().to_path_buf()),
            Vec::<WorkspaceEntry>::new(),
            vec![file("old.txt", 1)],
        );
        let plan = WorkspaceSyncPlan::build(
            &diff,
            SyncPolicy {
                mode: SyncMode::Mirror,
                ..SyncPolicy::default()
            },
        );
        let journal_dir = tempfile::tempdir().unwrap();
        let controller = WorkspaceSyncController::with_journal(
            default_registry(),
            OperationJournal::open(journal_dir.path().join("ops.jsonl")).unwrap(),
        );
        let frozen = controller.freeze(&plan, &diff).unwrap();
        let jobs = JobManager::new();
        let (job_tx, _job_rx) = mpsc::unbounded_channel();
        let (verification_tx, _verification_rx) = mpsc::unbounded_channel();

        let error = controller
            .launch(
                frozen,
                diff,
                false,
                jobs.clone(),
                job_tx,
                verification_tx,
            )
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            WorkspaceSyncLaunchError::Gate(SyncExecutionGateError::ConfirmationRequired)
        ));
        assert!(jobs.snapshot().is_empty());
        assert!(right.path().join("old.txt").exists());
    }

    #[tokio::test]
    async fn safe_local_plan_launches_through_job_manager() {
        let left = tempfile::tempdir().unwrap();
        let right = tempfile::tempdir().unwrap();
        tokio::fs::write(left.path().join("a.txt"), b"a")
            .await
            .unwrap();
        let diff = WorkspaceDiff::compare(
            Location::Local(left.path().to_path_buf()),
            Location::Local(right.path().to_path_buf()),
            vec![file("a.txt", 1)],
            Vec::<WorkspaceEntry>::new(),
        );
        let plan = WorkspaceSyncPlan::build(&diff, SyncPolicy::default());
        let journal_dir = tempfile::tempdir().unwrap();
        let controller = WorkspaceSyncController::with_journal(
            default_registry(),
            OperationJournal::open(journal_dir.path().join("ops.jsonl")).unwrap(),
        );
        let frozen = controller.freeze(&plan, &diff).unwrap();
        let jobs = JobManager::new();
        let (job_tx, mut job_rx) = mpsc::unbounded_channel();
        let (verification_tx, _verification_rx) = mpsc::unbounded_channel();

        let id = controller
            .launch(
                frozen,
                diff,
                false,
                jobs.clone(),
                job_tx,
                verification_tx,
            )
            .await
            .unwrap();

        while let Some(event) = job_rx.recv().await {
            if event.is_terminal() {
                break;
            }
        }
        let job = jobs.get(&id).unwrap();
        assert!(job.status.is_terminal());
        assert!(right.path().join("a.txt").exists());
    }

    #[test]
    fn launch_error_message_never_claims_a_mutation() {
        let error = WorkspaceSyncLaunchError::Compile(
            SyncCompileError::RemoteToRemoteUnsupported {
                source_location: Location::Sftp {
                    host: "a".into(),
                    path: "/src".into(),
                },
                destination_location: Location::Sftp {
                    host: "b".into(),
                    path: "/dst".into(),
                },
            },
        );
        let message = error.user_message();
        assert!(message.contains("Remote → Remote sync is not supported yet"));
        assert!(message.contains("No files were changed"));
    }

    #[test]
    fn controller_is_provider_neutral_at_the_api_boundary() {
        let controller = WorkspaceSyncController::new(default_registry());
        let plan = WorkspaceSyncPlan {
            left_root: Location::Local(PathBuf::from("/left")),
            right_root: Location::Local(PathBuf::from("/right")),
            policy: SyncPolicy::default(),
            operations: Vec::new(),
            bytes_to_transfer: 0,
            destructive_operations: 0,
            conflicts: 0,
        };
        let diff = WorkspaceDiff::compare(
            plan.left_root.clone(),
            plan.right_root.clone(),
            Vec::<WorkspaceEntry>::new(),
            Vec::<WorkspaceEntry>::new(),
        );
        assert!(controller.freeze(&plan, &diff).is_err());
    }
}
