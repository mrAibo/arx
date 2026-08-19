use std::path::PathBuf;

use arx::journal::OperationJournal;
use arx::transfer::{ExecutorAvailability, TransferMethod};
use arx::vfs::{EntryKind, Location, default_registry};
use arx::workspace_sync::{
    ConflictPolicy, SyncMode, SyncPolicy, WorkspaceDiff, WorkspaceEntry, WorkspaceFingerprint,
    WorkspaceSyncPlan,
};
use arx::workspace_sync_execution::{ExecutableSyncPlan, SyncConfirmationToken, SyncPlanValidator};
use arx::workspace_sync_executor::{
    PhysicalSyncStep, SyncCompileError, SyncExecutionCompiler, SyncExecutorMatrix,
};

fn local(path: impl Into<PathBuf>) -> Location {
    Location::Local(path.into())
}

fn sftp(host: &str, path: &str) -> Location {
    Location::Sftp {
        host: host.into(),
        path: path.into(),
    }
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

fn executable(
    diff: &WorkspaceDiff,
    policy: SyncPolicy,
    explicit_confirmation: bool,
) -> ExecutableSyncPlan {
    let registry = default_registry();
    let logical = WorkspaceSyncPlan::build(diff, policy);
    let frozen = SyncPlanValidator::freeze(&logical, diff, &registry).unwrap();
    let confirmation =
        explicit_confirmation.then(|| SyncConfirmationToken::from_explicit_confirmation(&frozen));
    ExecutableSyncPlan::new(frozen, confirmation).unwrap()
}

fn remote_matrix(host: &str) -> SyncExecutorMatrix {
    SyncExecutorMatrix::local_only().with_remote(
        host,
        ExecutorAvailability {
            native: false,
            rsync: false,
            sftp: true,
            s3: false,
            webdav: false,
        },
    )
}

#[test]
fn directory_and_children_compile_without_recursive_duplicate() {
    let diff = WorkspaceDiff::compare(
        local("/left"),
        local("/right"),
        vec![dir("src"), file("src/main.rs", 10)],
        Vec::<WorkspaceEntry>::new(),
    );
    let compiled = SyncExecutionCompiler::compile(
        executable(&diff, SyncPolicy::default(), false),
        &diff,
        &default_registry(),
        &SyncExecutorMatrix::local_only(),
    )
    .unwrap();

    assert_eq!(compiled.steps().len(), 2);
    assert!(matches!(
        &compiled.steps()[0].step,
        PhysicalSyncStep::EnsureDirectory { relative_path, .. } if relative_path == "src"
    ));
    assert!(matches!(
        &compiled.steps()[1].step,
        PhysicalSyncStep::TransferFile { relative_path, .. } if relative_path == "src/main.rs"
    ));
}

#[test]
fn directory_creation_is_parent_first() {
    let diff = WorkspaceDiff::compare(
        local("/left"),
        local("/right"),
        vec![dir("a"), dir("a/b"), file("a/b/c.txt", 1)],
        Vec::<WorkspaceEntry>::new(),
    );
    let compiled = SyncExecutionCompiler::compile(
        executable(&diff, SyncPolicy::default(), false),
        &diff,
        &default_registry(),
        &SyncExecutorMatrix::local_only(),
    )
    .unwrap();

    let paths = compiled
        .steps()
        .iter()
        .map(|step| step.step.relative_path())
        .collect::<Vec<_>>();
    assert_eq!(paths, vec!["a", "a/b", "a/b/c.txt"]);
}

#[test]
fn mirror_deletes_files_before_directories_and_deepest_directory_first() {
    let diff = WorkspaceDiff::compare(
        local("/left"),
        local("/right"),
        Vec::<WorkspaceEntry>::new(),
        vec![
            dir("old"),
            dir("old/sub"),
            file("old/a.txt", 1),
            file("old/sub/b.txt", 1),
        ],
    );
    let policy = SyncPolicy {
        mode: SyncMode::Mirror,
        ..SyncPolicy::default()
    };
    let compiled = SyncExecutionCompiler::compile(
        executable(&diff, policy, true),
        &diff,
        &default_registry(),
        &SyncExecutorMatrix::local_only(),
    )
    .unwrap();

    assert!(matches!(
        compiled.steps()[0].step,
        PhysicalSyncStep::DeleteFile { .. }
    ));
    assert!(matches!(
        compiled.steps()[1].step,
        PhysicalSyncStep::DeleteFile { .. }
    ));
    assert!(matches!(
        &compiled.steps()[2].step,
        PhysicalSyncStep::RemoveDirectory { relative_path, .. } if relative_path == "old/sub"
    ));
    assert!(matches!(
        &compiled.steps()[3].step,
        PhysicalSyncStep::RemoveDirectory { relative_path, .. } if relative_path == "old"
    ));
}

#[test]
fn file_replaces_directory_only_through_explicit_structural_steps() {
    let diff = WorkspaceDiff::compare(
        local("/left"),
        local("/right"),
        vec![file("config", 7)],
        vec![dir("config"), file("config/old.txt", 3)],
    );
    let policy = SyncPolicy {
        mode: SyncMode::Mirror,
        conflicts: ConflictPolicy::PreferSource,
        ..SyncPolicy::default()
    };
    let compiled = SyncExecutionCompiler::compile(
        executable(&diff, policy, true),
        &diff,
        &default_registry(),
        &SyncExecutorMatrix::local_only(),
    )
    .unwrap();

    assert_eq!(compiled.steps().len(), 3);
    assert!(matches!(
        &compiled.steps()[0].step,
        PhysicalSyncStep::DeleteFile { relative_path, .. } if relative_path == "config/old.txt"
    ));
    assert!(matches!(
        &compiled.steps()[1].step,
        PhysicalSyncStep::RemoveDirectory { relative_path, .. } if relative_path == "config"
    ));
    assert!(matches!(
        &compiled.steps()[2].step,
        PhysicalSyncStep::TransferFile { relative_path, .. } if relative_path == "config"
    ));
}

#[test]
fn directory_replaces_file_only_through_explicit_structural_steps() {
    let diff = WorkspaceDiff::compare(
        local("/left"),
        local("/right"),
        vec![dir("config"), file("config/new.txt", 5)],
        vec![file("config", 1)],
    );
    let policy = SyncPolicy {
        conflicts: ConflictPolicy::PreferSource,
        ..SyncPolicy::default()
    };
    let compiled = SyncExecutionCompiler::compile(
        executable(&diff, policy, true),
        &diff,
        &default_registry(),
        &SyncExecutorMatrix::local_only(),
    )
    .unwrap();

    assert_eq!(compiled.steps().len(), 3);
    assert!(matches!(
        &compiled.steps()[0].step,
        PhysicalSyncStep::DeleteFile { relative_path, .. } if relative_path == "config"
    ));
    assert!(matches!(
        &compiled.steps()[1].step,
        PhysicalSyncStep::EnsureDirectory { relative_path, .. } if relative_path == "config"
    ));
    assert!(matches!(
        &compiled.steps()[2].step,
        PhysicalSyncStep::TransferFile { relative_path, .. } if relative_path == "config/new.txt"
    ));
}

#[test]
fn update_mode_refuses_implicit_recursive_directory_replacement() {
    let diff = WorkspaceDiff::compare(
        local("/left"),
        local("/right"),
        vec![file("config", 7)],
        vec![dir("config"), file("config/important.txt", 9)],
    );
    let policy = SyncPolicy {
        conflicts: ConflictPolicy::PreferSource,
        ..SyncPolicy::default()
    };
    let error = SyncExecutionCompiler::compile(
        executable(&diff, policy, true),
        &diff,
        &default_registry(),
        &SyncExecutorMatrix::local_only(),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        SyncCompileError::StructuralReplacementRequiresDeletes { ref path } if path == "config"
    ));
}

