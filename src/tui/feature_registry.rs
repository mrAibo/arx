//! Private TUI feature-controller registry (PACK R).
//!
//! Proof consumers registered by `ActionId`: Quick Actions (SHA-256 / touch /
//! tar.gz), Storage Inspector, and SSH Host Manager. This is an internal
//! binary-side routing table — no public feature trait, no feature-id type,
//! no trait objects. The stable `ActionId` is the only cross-layer key.

use super::quick_actions;
use arx::app::{Action, ActionId, AppState};
use arx::effect_dispatcher::EffectDispatcher;
use arx::vfs::Entry;

/// Narrow context handed to registered action handlers.
///
/// Deliberately small: only what the proof consumers actually need. Runtime
/// authorities (scanner, sync runtime, pane loader, transfer runtime, editor,
/// terminal session) stay owned by the central dispatch.
pub(crate) struct FeatureActionContext<'a> {
    pub state: &'a mut AppState,
    pub focused: Option<&'a Entry>,
    pub active_entries: &'a [&'a Entry],
    pub effect_dispatcher: &'a EffectDispatcher,
}

type ActionHandler = fn(&mut FeatureActionContext, &Action) -> bool;

pub(crate) struct FeatureControllerRegistration {
    /// Actions owned exclusively by this controller.
    pub actions: &'static [ActionId],
    /// Activation handler; returns true when the action was consumed.
    pub handle_action: ActionHandler,
}

fn quick_action_handler(ctx: &mut FeatureActionContext, action: &Action) -> bool {
    let FeatureActionContext {
        state,
        focused,
        active_entries,
        effect_dispatcher,
    } = ctx;
    quick_actions::handle_action(
        state,
        action,
        focused.cloned().as_ref(),
        active_entries,
        effect_dispatcher,
    )
}

#[cfg(target_os = "linux")]
fn storage_handler(ctx: &mut FeatureActionContext, _action: &Action) -> bool {
    match arx::storage_inspector_ui::launch_storage_inspector(ctx.state) {
        Ok(id) => {
            ctx.state.message = Some(format!("Storage Inspector: {id}"));
        }
        Err(message) => {
            ctx.state.message = Some(message);
        }
    }
    true
}

fn ssh_hosts_handler(ctx: &mut FeatureActionContext, _action: &Action) -> bool {
    // Existing overlay open path stays the authority: it reloads managed hosts
    // and resets status/cursor.
    ctx.state.toggle_overlay(arx::app::OverlayKind::SshHosts);
    true
}

const QUICK_ACTIONS: &[ActionId] = &[
    ActionId::ComputeSha256,
    ActionId::TouchFile,
    ActionId::CompressTarGz,
];

#[cfg(target_os = "linux")]
const STORAGE_ACTIONS: &[ActionId] = &[ActionId::OpenStorageInspector];

const SSH_HOSTS_ACTIONS: &[ActionId] = &[ActionId::OpenSshHosts];

/// THE controller registration table, keyed directly by ActionId.
static CONTROLLERS: &[FeatureControllerRegistration] = &[
    FeatureControllerRegistration {
        actions: QUICK_ACTIONS,
        handle_action: quick_action_handler,
    },
    #[cfg(target_os = "linux")]
    FeatureControllerRegistration {
        actions: STORAGE_ACTIONS,
        handle_action: storage_handler,
    },
    FeatureControllerRegistration {
        actions: SSH_HOSTS_ACTIONS,
        handle_action: ssh_hosts_handler,
    },
];

/// Route a registered-feature action to its controller.
///
/// Returns true when the action was claimed and handled. Unregistered actions
/// are not claimed and fall through to the remaining central dispatch.
pub(crate) fn handle_registered_action(ctx: &mut FeatureActionContext, action: &Action) -> bool {
    let id = action.id();
    let Some(controller) = CONTROLLERS.iter().find(|c| c.actions.contains(&id)) else {
        return false;
    };
    (controller.handle_action)(ctx, action)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    fn dispatcher() -> EffectDispatcher {
        let (dispatcher, _rx) = EffectDispatcher::channel(arx::vfs::default_registry());
        dispatcher
    }

    #[test]
    fn r_each_proof_action_maps_to_exactly_one_controller() {
        for id in [
            ActionId::ComputeSha256,
            ActionId::TouchFile,
            ActionId::CompressTarGz,
            ActionId::OpenStorageInspector,
            ActionId::OpenSshHosts,
        ] {
            assert_eq!(
                CONTROLLERS
                    .iter()
                    .filter(|c| c.actions.contains(&id))
                    .count(),
                1,
                "{id:?} must map to exactly one controller"
            );
        }
    }

    #[test]
    fn r_unregistered_action_is_not_claimed() {
        let mut dispatcher_box = dispatcher();
        let mut state = AppState::default();
        let mut ctx = FeatureActionContext {
            state: &mut state,
            focused: None,
            active_entries: &[],
            effect_dispatcher: &mut dispatcher_box,
        };
        assert!(!handle_registered_action(&mut ctx, &Action::Quit));
        assert!(!handle_registered_action(&mut ctx, &Action::Copy));
    }

    #[test]
    fn r_ssh_activation_uses_existing_overlay_seam() {
        let mut dispatcher_box = dispatcher();
        let mut state = AppState::default();
        assert!(!state.show_ssh_hosts);
        let mut ctx = FeatureActionContext {
            state: &mut state,
            focused: None,
            active_entries: &[],
            effect_dispatcher: &mut dispatcher_box,
        };
        assert!(handle_registered_action(&mut ctx, &Action::OpenSshHosts));
        assert!(
            ctx.state.show_ssh_hosts,
            "activation must go through the existing toggle_overlay path"
        );
    }
}
