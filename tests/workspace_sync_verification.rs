use std::path::{Path, PathBuf};
use std::time::Duration;

use arx::app::RemoteWorkspaceState;
use arx::jobs::{JobEvent, JobManager, JobResult, JobStatus};
use arx::journal::OperationJournal;
use arx::services::{WorkspaceScanId, WorkspaceScanOptions, WorkspaceScanResponse};
use arx::vfs::{EntryKind, Location, default_registry, local::LocalFs};
use arx::workspace_sync::{
    SyncPolicy, WorkspaceDiff, WorkspaceEntry, WorkspaceFingerprint, WorkspaceSide,
    WorkspaceSyncPlan,
};
use arx::workspace_sync_execution::{ExecutableSyncPlan, SyncPlanId, SyncPlanValidator};
use arx::workspace_sync_executor::{
    CompiledSyncPlan, SyncExecutionCompiler, SyncExecutorMatrix, SyncTerminalState,
    WorkspaceSyncExecutor,
};
use arx::workspace_sync_verification::{
    SyncVerificationCoordinator, SyncVerificationEvent, SyncVerificationResult,
    SyncVerificationRun, SyncVerificationStatus, SyncVerificationVerdict,
};
use tokio::sync::mpsc;

fn local(path: impl Into<PathBuf>) -> Location {
    Location::Local(path.into())
}

fn fingerprint(
    size: Option<u64>,
    modified_unix_ms: Option<u64>,
    content_hash: Option<&str>,
) -> WorkspaceFingerprint {
    WorkspaceFingerprint {
        kind: EntryKind::File,
        size,
        modified_unix_ms,
        content_hash: content_hash.map(str::to_string),
    }
}

fn entry(path: &str, fingerprint: WorkspaceFingerprint) -> WorkspaceEntry {
    WorkspaceEntry {
        relative_path: path.into(),
        fingerprint,
    }
}

fn test_plan_id() -> SyncPlanId {
    let diff = WorkspaceDiff::compare(
        local("/left"),
        local("/right"),
        vec![entry("a.txt", fingerprint(Some(1), None, None))],
        Vec::<WorkspaceEntry>::new(),
    );
    let registry = default_registry();
    let plan = WorkspaceSyncPlan::build(&diff, SyncPolicy::default());
    SyncPlanValidator::freeze(&plan, &diff, &registry)
        .unwrap()
        .id()
}

fn compile_local(left: &Path, right: &Path, files: &[&str]) -> CompiledSyncPlan {
    let listed = LocalFs::list(left).expect("local verification fixture should be listable");
    let left_entries = files
        .iter()
        .map(|name| {
            let provider_entry = listed
                .iter()
                .find(|item| item.name == *name)
                .unwrap_or_else(|| panic!("missing local verification fixture entry: {name}"));
            entry(
                name,
                WorkspaceFingerprint {
                    kind: provider_entry.kind,
                    size: provider_entry.size,
                    modified_unix_ms: provider_entry.modified_unix_ms,
                    content_hash: None,
                },
            )
        })
        .collect::<Vec<_>>();
    let diff = WorkspaceDiff::compare(
        local(left.to_path_buf()),
        local(right.to_path_buf()),
        left_entries,
        Vec::<WorkspaceEntry>::new(),
    );
    let registry = default_registry();
    let plan = WorkspaceSyncPlan::build(&diff, SyncPolicy::default());
    let frozen = SyncPlanValidator::freeze(&plan, &diff, &registry).unwrap();
    let executable = ExecutableSyncPlan::new(frozen, None).unwrap();
    SyncExecutionCompiler::compile(
        executable,
        &diff,
        &registry,
        &SyncExecutorMatrix::local_only(),
    )
    .unwrap()
}

fn sync_executor(journal: OperationJournal) -> WorkspaceSyncExecutor {
    WorkspaceSyncExecutor::new(default_registry(), journal)
}

