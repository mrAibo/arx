from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f'{label}: expected 1 match, found {count}')
    return text.replace(old, new, 1)

path = Path('src/app/workspace_sync_ux.rs')
text = path.read_text()
text = replace_once(
    text,
    '''    pub fn is_job_flow(&self) -> bool {
        self.job_id().is_some()
    }
''',
    '''    pub fn is_job_flow(&self) -> bool {
        self.job_id().is_some()
    }

    /// While an immutable plan is launching or a Job is still active, actions
    /// that rebuild/disable the workspace comparison are presentation-unsafe.
    /// Normal pane navigation remains available; late verification correlation
    /// protects those independent workspace changes.
    pub fn is_locked_flow(&self) -> bool {
        matches!(
            self,
            Self::Launching { .. }
                | Self::Queued { .. }
                | Self::Running { .. }
                | Self::Cancelling { .. }
                | Self::Verifying { .. }
        )
    }
''',
    'locked-flow helper',
)
marker = '''    #[test]
    fn only_runtime_backed_states_expose_a_job_id() {'''
insert = '''    #[test]
    fn launching_and_active_job_states_lock_preview_mutation() {
        let launching = WorkspaceSyncUxState::Launching {
            plan_id: crate::workspace_sync_execution::SyncPlanValidator::freeze(
                &crate::workspace_sync::WorkspaceSyncPlan::build(
                    &crate::workspace_sync::WorkspaceDiff::compare(
                        crate::vfs::Location::Local("/left".into()),
                        crate::vfs::Location::Local("/right".into()),
                        vec![crate::workspace_sync::WorkspaceEntry {
                            relative_path: "a.txt".into(),
                            fingerprint: crate::workspace_sync::WorkspaceFingerprint {
                                kind: crate::vfs::EntryKind::File,
                                size: Some(1),
                                modified_unix_ms: None,
                                content_hash: None,
                            },
                        }],
                        Vec::new(),
                    ),
                    crate::workspace_sync::SyncPolicy::default(),
                ),
                &crate::workspace_sync::WorkspaceDiff::compare(
                    crate::vfs::Location::Local("/left".into()),
                    crate::vfs::Location::Local("/right".into()),
                    vec![crate::workspace_sync::WorkspaceEntry {
                        relative_path: "a.txt".into(),
                        fingerprint: crate::workspace_sync::WorkspaceFingerprint {
                            kind: crate::vfs::EntryKind::File,
                            size: Some(1),
                            modified_unix_ms: None,
                            content_hash: None,
                        },
                    }],
                    Vec::new(),
                ),
                &crate::vfs::default_registry(),
            )
            .unwrap()
            .id(),
        };
        assert!(launching.is_locked_flow());
        assert!(WorkspaceSyncUxState::Running {
            job_id: "sync-1".into()
        }
        .is_locked_flow());
        assert!(!WorkspaceSyncUxState::Preview { plan_id: None }.is_locked_flow());
        assert!(!WorkspaceSyncUxState::Finished {
            job_id: "sync-1".into()
        }
        .is_locked_flow());
    }

'''
if text.count(marker) != 1:
    raise SystemExit('locked-flow test anchor mismatch')
text = text.replace(marker, insert + marker, 1)
path.write_text(text)

path = Path('src/app/actions.rs')
text = path.read_text()
text = replace_once(
    text,
    '''                    super::WorkspaceSyncUxState::Queued { .. }
                    | super::WorkspaceSyncUxState::Running { .. }
''',
    '''                    super::WorkspaceSyncUxState::Launching { .. }
                    | super::WorkspaceSyncUxState::Queued { .. }
                    | super::WorkspaceSyncUxState::Running { .. }
''',
    'launching input lock',
)
marker = '''    #[test]
    fn browser_is_the_default_input_context() {'''
insert = '''    #[test]
    fn launching_sync_owns_input_without_preview_execute_binding() {
        let diff = crate::workspace_sync::WorkspaceDiff::compare(
            crate::vfs::Location::Local("/left".into()),
            crate::vfs::Location::Local("/right".into()),
            vec![crate::workspace_sync::WorkspaceEntry {
                relative_path: "a.txt".into(),
                fingerprint: crate::workspace_sync::WorkspaceFingerprint {
                    kind: crate::vfs::EntryKind::File,
                    size: Some(1),
                    modified_unix_ms: None,
                    content_hash: None,
                },
            }],
            Vec::new(),
        );
        let plan = crate::workspace_sync::WorkspaceSyncPlan::build(
            &diff,
            crate::workspace_sync::SyncPolicy::default(),
        );
        let plan_id = crate::workspace_sync_execution::SyncPlanValidator::freeze(
            &plan,
            &diff,
            &crate::vfs::default_registry(),
        )
        .unwrap()
        .id();
        let mut state = AppState::default();
        state.remote_workspace.preview_open = true;
        state.remote_workspace.ux = super::WorkspaceSyncUxState::Launching { plan_id };
        assert_eq!(state.input_context(), InputContext::SyncJob);
    }

'''
if text.count(marker) != 1:
    raise SystemExit('launching input-context test anchor mismatch')
text = text.replace(marker, insert + marker, 1)
path.write_text(text)

path = Path('src/tui.rs')
text = path.read_text()
# Guard the workspace-control actions before their existing branches. Keeping
# the guard local to the dispatcher makes keyboard, Command Center and future
# context-menu invocation obey the same lock.
old = '''        Action::ToggleWorkspaceComparison => {
            if state.remote_workspace.enabled {
                state.remote_workspace.disable();
                state.show_diff = false;
                state.message = Some("Remote Workspace comparison off".into());
            } else {
                start_workspace_scan(workspace_scanner, state, false);
            }
        }
        Action::PreviewWorkspaceSync => {
            start_workspace_scan(workspace_scanner, state, true);
            state.open_overlay(OverlayKind::SyncPreview);
        }
        Action::ReverseWorkspaceDirection => state.remote_workspace.reverse_direction(),
        Action::ToggleWorkspaceSyncMode => state.remote_workspace.toggle_mode(),
'''
new = '''        Action::ToggleWorkspaceComparison
        | Action::PreviewWorkspaceSync
        | Action::ReverseWorkspaceDirection
        | Action::ToggleWorkspaceSyncMode
            if state.remote_workspace.ux.is_locked_flow() =>
        {
            state.open_overlay(OverlayKind::SyncPreview);
            state.message = Some(
                "Workspace sync is already preparing or active; the current immutable plan is locked."
                    .into(),
            );
        }
        Action::ToggleWorkspaceComparison => {
            if state.remote_workspace.enabled {
                state.remote_workspace.disable();
                state.show_diff = false;
                state.message = Some("Remote Workspace comparison off".into());
            } else {
                start_workspace_scan(workspace_scanner, state, false);
            }
        }
        Action::PreviewWorkspaceSync => {
            start_workspace_scan(workspace_scanner, state, true);
            state.open_overlay(OverlayKind::SyncPreview);
        }
        Action::ReverseWorkspaceDirection => state.remote_workspace.reverse_direction(),
        Action::ToggleWorkspaceSyncMode => state.remote_workspace.toggle_mode(),
'''
text = replace_once(text, old, new, 'workspace control lock guard')
path.write_text(text)

path = Path('tests/refactor_contracts.rs')
text = path.read_text()
marker = '''#[test]
fn sync_preview_ui_routes_execution_through_workspace_sync_controller() {'''
insert = '''#[test]
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

'''
if text.count(marker) != 1:
    raise SystemExit('launch-lock contract anchor mismatch')
text = text.replace(marker, insert + marker, 1)
path.write_text(text)
