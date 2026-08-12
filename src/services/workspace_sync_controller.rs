use std::collections::BTreeSet;
use std::io;
use std::sync::{Arc, Mutex, atomic::AtomicBool};

use tokio::sync::mpsc;

use crate::jobs::{JobEvent, JobManager};
use crate::journal::OperationJournal;
use crate::transfer::probe::{detect_local_tools, detect_remote_tools, local_remote_executors};
use crate::vfs::{Location, ProviderRegistry};
use crate::workspace_sync::{WorkspaceDiff, WorkspaceSyncPlan};
use crate::workspace_sync_execution::{
    ExecutableSyncPlan, FrozenWorkspaceSyncPlan, SyncConfirmationToken, SyncExecutionGateError,
    SyncPlanValidator, SyncValidationError,
};
use crate::workspace_sync_executor::{
    SyncCompileError, SyncExecutionCompiler, SyncExecutorMatrix, WorkspaceSyncExecutor,
};
use crate::workspace_sync_verification::{SyncVerificationCoordinator, SyncVerificationEvent};

use super::{WorkspaceScanError, WorkspaceScanOptions, scan_workspace};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SyncLaunchId(u64);

impl SyncLaunchId {
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Default)]
struct SyncLaunchGeneration {
    current: u64,
    committed: bool,
}

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
    launch_generation: Arc<Mutex<SyncLaunchGeneration>>,
}

impl WorkspaceSyncController {
    pub fn new(registry: ProviderRegistry) -> Self {
        Self {
            verification: SyncVerificationCoordinator::new(registry.clone()),
            registry,
            journal: None,
            launch_generation: Arc::new(Mutex::new(SyncLaunchGeneration::default())),
        }
    }

    pub fn with_journal(registry: ProviderRegistry, journal: OperationJournal) -> Self {
        Self {
            verification: SyncVerificationCoordinator::new(registry.clone()),
            registry,
            journal: Some(journal),
            launch_generation: Arc::new(Mutex::new(SyncLaunchGeneration::default())),
        }
    }

    pub fn freeze(
        &self,
        plan: &WorkspaceSyncPlan,
        current: &WorkspaceDiff,
    ) -> Result<FrozenWorkspaceSyncPlan, SyncValidationError> {
        SyncPlanValidator::freeze(plan, current, &self.registry)
    }

    pub fn begin_launch(&self) -> SyncLaunchId {
        let mut generation = self
            .launch_generation
            .lock()
            .expect("workspace sync launch generation poisoned");
        generation.current = generation
            .current
            .checked_add(1)
            .expect("workspace sync launch generation exhausted");
        generation.committed = false;
        SyncLaunchId(generation.current)
    }

    /// Supersede an in-flight preparation. Returns false once that preparation
    /// has atomically crossed the Job-creation boundary.
    pub fn supersede_launch(&self) -> bool {
        let mut generation = self
            .launch_generation
            .lock()
            .expect("workspace sync launch generation poisoned");
        if generation.committed {
            return false;
        }
        generation.current = generation
            .current
            .checked_add(1)
            .expect("workspace sync launch generation exhausted");
        true
    }

    pub fn is_launch_current(&self, launch_id: SyncLaunchId) -> bool {
        self.launch_generation
            .lock()
            .expect("workspace sync launch generation poisoned")
            .current
            == launch_id.get()
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
        let launch_id = self.begin_launch();
        self.launch_guarded(
            launch_id,
            frozen,
            current,
            explicitly_confirmed,
            jobs,
            (job_events, verification_events),
        )
        .await
    }

    pub async fn launch_guarded(
        &self,
        launch_id: SyncLaunchId,
        frozen: FrozenWorkspaceSyncPlan,
        current: WorkspaceDiff,
        explicitly_confirmed: bool,
        jobs: JobManager,
        events: (
            mpsc::UnboundedSender<JobEvent>,
            mpsc::UnboundedSender<SyncVerificationEvent>,
        ),
    ) -> Result<String, WorkspaceSyncLaunchError> {
        let (job_events, verification_events) = events;
        self.ensure_launch_current(launch_id)?;

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
            .executor_matrix(
                executable.plan().left_root(),
                executable.plan().right_root(),
            )
            .await?;
        self.ensure_launch_current(launch_id)?;
        let fresh = self
            .rescan_current(
                launch_id,
                executable.plan().left_root(),
                executable.plan().right_root(),
            )
            .await?;
        SyncPlanValidator::validate_frozen(executable.plan(), &fresh)?;
        let compiled =
            SyncExecutionCompiler::compile(executable, &fresh, &self.registry, &executors)?;
        self.ensure_launch_current(launch_id)?;
        let journal = match &self.journal {
            Some(journal) => journal.clone(),
            None => OperationJournal::open_default()?,
        };
        let executor = WorkspaceSyncExecutor::new(self.registry.clone(), journal);

        // Hold the generation lock across the synchronous Job creation call.
        // If a newer workspace action superseded this launch first, no Job can
        // appear. If we commit first, a concurrent supersede attempt returns
        // false and the UI keeps the immutable launch locked until its Job id
        // arrives.
        let mut generation = self
            .launch_generation
            .lock()
            .expect("workspace sync launch generation poisoned");
        if generation.current != launch_id.get() {
            return Err(WorkspaceSyncLaunchError::Superseded);
        }
        generation.committed = true;
        let job_id = jobs.spawn_workspace_sync_with_verification(
            compiled,
            executor,
            job_events,
            self.verification.clone(),
            verification_events,
        );
        Ok(job_id)
    }

