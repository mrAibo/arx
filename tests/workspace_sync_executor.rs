use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use arx::journal::{OperationJournal, OperationState};
use arx::vfs::{EntryKind, Location, default_registry};
use arx::workspace_sync::{
    SyncMode, SyncPolicy, WorkspaceDiff, WorkspaceEntry, WorkspaceFingerprint, WorkspaceSyncPlan,
};
use arx::workspace_sync_execution::{ExecutableSyncPlan, SyncConfirmationToken, SyncPlanValidator};
use arx::workspace_sync_executor::{
    SyncExecutionCompiler, SyncExecutionError, SyncExecutionEvent, SyncExecutorMatrix,
    SyncTerminalState, WorkspaceSyncExecutor,
};
use arx::workspace_sync_journal::SyncJournalMetadata;
use tokio::sync::mpsc;

fn local(path: impl Into<PathBuf>) -> Location {
    Location::Local(path.into())
}

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

fn dir(path: &str) -> WorkspaceEntry {
    WorkspaceEntry {
        relative_path: path.into(),
        fingerprint: WorkspaceFingerprint {
            kind: EntryKind::Directory,
            size: None,
            modified_unix_ms: None,
            content_hash: None,
        },
    }
}

fn compile_local(
    left: &Path,
    right: &Path,
    left_entries: Vec<WorkspaceEntry>,
    right_entries: Vec<WorkspaceEntry>,
    policy: SyncPolicy,
) -> arx::workspace_sync_executor::CompiledSyncPlan {
    let diff = WorkspaceDiff::compare(
        local(left.to_path_buf()),
        local(right.to_path_buf()),
        left_entries,
        right_entries,
    );
    let registry = default_registry();
    let logical = WorkspaceSyncPlan::build(&diff, policy);
    let frozen = SyncPlanValidator::freeze(&logical, &diff, &registry).unwrap();
    let confirmation = frozen
        .requires_confirmation()
        .then(|| SyncConfirmationToken::from_explicit_confirmation(&frozen));
    let executable = ExecutableSyncPlan::new(frozen, confirmation).unwrap();
    SyncExecutionCompiler::compile(
        executable,
        &diff,
        &registry,
        &SyncExecutorMatrix::local_only(),
    )
    .unwrap()
}

fn executor(journal: OperationJournal) -> WorkspaceSyncExecutor {
    WorkspaceSyncExecutor::new(default_registry(), journal)
}

fn states(journal: &OperationJournal) -> Vec<OperationState> {
    journal
        .read_all()
        .unwrap()
        .into_iter()
        .map(|record| record.state)
        .collect()
}

#[tokio::test]
async fn successful_plan_journals_started_running_completed() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    tokio::fs::write(left.path().join("a.txt"), b"a")
        .await
        .unwrap();
    let plan = compile_local(
        left.path(),
        right.path(),
        vec![file("a.txt", 1)],
        Vec::new(),
        SyncPolicy::default(),
    );
    let journal_dir = tempfile::tempdir().unwrap();
    let journal = OperationJournal::open(journal_dir.path().join("ops.jsonl")).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();

    let outcome = executor(journal.clone())
        .execute(plan, Arc::new(AtomicBool::new(false)), tx)
        .await
        .unwrap();

    assert!(matches!(outcome.terminal, SyncTerminalState::Completed));
    assert_eq!(outcome.completed.len(), 1);
    assert!(outcome.remaining.is_empty());
    assert_eq!(
        tokio::fs::read(right.path().join("a.txt")).await.unwrap(),
        b"a"
    );
    assert_eq!(
        states(&journal),
        vec![
            OperationState::Started,
            OperationState::Running,
            OperationState::Completed,
        ]
    );
}