#[test]
fn local_to_local_file_compiles_to_native_transfer() {
    let diff = WorkspaceDiff::compare(
        local("/left"),
        local("/right"),
        vec![file("a.txt", 1)],
        Vec::<WorkspaceEntry>::new(),
    );
    let compiled = SyncExecutionCompiler::compile(
        executable(&diff, SyncPolicy::default(), false),
        &diff,
        &default_registry(),
        &SyncExecutorMatrix::local_only(),
    )
    .unwrap();

    assert!(matches!(
        &compiled.steps()[0].step,
        PhysicalSyncStep::TransferFile { transfer, .. }
            if transfer.method == TransferMethod::Native
    ));
}

#[test]
fn local_to_sftp_regular_file_compiles_when_sftp_executor_is_available() {
    let diff = WorkspaceDiff::compare(
        local("/left"),
        sftp("prod", "/right"),
        vec![file("a.txt", 1)],
        Vec::<WorkspaceEntry>::new(),
    );
    let compiled = SyncExecutionCompiler::compile(
        executable(&diff, SyncPolicy::default(), false),
        &diff,
        &default_registry(),
        &remote_matrix("prod"),
    )
    .unwrap();

    assert!(matches!(
        &compiled.steps()[0].step,
        PhysicalSyncStep::TransferFile { transfer, .. }
            if transfer.method == TransferMethod::Sftp
    ));
}

