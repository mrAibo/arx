use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use arx::jobs::{
    JobEvent, JobKind, JobManager, JobProgress, JobResult, JobStatus, SyncJobProgress,
};
use arx::journal::OperationJournal;
use arx::vfs::{EntryKind, Location, default_registry};
use arx::workspace_sync::{
    SyncPolicy, WorkspaceDiff, WorkspaceEntry, WorkspaceFingerprint, WorkspaceSyncPlan,
};
use arx::workspace_sync_execution::{ExecutableSyncPlan, SyncPlanValidator};
use arx::workspace_sync_executor::{
    CompiledSyncPlan, SyncExecutionCompiler, SyncExecutorMatrix, SyncTerminalState,
    WorkspaceSyncExecutor,
};
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

fn compile_local(left: &Path, right: &Path, left_entries: Vec<WorkspaceEntry>) -> CompiledSyncPlan {
    let diff = WorkspaceDiff::compare(
        local(left.to_path_buf()),
        local(right.to_path_buf()),
        left_entries,
        Vec::<WorkspaceEntry>::new(),
    );
    let registry = default_registry();
    let logical = WorkspaceSyncPlan::build(&diff, SyncPolicy::default());
    let frozen = SyncPlanValidator::freeze(&logical, &diff, &registry).unwrap();
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

async fn terminal_event(rx: &mut mpsc::UnboundedReceiver<JobEvent>) -> JobEvent {
    while let Some(event) = rx.recv().await {
        if event.is_terminal() {
            return event;
        }
    }
    panic!("job event channel closed before a terminal event");
}

#[tokio::test]
async fn completed_sync_job_retains_typed_outcome_and_structured_progress() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    tokio::fs::write(left.path().join("a.txt"), b"a")
        .await
        .unwrap();
    let plan = compile_local(left.path(), right.path(), vec![file("a.txt", 1)]);
    let journal_dir = tempfile::tempdir().unwrap();
    let journal = OperationJournal::open(journal_dir.path().join("ops.jsonl")).unwrap();
    let manager = JobManager::new();
    let (tx, mut rx) = mpsc::unbounded_channel();

    let id = manager.spawn_workspace_sync(plan, sync_executor(journal), tx);
    let terminal = terminal_event(&mut rx).await;

    assert!(matches!(terminal, JobEvent::Completed { .. }));
    let job = manager.get(&id).unwrap();
    assert_eq!(job.kind, JobKind::Synchronize);
    assert_eq!(job.status, JobStatus::Completed);
    let JobProgress::WorkspaceSync(progress) = job.progress else {
        panic!("sync job lost structured progress");
    };
    assert_eq!(progress.completed_steps, 1);
    assert_eq!(progress.total_steps, 1);
    assert_eq!(progress.transferred_bytes, 1);
    assert_eq!(progress.total_bytes, 1);

    let Some(JobResult::WorkspaceSync(outcome)) = job.result else {
        panic!("sync job lost typed execution outcome");
    };
    assert!(matches!(outcome.terminal, SyncTerminalState::Completed));
    assert_eq!(outcome.completed.len(), 1);
    assert!(outcome.remaining.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn cancel_before_worker_runs_keeps_one_shared_token_and_zero_completed_steps() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    tokio::fs::write(left.path().join("a.txt"), b"a")
        .await
        .unwrap();
    let plan = compile_local(left.path(), right.path(), vec![file("a.txt", 1)]);
    let journal_dir = tempfile::tempdir().unwrap();
    let journal = OperationJournal::open(journal_dir.path().join("ops.jsonl")).unwrap();
    let manager = JobManager::new();
    let (tx, mut rx) = mpsc::unbounded_channel();

    let id = manager.spawn_workspace_sync(plan, sync_executor(journal), tx);
    let token = manager.cancel_token(&id).unwrap();
    assert!(manager.cancel(&id));
    assert!(token.load(Ordering::Relaxed));

    let terminal = terminal_event(&mut rx).await;
    assert!(matches!(terminal, JobEvent::Cancelled { .. }));
    let job = manager.get(&id).unwrap();
    assert_eq!(job.status, JobStatus::Cancelled);
    let Some(JobResult::WorkspaceSync(outcome)) = job.result else {
        panic!("cancelled sync job lost typed execution outcome");
    };
    assert!(matches!(
        outcome.terminal,
        SyncTerminalState::Cancelled { completed_steps: 0 }
    ));
    assert!(outcome.completed.is_empty());
    assert_eq!(outcome.remaining.len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn cancel_at_current_step_propagates_through_the_jobs_token() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    tokio::fs::write(left.path().join("a.txt"), b"a")
        .await
        .unwrap();
    let plan = compile_local(left.path(), right.path(), vec![file("a.txt", 1)]);
    let journal_dir = tempfile::tempdir().unwrap();
    let journal = OperationJournal::open(journal_dir.path().join("ops.jsonl")).unwrap();
    let manager = JobManager::new();
    let (tx, mut rx) = mpsc::unbounded_channel();

    let id = manager.spawn_workspace_sync(plan, sync_executor(journal), tx);
    while let Some(event) = rx.recv().await {
        if matches!(
            event,
            JobEvent::Progress {
                progress: JobProgress::WorkspaceSync(SyncJobProgress {
                    current_step: Some(_),
                    ..
                }),
                ..
            }
        ) {
            assert!(manager.cancel(&id));
            break;
        }
    }
    let terminal = terminal_event(&mut rx).await;

    assert!(matches!(terminal, JobEvent::Cancelled { .. }));
    let job = manager.get(&id).unwrap();
    let Some(JobResult::WorkspaceSync(outcome)) = job.result else {
        panic!("cancelled sync job lost typed execution outcome");
    };
    assert!(matches!(
        outcome.terminal,
        SyncTerminalState::Cancelled { .. }
    ));
}

#[tokio::test]
async fn failed_step_retains_full_partial_sync_outcome() {
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
    );
    tokio::fs::write(left.path().join("b.txt"), b"changed")
        .await
        .unwrap();
    let journal_dir = tempfile::tempdir().unwrap();
    let journal = OperationJournal::open(journal_dir.path().join("ops.jsonl")).unwrap();
    let manager = JobManager::new();
    let (tx, mut rx) = mpsc::unbounded_channel();

    let id = manager.spawn_workspace_sync(plan, sync_executor(journal), tx);
    let terminal = terminal_event(&mut rx).await;

    assert!(matches!(
        terminal,
        JobEvent::Failed {
            result: Some(_),
            ..
        }
    ));
    let job = manager.get(&id).unwrap();
    assert_eq!(job.status, JobStatus::Failed);
    let Some(JobResult::WorkspaceSync(outcome)) = job.result else {
        panic!("failed sync job was flattened to a generic error");
    };
    assert_eq!(outcome.completed.len(), 1);
    assert!(matches!(
        outcome.terminal,
        SyncTerminalState::Failed { step, .. } if step.0 == 2
    ));
    assert_eq!(outcome.remaining.len(), 1);
    assert_eq!(outcome.remaining[0].step.relative_path(), "c.txt");
}

#[test]
fn job_layer_accepts_compiled_sync_plans_only() {
    let jobs = include_str!("../src/jobs/mod.rs");
    assert!(jobs.contains("compiled_plan: CompiledSyncPlan"));
    assert!(!jobs.contains("compiled_plan: WorkspaceSyncPlan"));
    assert!(!jobs.contains("compiled_plan: FrozenWorkspaceSyncPlan"));
    assert!(!jobs.contains("SyncExecutionCompiler::compile"));
    assert!(!jobs.contains("TransferPlanner::plan"));
}