async fn terminal_job_event(rx: &mut mpsc::UnboundedReceiver<JobEvent>) -> JobEvent {
    while let Some(event) = rx.recv().await {
        if event.is_terminal() {
            return event;
        }
    }
    panic!("job channel closed before terminal event");
}

async fn terminal_verification(
    run: &mut SyncVerificationRun,
) -> arx::workspace_sync_verification::SyncVerificationSnapshot {
    while let Some(snapshot) = run.recv().await {
        if snapshot.status.is_terminal() {
            return snapshot;
        }
    }
    panic!("verification channel closed before terminal status");
}

async fn terminal_verification_event(
    rx: &mut mpsc::UnboundedReceiver<SyncVerificationEvent>,
) -> SyncVerificationEvent {
    while let Some(event) = rx.recv().await {
        if event.verification.status.is_terminal() {
            return event;
        }
    }
    panic!("verification event channel closed before terminal status");
}

#[test]
fn equal_hashes_are_verified_as_synchronized() {
    let diff = WorkspaceDiff::compare(
        local("/left"),
        local("/right"),
        vec![entry(
            "a.txt",
            fingerprint(Some(10), None, Some("same-hash")),
        )],
        vec![entry(
            "a.txt",
            fingerprint(Some(10), None, Some("same-hash")),
        )],
    );
    let result = SyncVerificationResult::from_diff(test_plan_id(), diff);

    assert_eq!(result.verdict, SyncVerificationVerdict::Synchronized);
    assert_eq!(result.changed_entries, 0);
    assert_eq!(result.unverified_entries, 0);
}

#[test]
fn size_only_equality_is_inconclusive_not_a_fake_failure() {
    let diff = WorkspaceDiff::compare(
        local("/left"),
        local("/right"),
        vec![entry("a.txt", fingerprint(Some(10), None, None))],
        vec![entry("a.txt", fingerprint(Some(10), None, None))],
    );
    let result = SyncVerificationResult::from_diff(test_plan_id(), diff);

    assert_eq!(
        result.verdict,
        SyncVerificationVerdict::Inconclusive { unverified: 1 }
    );
    assert_eq!(result.changed_entries, 0);
    assert_eq!(result.unverified_entries, 1);
}

#[test]
fn proven_size_mismatch_is_a_remaining_difference() {
    let diff = WorkspaceDiff::compare(
        local("/left"),
        local("/right"),
        vec![entry("a.txt", fingerprint(Some(10), None, None))],
        vec![entry("a.txt", fingerprint(Some(11), None, None))],
    );
    let result = SyncVerificationResult::from_diff(test_plan_id(), diff);

    assert_eq!(
        result.verdict,
        SyncVerificationVerdict::DifferencesRemain {
            changed: 1,
            conflicts: 1,
            unverified: 0,
        }
    );
}

#[test]
fn one_sided_entry_is_a_remaining_difference_not_inconclusive() {
    let diff = WorkspaceDiff::compare(
        local("/left"),
        local("/right"),
        vec![entry("a.txt", fingerprint(Some(10), None, None))],
        Vec::<WorkspaceEntry>::new(),
    );
    let result = SyncVerificationResult::from_diff(test_plan_id(), diff);

    assert!(matches!(
        result.verdict,
        SyncVerificationVerdict::DifferencesRemain {
            changed: 1,
            conflicts: 0,
            unverified: 0,
        }
    ));
}

#[tokio::test]
async fn empty_equal_rescan_is_synchronized() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    let coordinator = SyncVerificationCoordinator::new(default_registry());
    let mut run = coordinator.start(
        "sync-1".into(),
        test_plan_id(),
        local(left.path().to_path_buf()),
        local(right.path().to_path_buf()),
    );

    let terminal = terminal_verification(&mut run).await;
    let SyncVerificationStatus::Finished(result) = terminal.status else {
        panic!("equal empty roots were not verified");
    };
    assert_eq!(result.verdict, SyncVerificationVerdict::Synchronized);
}

