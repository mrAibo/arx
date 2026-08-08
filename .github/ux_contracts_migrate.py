from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f'{label}: expected 1 match, found {count}')
    return text.replace(old, new, 1)

# Availability belongs to the application action layer. It decides whether an
# action makes sense for the current presentation state; the controller still
# performs the authoritative freeze/compile/capability checks asynchronously.
path = Path('src/app/availability.rs')
text = path.read_text()
text = replace_once(
    text,
    'use super::{ActionId, AppState};\n',
    'use super::{ActionId, AppState, WorkspaceSyncUxState};\n',
    'availability UX import',
)
text = replace_once(
    text,
    '''    pub passive_capabilities: CapabilitySet,
    pub selection_count: usize,
}''',
    '''    pub passive_capabilities: CapabilitySet,
    pub selection_count: usize,
    pub sync_execute_ready: bool,
    pub sync_confirmation_ready: bool,
    pub sync_cancel_ready: bool,
    pub sync_details_ready: bool,
    pub sync_return_preview_ready: bool,
}''',
    'availability context fields',
)
text = replace_once(
    text,
    '''        Self {
            active_provider,
            passive_provider,
            active_capabilities,
            passive_capabilities,
            selection_count: state.selected.len(),
        }''',
    '''        let sync_execute_ready = matches!(
            state.remote_workspace.ux,
            WorkspaceSyncUxState::Preview { .. }
        ) && state
            .remote_workspace
            .plan
            .as_ref()
            .is_some_and(|plan| plan.can_execute());
        let sync_confirmation_ready = matches!(
            state.remote_workspace.ux,
            WorkspaceSyncUxState::ConfirmationRequired { .. }
        ) && state.remote_workspace.frozen_plan.is_some();
        let sync_cancel_ready = matches!(
            state.remote_workspace.ux,
            WorkspaceSyncUxState::Queued { .. } | WorkspaceSyncUxState::Running { .. }
        );
        let sync_details_ready = state.remote_workspace.ux.is_job_flow();
        let sync_return_preview_ready = matches!(
            state.remote_workspace.ux,
            WorkspaceSyncUxState::ConfirmationRequired { .. }
                | WorkspaceSyncUxState::Blocked { .. }
                | WorkspaceSyncUxState::Finished { .. }
        );

        Self {
            active_provider,
            passive_provider,
            active_capabilities,
            passive_capabilities,
            selection_count: state.selected.len(),
            sync_execute_ready,
            sync_confirmation_ready,
            sync_cancel_ready,
            sync_details_ready,
            sync_return_preview_ready,
        }''',
    'availability from state',
)
text = replace_once(
    text,
    '''        ActionId::BeginChown if ctx.active_provider != ProviderId::Local => {
            ActionAvailability::Disabled {
                reason: "Owner changes are currently local-only".into(),
            }
        }
        _ => ActionAvailability::Available,
''',
    '''        ActionId::BeginChown if ctx.active_provider != ProviderId::Local => {
            ActionAvailability::Disabled {
                reason: "Owner changes are currently local-only".into(),
            }
        }
        ActionId::ExecuteWorkspaceSync if !ctx.sync_execute_ready => ActionAvailability::Disabled {
            reason: "Workspace sync needs a current conflict-free preview".into(),
        },
        ActionId::ConfirmWorkspaceSync if !ctx.sync_confirmation_ready => {
            ActionAvailability::Disabled {
                reason: "No destructive frozen sync plan is awaiting confirmation".into(),
            }
        }
        ActionId::CancelWorkspaceSync if !ctx.sync_cancel_ready => ActionAvailability::Disabled {
            reason: "No queued or running workspace sync can be cancelled".into(),
        },
        ActionId::ShowWorkspaceSyncDetails if !ctx.sync_details_ready => {
            ActionAvailability::Disabled {
                reason: "No workspace sync Job is active or available".into(),
            }
        }
        ActionId::ReturnToWorkspaceSyncPreview if !ctx.sync_return_preview_ready => {
            ActionAvailability::Disabled {
                reason: "There is no confirmation, result, or blocked sync view to leave".into(),
            }
        }
        _ => ActionAvailability::Available,
''',
    'availability action rules',
)
text = replace_once(
    text,
    '''            passive_capabilities: LOCAL_CAPABILITIES,
            selection_count: 0,
        }''',
    '''            passive_capabilities: LOCAL_CAPABILITIES,
            selection_count: 0,
            sync_execute_ready: false,
            sync_confirmation_ready: false,
            sync_cancel_ready: false,
            sync_details_ready: false,
            sync_return_preview_ready: false,
        }''',
    'availability test helper',
)
marker = '''    #[test]
    fn ordinary_navigation_actions_remain_available() {'''
