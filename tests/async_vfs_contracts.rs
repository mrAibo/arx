use std::path::PathBuf;

use arx::app::{AppState, Pane};
use arx::jobs::{JobEvent, JobKind, JobManager, JobStatus};
use arx::services::{
    PaneLoadId, PaneLoadPurpose, PaneLoader, WorkspaceScanOptions, scan_workspace,
};
use arx::vfs::{Location, ProviderRegistry};

#[test]
fn tui_does_not_reinstall_thread_local_provider_registry() {
    let tui = include_str!("../src/tui.rs");
    assert!(
        !tui.contains("set_global_registry"),
        "TUI regressed to thread-local ProviderRegistry ownership"
    );
}

#[test]
fn tui_initial_loading_uses_pane_loader() {
    let tui = include_str!("../src/tui.rs");
    assert!(
        tui.contains("PaneLoader::channel"),
        "async PaneLoader is no longer wired into the runtime"
    );
}

#[test]
fn tui_has_no_synchronous_directory_reload_helper_left() {
    let tui = include_str!("../src/tui.rs");
    assert!(
        !tui.contains("fn load_entries("),
        "synchronous directory loading helper returned to TUI"
    );
    assert!(
        !tui.contains("= load_entries("),
        "a navigation path still performs synchronous VFS loading"
    );
}

#[test]
fn provider_instances_distinguish_sftp_hosts() {
    let prod = Location::Sftp {
        host: "prod".into(),
        path: "/srv".into(),
    };
    let staging = Location::Sftp {
        host: "staging".into(),
        path: "/srv".into(),
    };

    assert_ne!(
        ProviderRegistry::instance_key_for_location(&prod),
        ProviderRegistry::instance_key_for_location(&staging)
    );
}

#[test]
fn sftp_provider_is_constructed_as_a_reusable_host_scoped_instance() {
    let source = include_str!("../src/vfs/sftp.rs");
    assert!(
        source.contains("connection: Mutex<Option<"),
        "SFTP provider lost its host-scoped reusable connection"
    );
    assert!(
        source.contains("list_pooled"),
        "SFTP list path regressed to per-directory connection creation"
    );
}

#[test]
fn newer_pane_load_generation_invalidates_old_response() {
    let mut state = AppState::default();
    let location = state.left.location.clone();
    let first = PaneLoadId(100);
    let second = PaneLoadId(101);

    state.register_pane_load(
        Pane::Left,
        first,
        location.clone(),
        PaneLoadPurpose::Refresh,
    );
    state.register_pane_load(
        Pane::Left,
        second,
        location.clone(),
        PaneLoadPurpose::Refresh,
    );

    assert!(!state.accepts_pane_load(Pane::Left, first, &location));
    assert!(state.accepts_pane_load(Pane::Left, second, &location));
}

#[test]
fn navigation_invalidates_old_pane_load_even_if_id_is_latest() {
    let mut state = AppState::default();
    let old_location = state.left.location.clone();
    let id = PaneLoadId(200);
    state.register_pane_load(
        Pane::Left,
        id,
        old_location.clone(),
        PaneLoadPurpose::Refresh,
    );
    state.left.location = Location::Local(PathBuf::from("/different"));

    assert!(!state.accepts_pane_load(Pane::Left, id, &old_location));
}

#[test]
fn pending_navigation_target_is_not_the_committed_location() {
    let mut state = AppState::default();
    let current = state.left.location.clone();
    let target = Location::Sftp {
        host: "offline-host".into(),
        path: "/srv".into(),
    };
    let id = PaneLoadId(250);

    state.register_pane_load(
        Pane::Left,
        id,
        target.clone(),
        PaneLoadPurpose::Navigate {
            remember_current: true,
        },
    );

    // Registering an async navigation request must not optimistically replace
    // the visible location. A failed response therefore needs no rollback.
    assert_eq!(state.left.location, current);
    assert_ne!(state.left.location, target);
    assert!(state.accepts_pane_load(Pane::Left, id, &target));
}

#[test]
fn new_navigation_request_supersedes_previous_target_and_generation() {
    let mut state = AppState::default();
    let first_target = Location::Sftp {
        host: "prod".into(),
        path: "/srv".into(),
    };
    let second_target = Location::Sftp {
        host: "staging".into(),
        path: "/srv".into(),
    };

    state.register_pane_load(
        Pane::Left,
        PaneLoadId(300),
        first_target.clone(),
        PaneLoadPurpose::Navigate {
            remember_current: true,
        },
    );
    state.register_pane_load(
        Pane::Left,
        PaneLoadId(301),
        second_target.clone(),
        PaneLoadPurpose::Navigate {
            remember_current: true,
        },
    );

    assert!(!state.accepts_pane_load(Pane::Left, PaneLoadId(300), &first_target));
    assert!(state.accepts_pane_load(Pane::Left, PaneLoadId(301), &second_target));
}