#[tokio::test]
async fn scan_correlation_rejects_stale_ids_and_roots() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    let coordinator = SyncVerificationCoordinator::new(default_registry());
    let mut run = coordinator.start(
        "sync-1".into(),
        test_plan_id(),
        local(left.path().to_path_buf()),
        local(right.path().to_path_buf()),
    );

    let running = loop {
        let snapshot = run.recv().await.unwrap();
        if matches!(snapshot.status, SyncVerificationStatus::Running { .. }) {
            break snapshot;
        }
    };
    let SyncVerificationStatus::Running {
        left_scan,
        right_scan,
    } = running.status
    else {
        unreachable!();
    };

    let stale_left = WorkspaceScanResponse {
        id: WorkspaceScanId(left_scan.0 + 100),
        side: WorkspaceSide::Left,
        root: running.left_root.clone(),
        result: Ok(Vec::new()),
    };
    assert!(!running.accepts_scan(&stale_left));

    let stale_right = WorkspaceScanResponse {
        id: right_scan,
        side: WorkspaceSide::Right,
        root: local("/different-root"),
        result: Ok(Vec::new()),
    };
    assert!(!running.accepts_scan(&stale_right));
}

#[tokio::test(flavor = "current_thread")]
async fn second_verification_supersedes_first_generation() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    let coordinator = SyncVerificationCoordinator::new(default_registry());
    let mut first = coordinator.start(
        "sync-1".into(),
        test_plan_id(),
        local(left.path().to_path_buf()),
        local(right.path().to_path_buf()),
    );
    let mut second = coordinator.start(
        "sync-1".into(),
        test_plan_id(),
        local(left.path().to_path_buf()),
        local(right.path().to_path_buf()),
    );

    let first_terminal = terminal_verification(&mut first).await;
    assert!(matches!(
        first_terminal.status,
        SyncVerificationStatus::Superseded
    ));
    let second_terminal = terminal_verification(&mut second).await;
    assert!(matches!(
        second_terminal.status,
        SyncVerificationStatus::Finished(_)
    ));
    assert!(second.id() > first.id());
}

#[tokio::test]
async fn completed_job_stays_completed_when_verification_is_synchronized() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    tokio::fs::write(left.path().join("a.txt"), b"a")
        .await
        .unwrap();
    let plan = compile_local(left.path(), right.path(), &["a.txt"]);
    let journal_dir = tempfile::tempdir().unwrap();
    let journal = OperationJournal::open(journal_dir.path().join("ops.jsonl")).unwrap();
    let manager = JobManager::new();
    let (job_tx, mut job_rx) = mpsc::unbounded_channel();
    let (verification_tx, mut verification_rx) = mpsc::unbounded_channel();

    let id = manager.spawn_workspace_sync_with_verification(
        plan,
        sync_executor(journal),
        job_tx,
        SyncVerificationCoordinator::new(default_registry()),
        verification_tx,
    );
    let terminal = terminal_job_event(&mut job_rx).await;
    assert!(matches!(terminal, JobEvent::Completed { .. }));
    let verification = terminal_verification_event(&mut verification_rx).await;
    assert_eq!(verification.job_id, id);

    let job = manager.get(&id).unwrap();
    assert_eq!(job.status, JobStatus::Completed);
    let Some(JobResult::WorkspaceSync(outcome)) = &job.result else {
        panic!("execution result was lost during verification");
    };
    assert!(matches!(outcome.terminal, SyncTerminalState::Completed));
    let Some(verification) = &job.verification else {
        panic!("verification was not attached to the sync job");
    };
    let SyncVerificationStatus::Finished(result) = &verification.status else {
        panic!("verification did not finish");
    };
    assert_eq!(result.verdict, SyncVerificationVerdict::Synchronized);
}