    fn ensure_launch_current(
        &self,
        launch_id: SyncLaunchId,
    ) -> Result<(), WorkspaceSyncLaunchError> {
        if self.is_launch_current(launch_id) {
            Ok(())
        } else {
            Err(WorkspaceSyncLaunchError::Superseded)
        }
    }

    async fn rescan_current(
        &self,
        launch_id: SyncLaunchId,
        left: &Location,
        right: &Location,
    ) -> Result<WorkspaceDiff, WorkspaceSyncLaunchError> {
        let cancel = AtomicBool::new(false);
        let left_entries = scan_workspace(
            &self.registry,
            left,
            WorkspaceScanOptions::default(),
            &cancel,
        )
        .await?;
        self.ensure_launch_current(launch_id)?;
        let right_entries = scan_workspace(
            &self.registry,
            right,
            WorkspaceScanOptions::default(),
            &cancel,
        )
        .await?;
        self.ensure_launch_current(launch_id)?;
        Ok(WorkspaceDiff::compare(
            left.clone(),
            right.clone(),
            left_entries,
            right_entries,
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
    #[error("workspace revalidation failed: {0}")]
    Revalidation(#[from] WorkspaceScanError),
    #[error("transfer capability probe failed: {0}")]
    Probe(String),
    #[error("transfer capability worker failed: {0}")]
    ProbeWorker(String),
    #[error("operation journal is unavailable: {0}")]
    Journal(#[from] io::Error),
    #[error("workspace sync preparation was superseded by a newer action")]
    Superseded,
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
            Self::Superseded => {
                "A newer workspace action replaced this preparation.".to_string()
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
    use crate::workspace_sync::{SyncMode, SyncPolicy, WorkspaceEntry, WorkspaceFingerprint};

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
            .launch(frozen, diff, false, jobs.clone(), job_tx, verification_tx)
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
    async fn out_of_band_change_is_rescanned_before_job_creation() {
        let left = tempfile::tempdir().unwrap();
        let right = tempfile::tempdir().unwrap();
        tokio::fs::write(right.path().join("old.txt"), b"x")
            .await
            .unwrap();

        let registry = default_registry();
        let left_root = Location::Local(left.path().to_path_buf());
        let right_root = Location::Local(right.path().to_path_buf());
        let cancel = AtomicBool::new(false);
        let original = WorkspaceDiff::compare(
            left_root.clone(),
            right_root.clone(),
            scan_workspace(
                &registry,
                &left_root,
                WorkspaceScanOptions::default(),
                &cancel,
            )
            .await
            .unwrap(),
            scan_workspace(
                &registry,
                &right_root,
                WorkspaceScanOptions::default(),
                &cancel,
            )
            .await
            .unwrap(),
        );
        let plan = WorkspaceSyncPlan::build(
            &original,
            SyncPolicy {
                mode: SyncMode::Mirror,
                ..SyncPolicy::default()
            },
        );
        let controller = WorkspaceSyncController::new(registry);
        let frozen = controller.freeze(&plan, &original).unwrap();

        tokio::fs::write(
            right.path().join("old.txt"),
            b"changed-after-preview-with-different-size\n",
        )
        .await
        .unwrap();

        let jobs = JobManager::new();
        let (job_tx, _job_rx) = mpsc::unbounded_channel();
        let (verification_tx, _verification_rx) = mpsc::unbounded_channel();
        let error = controller
            .launch(
                frozen,
                original,
                true,
                jobs.clone(),
                job_tx,
                verification_tx,
            )
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            WorkspaceSyncLaunchError::Validation(SyncValidationError::DestinationChanged { ref path })
                if path == "old.txt"
        ));
        assert!(jobs.snapshot().is_empty());
        assert_eq!(
            tokio::fs::read(right.path().join("old.txt")).await.unwrap(),
            b"changed-after-preview-with-different-size\n"
        );
    }

    #[tokio::test]
    async fn stale_confirmed_frozen_plan_cannot_queue_after_workspace_change() {
        let left = tempfile::tempdir().unwrap();
        let right = tempfile::tempdir().unwrap();
        let original = WorkspaceDiff::compare(
            Location::Local(left.path().to_path_buf()),
            Location::Local(right.path().to_path_buf()),
            Vec::<WorkspaceEntry>::new(),
            vec![file("old.txt", 1)],
        );
        let plan = WorkspaceSyncPlan::build(
            &original,
            SyncPolicy {
                mode: SyncMode::Mirror,
                ..SyncPolicy::default()
            },
        );
        let controller = WorkspaceSyncController::new(default_registry());
        let frozen = controller.freeze(&plan, &original).unwrap();
        let changed = WorkspaceDiff::compare(
            Location::Local(left.path().to_path_buf()),
            Location::Local(right.path().to_path_buf()),
            Vec::<WorkspaceEntry>::new(),
            vec![file("old.txt", 2)],
        );
        let jobs = JobManager::new();
        let (job_tx, _job_rx) = mpsc::unbounded_channel();
        let (verification_tx, _verification_rx) = mpsc::unbounded_channel();

        let error = controller
            .launch(frozen, changed, true, jobs.clone(), job_tx, verification_tx)
            .await
            .unwrap_err();

        assert!(matches!(error, WorkspaceSyncLaunchError::Validation(_)));
        assert!(jobs.snapshot().is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn compiler_rejection_creates_no_job_and_no_mutation() {
        let left = tempfile::tempdir().unwrap();
        let right = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("missing-target", left.path().join("link")).unwrap();
        let registry = default_registry();
        let left_root = Location::Local(left.path().to_path_buf());
        let right_root = Location::Local(right.path().to_path_buf());
        let cancel = AtomicBool::new(false);
        let diff = WorkspaceDiff::compare(
            left_root.clone(),
            right_root.clone(),
            scan_workspace(
                &registry,
                &left_root,
                WorkspaceScanOptions::default(),
                &cancel,
            )
            .await
            .unwrap(),
            Vec::<WorkspaceEntry>::new(),
        );
        let plan = WorkspaceSyncPlan::build(&diff, SyncPolicy::default());
        let controller = WorkspaceSyncController::new(registry);
        let frozen = controller.freeze(&plan, &diff).unwrap();
        let jobs = JobManager::new();
        let (job_tx, _job_rx) = mpsc::unbounded_channel();
        let (verification_tx, _verification_rx) = mpsc::unbounded_channel();

        let error = controller
            .launch(frozen, diff, false, jobs.clone(), job_tx, verification_tx)
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            WorkspaceSyncLaunchError::Compile(SyncCompileError::UnsupportedEntryKind { .. })
        ));
        assert!(jobs.snapshot().is_empty());
        assert!(!right.path().join("link").exists());
    }

    #[tokio::test]
    async fn safe_local_plan_launches_through_job_manager() {
        let left = tempfile::tempdir().unwrap();
        let right = tempfile::tempdir().unwrap();
        tokio::fs::write(left.path().join("a.txt"), b"a")
            .await
            .unwrap();
        let registry = default_registry();
        let left_root = Location::Local(left.path().to_path_buf());
        let right_root = Location::Local(right.path().to_path_buf());
        let cancel = AtomicBool::new(false);
        let diff = WorkspaceDiff::compare(
            left_root.clone(),
            right_root,
            scan_workspace(
                &registry,
                &left_root,
                WorkspaceScanOptions::default(),
                &cancel,
            )
            .await
            .unwrap(),
            Vec::<WorkspaceEntry>::new(),
        );
        let plan = WorkspaceSyncPlan::build(&diff, SyncPolicy::default());
        let journal_dir = tempfile::tempdir().unwrap();
        let controller = WorkspaceSyncController::with_journal(
            registry,
            OperationJournal::open(journal_dir.path().join("ops.jsonl")).unwrap(),
        );
        let frozen = controller.freeze(&plan, &diff).unwrap();
        let jobs = JobManager::new();
        let (job_tx, mut job_rx) = mpsc::unbounded_channel();
        let (verification_tx, _verification_rx) = mpsc::unbounded_channel();

        let id = controller
            .launch(frozen, diff, false, jobs.clone(), job_tx, verification_tx)
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
        let error =
            WorkspaceSyncLaunchError::Compile(SyncCompileError::RemoteToRemoteUnsupported {
                source_location: Box::new(Location::Sftp {
                    host: "a".into(),
                    path: "/src".into(),
                }),
                destination_location: Box::new(Location::Sftp {
                    host: "b".into(),
                    path: "/dst".into(),
                }),
            });
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

    #[tokio::test]
    async fn superseded_launch_generation_cannot_create_a_job() {
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
        let controller = WorkspaceSyncController::new(default_registry());
        let frozen = controller.freeze(&plan, &diff).unwrap();
        let stale_launch = controller.begin_launch();
        assert!(controller.supersede_launch());

        let jobs = JobManager::new();
        let (job_tx, _job_rx) = mpsc::unbounded_channel();
        let (verification_tx, _verification_rx) = mpsc::unbounded_channel();
        let error = controller
            .launch_guarded(
                stale_launch,
                frozen,
                diff,
                false,
                jobs.clone(),
                (job_tx, verification_tx),
            )
            .await
            .unwrap_err();

        assert!(matches!(error, WorkspaceSyncLaunchError::Superseded));
        assert!(jobs.snapshot().is_empty());
        assert!(!right.path().join("a.txt").exists());
    }
}