#[tokio::test]
async fn already_cancelled_plan_completes_zero_steps() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    tokio::fs::write(left.path().join("a.txt"), b"a")
        .await
        .unwrap();
    let plan = compile_local(
        left.path(),
        right.path(),
        vec![file("a.txt", 1)],
        Vec::new(),
        SyncPolicy::default(),
    );
    let journal_dir = tempfile::tempdir().unwrap();
    let journal = OperationJournal::open(journal_dir.path().join("ops.jsonl")).unwrap();
    let cancel = Arc::new(AtomicBool::new(true));
    let (tx, _rx) = mpsc::unbounded_channel();

    let outcome = executor(journal.clone())
        .execute(plan, cancel, tx)
        .await
        .unwrap();

    assert!(matches!(
        outcome.terminal,
        SyncTerminalState::Cancelled { completed_steps: 0 }
    ));
    assert_eq!(outcome.completed.len(), 0);
    assert_eq!(outcome.remaining.len(), 1);
    assert!(!right.path().join("a.txt").exists());
    assert_eq!(
        states(&journal),
        vec![
            OperationState::Started,
            OperationState::Running,
            OperationState::Cancelled,
        ]
    );
}

#[tokio::test]
async fn source_change_after_compile_stops_before_mutation() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    tokio::fs::write(left.path().join("a.txt"), b"a")
        .await
        .unwrap();
    let plan = compile_local(
        left.path(),
        right.path(),
        vec![file("a.txt", 1)],
        Vec::new(),
        SyncPolicy::default(),
    );
    tokio::fs::write(left.path().join("a.txt"), b"changed")
        .await
        .unwrap();
    let journal_dir = tempfile::tempdir().unwrap();
    let journal = OperationJournal::open(journal_dir.path().join("ops.jsonl")).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();

    let outcome = executor(journal.clone())
        .execute(plan, Arc::new(AtomicBool::new(false)), tx)
        .await
        .unwrap();

    assert!(matches!(
        outcome.terminal,
        SyncTerminalState::Failed {
            error: SyncExecutionError::StaleStep { .. },
            ..
        }
    ));
    assert!(outcome.completed.is_empty());
    assert!(!right.path().join("a.txt").exists());
    assert_eq!(states(&journal).last(), Some(&OperationState::Failed));
}

#[tokio::test]
async fn destination_change_after_compile_stops_before_overwrite() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    tokio::fs::write(left.path().join("a.txt"), b"a")
        .await
        .unwrap();
    let plan = compile_local(
        left.path(),
        right.path(),
        vec![file("a.txt", 1)],
        Vec::new(),
        SyncPolicy::default(),
    );
    tokio::fs::write(right.path().join("a.txt"), b"external")
        .await
        .unwrap();
    let journal_dir = tempfile::tempdir().unwrap();
    let journal = OperationJournal::open(journal_dir.path().join("ops.jsonl")).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();

    let outcome = executor(journal)
        .execute(plan, Arc::new(AtomicBool::new(false)), tx)
        .await
        .unwrap();

    assert!(matches!(
        outcome.terminal,
        SyncTerminalState::Failed {
            error: SyncExecutionError::StaleStep { .. },
            ..
        }
    ));
    assert_eq!(
        tokio::fs::read(right.path().join("a.txt")).await.unwrap(),
        b"external"
    );
}

#[tokio::test]
async fn new_child_prevents_non_recursive_directory_delete() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    tokio::fs::create_dir(right.path().join("old"))
        .await
        .unwrap();
    let plan = compile_local(
        left.path(),
        right.path(),
        Vec::new(),
        vec![dir("old")],
        SyncPolicy {
            mode: SyncMode::Mirror,
            ..SyncPolicy::default()
        },
    );
    tokio::fs::write(right.path().join("old/IMPORTANT"), b"keep")
        .await
        .unwrap();
    let journal_dir = tempfile::tempdir().unwrap();
    let journal = OperationJournal::open(journal_dir.path().join("ops.jsonl")).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();

    let outcome = executor(journal)
        .execute(plan, Arc::new(AtomicBool::new(false)), tx)
        .await
        .unwrap();

    assert!(matches!(
        outcome.terminal,
        SyncTerminalState::Failed {
            error: SyncExecutionError::Mutation { .. },
            ..
        }
    ));
    assert!(right.path().join("old/IMPORTANT").exists());
}