insert = '''    #[test]
    fn workspace_sync_actions_follow_presentation_state() {
        let mut ctx = context(ProviderId::Local, LOCAL_CAPABILITIES);
        assert!(matches!(
            action_availability(ActionId::ExecuteWorkspaceSync, &ctx),
            ActionAvailability::Disabled { .. }
        ));

        ctx.sync_execute_ready = true;
        assert_eq!(
            action_availability(ActionId::ExecuteWorkspaceSync, &ctx),
            ActionAvailability::Available
        );

        ctx.sync_confirmation_ready = true;
        assert_eq!(
            action_availability(ActionId::ConfirmWorkspaceSync, &ctx),
            ActionAvailability::Available
        );

        ctx.sync_cancel_ready = true;
        assert_eq!(
            action_availability(ActionId::CancelWorkspaceSync, &ctx),
            ActionAvailability::Available
        );
        ctx.sync_cancel_ready = false;
        assert!(matches!(
            action_availability(ActionId::CancelWorkspaceSync, &ctx),
            ActionAvailability::Disabled { .. }
        ));

        ctx.sync_details_ready = true;
        assert_eq!(
            action_availability(ActionId::ShowWorkspaceSyncDetails, &ctx),
            ActionAvailability::Available
        );
        ctx.sync_return_preview_ready = true;
        assert_eq!(
            action_availability(ActionId::ReturnToWorkspaceSyncPreview, &ctx),
            ActionAvailability::Available
        );
    }

'''
if text.count(marker) != 1:
    raise SystemExit('availability test anchor mismatch')
text = text.replace(marker, insert + marker, 1)
path.write_text(text)

# Phase0 deliberately asserted that recursive scanning could not execute sync.
# #34 replaces that obsolete invariant with the real safety boundary: TUI may
# launch, but only through WorkspaceSyncController and never by re-entering the
# compiler/planner/mutation internals itself.
path = Path('tests/async_vfs_contracts.rs')
text = path.read_text()
old = '''#[test]
fn workspace_sync_execution_remains_gated_after_recursive_scanner_wiring() {
    let tui = include_str!("../src/tui.rs");
    assert!(
        tui.contains("execution intentionally disabled"),
        "recursive scan work accidentally enabled destructive sync execution"
    );
}
'''
new = '''#[test]
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
    assert!(sync_ui.contains("controller.launch("));
    assert!(!tui.contains("execution intentionally disabled"));
    for forbidden in [
        "SyncExecutionCompiler",
        "SyncPlanValidator",
        "TransferPlanner",
        "SyncConfirmationToken",
        "execute_transfer",
    ] {
        assert!(
            !sync_ui.contains(forbidden),
            "workspace sync presentation re-entered backend planning through {forbidden}"
        );
    }
}
'''
text = replace_once(text, old, new, 'async VFS obsolete sync gate contract')
path.write_text(text)

path = Path('tests/refactor_contracts.rs')
text = path.read_text()
old = '''#[test]
fn sync_preview_ui_does_not_contain_an_execution_shortcut_yet() {
    let tui = include_str!("../src/tui.rs");
    assert!(
        tui.contains("execution intentionally disabled"),
        "execution was enabled before the sync executor safety gate was introduced"
    );
}
'''
new = '''#[test]
fn sync_preview_ui_routes_execution_through_workspace_sync_controller() {
    let tui = include_str!("../src/tui.rs");
    let start = tui.find("fn prepare_workspace_sync(").unwrap();
    let end = tui[start..]
        .find("fn start_workspace_scan(")
        .map(|offset| start + offset)
        .unwrap();
    let sync_ui = &tui[start..end];

    assert!(sync_ui.contains("sync.controller.freeze("));
    assert!(sync_ui.contains("controller.launch("));
    assert!(!tui.contains("execution intentionally disabled"));
    assert!(tui.contains("Audit record finalization failed"));
    assert!(tui.contains("No mismatch was proven"));
    assert!(tui.contains("Cancelling…"));
    for forbidden in [
        "SyncExecutionCompiler",
        "SyncPlanValidator",
        "TransferPlanner",
        "SyncConfirmationToken",
        "execute_transfer",
    ] {
        assert!(!sync_ui.contains(forbidden));
    }
}
'''
text = replace_once(text, old, new, 'refactor obsolete sync preview contract')
path.write_text(text)