#[tokio::test]
async fn pane_loader_reads_local_directory_without_location_list_bridge() {
    let dir = tempfile::tempdir().unwrap();
    tokio::fs::write(dir.path().join("hello.txt"), b"hello")
        .await
        .unwrap();

    let (loader, mut rx) = PaneLoader::channel(arx::vfs::default_registry());
    let location = Location::Local(dir.path().to_path_buf());
    let id = loader.load(Pane::Right, location.clone(), PaneLoadPurpose::Refresh);

    let response = rx.recv().await.unwrap();

    assert_eq!(response.id, id);
    assert_eq!(response.location, location);
    let entries = response.result.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "hello.txt");
}

#[tokio::test]
async fn recursive_workspace_scan_obeys_depth_bound() {
    let dir = tempfile::tempdir().unwrap();
    tokio::fs::create_dir_all(dir.path().join("a/b/c"))
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("a/b/c/deep.txt"), b"x")
        .await
        .unwrap();

    let cancel = std::sync::atomic::AtomicBool::new(false);
    let entries = scan_workspace(
        &arx::vfs::default_registry(),
        &Location::Local(dir.path().to_path_buf()),
        WorkspaceScanOptions {
            max_depth: 1,
            max_entries: 100,
        },
        &cancel,
    )
    .await
    .unwrap();

    assert!(entries.iter().any(|entry| entry.relative_path == "a"));
    assert!(entries.iter().any(|entry| entry.relative_path == "a/b"));
    assert!(
        !entries
            .iter()
            .any(|entry| entry.relative_path == "a/b/c/deep.txt")
    );
}

#[test]
fn cancelled_pending_job_never_regresses_to_running() {
    let manager = JobManager::new();
    let job = manager.create_job("contract", JobKind::Transfer, "cancel-me", None, None);
    let token = job.cancel.clone();

    assert_eq!(job.status, JobStatus::Pending);
    assert!(manager.cancel(&job.id));
    assert!(token.load(std::sync::atomic::Ordering::Relaxed));
    assert_eq!(manager.get(&job.id).unwrap().status, JobStatus::Cancelling);

    assert!(manager.apply_event(&JobEvent::Running { id: job.id.clone() }));
    assert_eq!(manager.get(&job.id).unwrap().status, JobStatus::Cancelling);
}

#[test]
fn transfer_worker_uses_same_cancel_token_owned_by_job_manager() {
    let tui = include_str!("../src/tui.rs");
    assert!(tui.contains("let cancel = job.cancel.clone();"));
    assert!(tui.contains("job_manager.cancel(&id)"));
    assert!(!tui.contains("state.jobs.push("));
}

#[test]
fn async_vfs_runtime_does_not_import_legacy_vfsops_in_tui() {
    let tui = include_str!("../src/tui.rs");
    assert!(
        !tui.contains("VfsOps"),
        "legacy Location::list dispatch leaked back into TUI imports"
    );
}

#[test]
fn legacy_thread_local_registry_bridge_is_not_used_by_runtime() {
    let tui = include_str!("../src/tui.rs");
    assert!(!tui.contains("with_registry_mut"));
    assert!(!tui.contains("PROVIDER_REGISTRY"));
}

#[test]
fn workspace_sync_execution_stays_gated_behind_application_controller() {
    let tui = include_str!("../src/tui.rs");
    let start = tui.find("fn prepare_workspace_sync(").unwrap();
    let end = tui[start..]
        .find("fn start_workspace_scan(")
        .map(|offset| start + offset)
        .unwrap();
    let sync_ui = &tui[start..end];

    assert!(tui.contains("WorkspaceSyncController"));
    assert!(sync_ui.contains("sync.controller.freeze("));
    assert!(sync_ui.contains(".launch("));
    assert!(!tui.contains("execution intentionally disabled"));
    for forbidden in [
        "SyncExecutionCompiler",
        "SyncPlanValidator",
        "TransferPlanner",
        "MutationService",
        "SyncConfirmationToken",
        "execute_transfer",
    ] {
        assert!(
            !sync_ui.contains(forbidden),
            "workspace sync presentation re-entered backend planning through {forbidden}"
        );
    }
}