#[tokio::test]
async fn verification_failure_does_not_rewrite_completed_job_to_failed() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    tokio::fs::write(left.path().join("a.txt"), b"a")
        .await
        .unwrap();
    let plan = compile_local(left.path(), right.path(), &["a.txt"]);
    let journal_dir = tempfile::tempdir().unwrap();
    let journal = OperationJournal::open(journal_dir.path().join("ops.jsonl")).unwrap();
    let manager = JobManager::new();
    let (job_tx, mut job_rx) = mpsc::unbounded_channel();
    let (verification_tx, mut verification_rx) = mpsc::unbounded_channel();
    let verifier = SyncVerificationCoordinator::with_options(
        default_registry(),
        WorkspaceScanOptions {
            max_depth: 32,
            max_entries: 0,
        },
    );

    let id = manager.spawn_workspace_sync_with_verification(
        plan,
        sync_executor(journal),
        job_tx,
        verifier,
        verification_tx,
    );
    assert!(matches!(
        terminal_job_event(&mut job_rx).await,
        JobEvent::Completed { .. }
    ));
    let verification = terminal_verification_event(&mut verification_rx).await;
    assert!(matches!(
        verification.verification.status,
        SyncVerificationStatus::Failed { .. }
    ));

    let job = manager.get(&id).unwrap();
    assert_eq!(job.status, JobStatus::Completed);
    let Some(JobResult::WorkspaceSync(outcome)) = job.result else {
        panic!("completed execution result was overwritten");
    };
    assert!(matches!(outcome.terminal, SyncTerminalState::Completed));
}

#[tokio::test(flavor = "current_thread")]
async fn pre_cancel_with_zero_completed_steps_does_not_start_verification() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    tokio::fs::write(left.path().join("a.txt"), b"a")
        .await
        .unwrap();
    let plan = compile_local(left.path(), right.path(), &["a.txt"]);
    let journal_dir = tempfile::tempdir().unwrap();
    let journal = OperationJournal::open(journal_dir.path().join("ops.jsonl")).unwrap();
    let manager = JobManager::new();
    let (job_tx, mut job_rx) = mpsc::unbounded_channel();
    let (verification_tx, mut verification_rx) = mpsc::unbounded_channel();

    let id = manager.spawn_workspace_sync_with_verification(
        plan,
        sync_executor(journal),
        job_tx,
        SyncVerificationCoordinator::new(default_registry()),
        verification_tx,
    );
    assert!(manager.cancel(&id));
    assert!(matches!(
        terminal_job_event(&mut job_rx).await,
        JobEvent::Cancelled { .. }
    ));

    let no_verification = tokio::time::timeout(Duration::from_millis(100), verification_rx.recv())
        .await
        .unwrap();
    assert!(no_verification.is_none());
    assert!(manager.get(&id).unwrap().verification.is_none());
}