#[tokio::test]
async fn failure_on_step_two_reports_truthful_partial_completion() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    for name in ["a.txt", "b.txt", "c.txt"] {
        tokio::fs::write(left.path().join(name), b"x")
            .await
            .unwrap();
    }
    let plan = compile_local(
        left.path(),
        right.path(),
        vec![file("a.txt", 1), file("b.txt", 1), file("c.txt", 1)],
        Vec::new(),
        SyncPolicy::default(),
    );
    tokio::fs::write(left.path().join("b.txt"), b"changed")
        .await
        .unwrap();
    let journal_dir = tempfile::tempdir().unwrap();
    let journal = OperationJournal::open(journal_dir.path().join("ops.jsonl")).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();

    let outcome = executor(journal.clone())
        .execute(plan, Arc::new(AtomicBool::new(false)), tx)
        .await
        .unwrap();

    assert_eq!(outcome.completed.len(), 1);
    assert_eq!(outcome.completed[0].relative_path, "a.txt");
    assert!(matches!(
        outcome.terminal,
        SyncTerminalState::Failed { step, .. } if step.0 == 2
    ));
    assert_eq!(outcome.remaining.len(), 1);
    assert_eq!(outcome.remaining[0].step.relative_path(), "c.txt");
    assert!(right.path().join("a.txt").exists());
    assert!(!right.path().join("b.txt").exists());
    assert!(!right.path().join("c.txt").exists());

    let terminal = journal.read_all().unwrap().pop().unwrap();
    let metadata: SyncJournalMetadata = serde_json::from_value(terminal.metadata.unwrap()).unwrap();
    let execution = metadata.execution.unwrap();
    assert_eq!(execution.completed_steps, 1);
    assert_eq!(execution.failed_step, Some(2));
    assert_eq!(execution.remaining_steps, 1);
    assert!(!execution.rollback_attempted);
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_between_steps_prevents_next_step_from_starting() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    tokio::fs::write(left.path().join("a.txt"), b"a")
        .await
        .unwrap();
    tokio::fs::write(left.path().join("b.txt"), b"b")
        .await
        .unwrap();
    let plan = compile_local(
        left.path(),
        right.path(),
        vec![file("a.txt", 1), file("b.txt", 1)],
        Vec::new(),
        SyncPolicy::default(),
    );
    let journal_dir = tempfile::tempdir().unwrap();
    let journal = OperationJournal::open(journal_dir.path().join("ops.jsonl")).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    let sync_executor = executor(journal);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let handle = tokio::spawn(async move { sync_executor.execute(plan, worker_cancel, tx).await });

    while let Some(event) = rx.recv().await {
        if matches!(event, SyncExecutionEvent::StepCompleted { id, .. } if id.0 == 1) {
            cancel.store(true, Ordering::Relaxed);
            break;
        }
    }
    let outcome = handle.await.unwrap().unwrap();

    assert!(matches!(
        outcome.terminal,
        SyncTerminalState::Cancelled { completed_steps: 1 }
    ));
    assert!(right.path().join("a.txt").exists());
    assert!(!right.path().join("b.txt").exists());
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_at_transfer_start_uses_the_same_shared_token() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    tokio::fs::write(left.path().join("a.txt"), b"a")
        .await
        .unwrap();
    let plan = compile_local(
        left.path(),
        right.path(),
        vec![file("a.txt", 1)],
        Vec::new(),
        SyncPolicy::default(),
    );
    let journal_dir = tempfile::tempdir().unwrap();
    let journal = OperationJournal::open(journal_dir.path().join("ops.jsonl")).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    let sync_executor = executor(journal);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let handle = tokio::spawn(async move { sync_executor.execute(plan, worker_cancel, tx).await });

    while let Some(event) = rx.recv().await {
        if matches!(event, SyncExecutionEvent::StepStarted { id, .. } if id.0 == 1) {
            cancel.store(true, Ordering::Relaxed);
            break;
        }
    }
    let outcome = handle.await.unwrap().unwrap();

    assert!(matches!(
        outcome.terminal,
        SyncTerminalState::Cancelled { completed_steps: 0 }
    ));
    assert!(!right.path().join("a.txt").exists());
}