#[test]
fn sftp_to_local_regular_file_compiles_when_sftp_executor_is_available() {
    let diff = WorkspaceDiff::compare(
        sftp("prod", "/left"),
        local("/right"),
        vec![file("a.txt", 1)],
        Vec::<WorkspaceEntry>::new(),
    );
    let compiled = SyncExecutionCompiler::compile(
        executable(&diff, SyncPolicy::default(), false),
        &diff,
        &default_registry(),
        &remote_matrix("prod"),
    )
    .unwrap();

    assert!(matches!(
        &compiled.steps()[0].step,
        PhysicalSyncStep::TransferFile { transfer, .. }
            if transfer.method == TransferMethod::Sftp
    ));
}

#[test]
fn sftp_directory_creation_is_compile_time_gated() {
    let diff = WorkspaceDiff::compare(
        local("/left"),
        sftp("prod", "/right"),
        vec![dir("new-dir")],
        Vec::<WorkspaceEntry>::new(),
    );
    let error = SyncExecutionCompiler::compile(
        executable(&diff, SyncPolicy::default(), false),
        &diff,
        &default_registry(),
        &remote_matrix("prod"),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        SyncCompileError::UnsupportedDirectoryMutation { ref path, .. } if path == "new-dir"
    ));
}

#[test]
fn compiler_failure_creates_no_journal_record() {
    let journal_dir = tempfile::tempdir().unwrap();
    let journal = OperationJournal::open(journal_dir.path().join("ops.jsonl")).unwrap();
    let diff = WorkspaceDiff::compare(
        local("/left"),
        sftp("prod", "/right"),
        vec![dir("new-dir")],
        Vec::<WorkspaceEntry>::new(),
    );

    let result = SyncExecutionCompiler::compile(
        executable(&diff, SyncPolicy::default(), false),
        &diff,
        &default_registry(),
        &remote_matrix("prod"),
    );

    assert!(result.is_err());
    assert!(journal.read_all().unwrap().is_empty());
}

#[test]
fn execution_modules_expose_no_raw_logical_plan_executor() {
    let runtime = include_str!("../src/workspace_sync_executor/runtime.rs");
    let gate = include_str!("../src/workspace_sync_execution.rs");

    assert!(runtime.contains("plan: CompiledSyncPlan"));
    assert!(!runtime.contains("plan: WorkspaceSyncPlan"));
    assert!(!runtime.contains("plan: FrozenWorkspaceSyncPlan"));
    assert!(!gate.contains("pub async fn execute("));
}