#[tokio::test]
async fn partial_failure_starts_verification_without_changing_failed_status() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    for name in ["a.txt", "b.txt"] {
        tokio::fs::write(left.path().join(name), b"x")
            .await
            .unwrap();
    }
    let plan = compile_local(left.path(), right.path(), &["a.txt", "b.txt"]);
    tokio::fs::write(left.path().join("b.txt"), b"changed")
        .await
        .unwrap();
    let journal_dir = tempfile::tempdir().unwrap();
    let journal = OperationJournal::open(journal_dir.path().join("ops.jsonl")).unwrap();
    let manager = JobManager::new();
    let (job_tx, mut job_rx) = mpsc::unbounded_channel();
    let (verification_tx, mut verification_rx) = mpsc::unbounded_channel();

    let id = manager.spawn_workspace_sync_with_verification(
        plan,
        sync_executor(journal),
        job_tx,
        SyncVerificationCoordinator::new(default_registry()),
        verification_tx,
    );
    assert!(matches!(
        terminal_job_event(&mut job_rx).await,
        JobEvent::Failed { .. }
    ));
    let _ = terminal_verification_event(&mut verification_rx).await;

    let job = manager.get(&id).unwrap();
    assert_eq!(job.status, JobStatus::Failed);
    let Some(JobResult::WorkspaceSync(outcome)) = job.result else {
        panic!("failed partial execution outcome was lost");
    };
    assert_eq!(outcome.completed.len(), 1);
    assert!(outcome.workspace_may_have_changed);
    assert!(matches!(outcome.terminal, SyncTerminalState::Failed { .. }));
    assert!(job.verification.is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn first_step_cancel_request_never_suppresses_required_verification() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    tokio::fs::write(left.path().join("a.txt"), b"a")
        .await
        .unwrap();
    let plan = compile_local(left.path(), right.path(), &["a.txt"]);
    let journal_dir = tempfile::tempdir().unwrap();
    let journal = OperationJournal::open(journal_dir.path().join("ops.jsonl")).unwrap();
    let manager = JobManager::new();
    let (job_tx, mut job_rx) = mpsc::unbounded_channel();
    let (verification_tx, mut verification_rx) = mpsc::unbounded_channel();

    let id = manager.spawn_workspace_sync_with_verification(
        plan,
        sync_executor(journal),
        job_tx,
        SyncVerificationCoordinator::new(default_registry()),
        verification_tx,
    );
    while let Some(event) = job_rx.recv().await {
        if let JobEvent::Progress {
            progress: arx::jobs::JobProgress::WorkspaceSync(progress),
            ..
        } = event
            && progress.current_step.is_some()
            && progress.completed_steps == 0
        {
            // The tiny native copy may legitimately win the race before the
            // presentation observes StepStarted. Losing that race is not a
            // cancellation failure: Completed also requires verification.
            let _ = manager.cancel(&id);
            break;
        }
    }
    let terminal = terminal_job_event(&mut job_rx).await;
    assert!(matches!(
        terminal,
        JobEvent::Cancelled { .. } | JobEvent::Completed { .. }
    ));
    let _ = terminal_verification_event(&mut verification_rx).await;

    let job = manager.get(&id).unwrap();
    assert!(matches!(
        job.status,
        JobStatus::Cancelled | JobStatus::Completed
    ));
    let Some(JobResult::WorkspaceSync(outcome)) = job.result else {
        panic!("first-step execution outcome was lost");
    };
    assert!(outcome.workspace_may_have_changed);
    assert!(job.verification.is_some());
}

#[tokio::test]
async fn changed_workspace_roots_reject_late_verification_result() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    let left_root = local(left.path().to_path_buf());
    let right_root = local(right.path().to_path_buf());
    let coordinator = SyncVerificationCoordinator::new(default_registry());
    let mut run = coordinator.start(
        "sync-1".into(),
        test_plan_id(),
        left_root.clone(),
        right_root.clone(),
    );

    let pending = run.recv().await.unwrap();
    let running = run.recv().await.unwrap();
    let finished = terminal_verification(&mut run).await;

    let mut state = RemoteWorkspaceState {
        enabled: true,
        ..RemoteWorkspaceState::default()
    };
    assert!(state.apply_verification(&pending, &left_root, &right_root));
    assert!(state.apply_verification(&running, &left_root, &right_root));
    assert!(!state.apply_verification(&finished, &local("/new-left"), &local("/new-right")));
    assert!(state.diff.is_none());
    assert!(matches!(
        state.verification.as_ref().map(|item| &item.status),
        Some(SyncVerificationStatus::Superseded)
    ));
}

#[test]
fn verification_layer_is_read_only_and_does_not_reenter_execution() {
    let verification = include_str!("../src/workspace_sync_verification.rs");
    for forbidden in [
        "SyncExecutionCompiler",
        "WorkspaceSyncExecutor",
        "TransferPlanner",
        "MutationService",
        "execute_transfer",
    ] {
        assert!(
            !verification.contains(forbidden),
            "verification layer re-entered execution through {forbidden}"
        );
    }
}
