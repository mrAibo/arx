use std::path::PathBuf;

use arx::app::{AppState, OverlayKind};
use arx::effect_dispatcher::{EffectId, EffectLane, EffectScope};
use arx::vfs::{EntryKind, Location};
use arx::workspace_sync::{
    ConflictPolicy, SyncDirection, SyncMode, SyncPolicy, WorkspaceDiff, WorkspaceEntry,
    WorkspaceFingerprint, WorkspaceSyncOperation, WorkspaceSyncPlan,
};

#[test]
fn tui_is_a_presentation_and_input_boundary_not_a_process_adapter() {
    let tui = include_str!("../src/tui.rs");
    assert!(
        !tui.contains("std::process::Command::new"),
        "external process construction leaked back into src/tui.rs"
    );
    assert!(
        !tui.contains("std::fs::"),
        "direct std::fs access leaked back into src/tui.rs"
    );
}

#[test]
fn command_center_does_not_regress_to_string_protocol_routing() {
    let tui = include_str!("../src/tui.rs");
    assert!(
        !tui.contains("fn build_cc_matches"),
        "legacy Vec<(String, String)> Command Center builder returned"
    );
    assert!(
        !tui.contains("fn navigate_to("),
        "legacy stringly-typed Command Center router returned"
    );
}

#[test]
fn overlay_transition_is_exclusive() {
    let mut state = AppState::default();
    state.open_overlay(OverlayKind::Help);
    assert_eq!(state.active_overlay(), Some(OverlayKind::Help));

    state.open_overlay(OverlayKind::Jobs);
    assert_eq!(state.active_overlay(), Some(OverlayKind::Jobs));
    assert!(!state.show_help);
    assert!(state.show_jobs);
}

#[test]
fn latest_effect_in_lane_wins() {
    let mut state = AppState::default();
    let old = EffectId(10);
    let new = EffectId(11);
    state.register_effect(EffectLane::Preview, old);
    state.register_effect(EffectLane::Preview, new);

    assert!(!state.accepts_effect(old, EffectLane::Preview, &EffectScope::Global));
    assert!(state.accepts_effect(new, EffectLane::Preview, &EffectScope::Global));
}

#[test]
fn effect_for_location_is_stale_after_navigation() {
    let mut state = AppState::default();
    let original = state.left.location.clone();
    let id = EffectId(20);
    state.register_effect(EffectLane::LeftPane, id);

    state.left.location = Location::Local(PathBuf::from("/definitely-a-different-location"));

    assert!(!state.accepts_effect(id, EffectLane::LeftPane, &EffectScope::Location(original)));
}

fn fingerprint(size: u64, time: Option<u64>) -> WorkspaceFingerprint {
    WorkspaceFingerprint {
        kind: EntryKind::File,
        size: Some(size),
        modified_unix_ms: time,
        content_hash: None,
    }
}

fn entry(path: &str, size: u64, time: Option<u64>) -> WorkspaceEntry {
    WorkspaceEntry {
        relative_path: path.into(),
        fingerprint: fingerprint(size, time),
    }
}

fn roots() -> (Location, Location) {
    (
        Location::Local(PathBuf::from("/left")),
        Location::Local(PathBuf::from("/right")),
    )
}

#[test]
fn default_sync_policy_is_safe_update_with_manual_conflicts() {
    let policy = SyncPolicy::default();
    assert_eq!(policy.direction, SyncDirection::LeftToRight);
    assert_eq!(policy.mode, SyncMode::Update);
    assert_eq!(policy.conflicts, ConflictPolicy::RequireResolution);
}

#[test]
fn size_only_equality_is_not_trusted() {
    let (left_root, right_root) = roots();
    let diff = WorkspaceDiff::compare(
        left_root,
        right_root,
        vec![entry("same-size.bin", 100, None)],
        vec![entry("same-size.bin", 100, None)],
    );

    assert_eq!(diff.changed_count(), 1);
}

#[test]
fn mirror_destination_only_file_becomes_confirmed_delete() {
    let (left_root, right_root) = roots();
    let diff = WorkspaceDiff::compare(
        left_root,
        right_root,
        Vec::<WorkspaceEntry>::new(),
        vec![entry("old-release.tar", 500, Some(1))],
    );
    let plan = WorkspaceSyncPlan::build(
        &diff,
        SyncPolicy {
            mode: SyncMode::Mirror,
            ..SyncPolicy::default()
        },
    );

    assert!(matches!(
        plan.operations.first(),
        Some(WorkspaceSyncOperation::Delete { .. })
    ));
    assert!(plan.requires_confirmation());
    assert_eq!(plan.destructive_operations, 1);
}

#[test]
fn unresolved_conflict_is_not_executable() {
    let (left_root, right_root) = roots();
    let diff = WorkspaceDiff::compare(
        left_root,
        right_root,
        vec![entry("config.toml", 100, Some(10))],
        vec![entry("config.toml", 120, Some(20))],
    );
    let plan = WorkspaceSyncPlan::build(&diff, SyncPolicy::default());

    assert_eq!(plan.conflicts, 1);
    assert!(!plan.can_execute());
    assert!(plan.requires_confirmation());
}

#[test]
fn launching_workspace_sync_locks_preview_rebuild_actions() {
    let actions = include_str!("../src/app/actions.rs");
    let tui = include_str!("../src/tui.rs");
    assert!(actions.contains("WorkspaceSyncUxState::Launching { .. }"));
    assert!(actions.contains("InputContext::SyncJob"));
    assert!(tui.contains("state.remote_workspace.ux.is_locked_flow()"));
    for action in [
        "Action::ToggleWorkspaceComparison",
        "Action::PreviewWorkspaceSync",
        "Action::ReverseWorkspaceDirection",
        "Action::ToggleWorkspaceSyncMode",
    ] {
        assert!(tui.contains(action));
    }
}

#[test]
fn sync_preview_ui_routes_execution_through_workspace_sync_controller() {
    let tui = include_str!("../src/tui.rs");
    let start = tui.find("fn prepare_workspace_sync(").unwrap();
    let end = tui[start..]
        .find("fn start_workspace_scan(")
        .map(|offset| start + offset)
        .unwrap();
    let sync_ui = &tui[start..end];

    assert!(sync_ui.contains("sync.controller.freeze("));
    assert!(sync_ui.contains(".launch("));
    assert!(!tui.contains("execution intentionally disabled"));
    assert!(tui.contains("Audit record finalization failed"));
    assert!(tui.contains("No mismatch was proven"));
    assert!(tui.contains("Cancelling…"));
    for forbidden in [
        "SyncExecutionCompiler",
        "SyncPlanValidator",
        "TransferPlanner",
        "MutationService",
        "SyncConfirmationToken",
        "execute_transfer",
    ] {
        assert!(!sync_ui.contains(forbidden));
    }
}

#[test]
fn tui_jobs_are_render_snapshots_not_a_second_lifecycle_store() {
    let tui = include_str!("../src/tui.rs");
    assert!(tui.contains("let job_manager = arx::jobs::JobManager::new()"));
    assert!(!tui.contains("state.jobs.push("));
    assert!(!tui.contains("job.status = arx::jobs::JobStatus"));
    assert!(!tui.contains("job.cancel.store("));
}
