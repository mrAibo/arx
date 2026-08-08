use super::{ActionId, AppState, WorkspaceSyncUxState};
use crate::vfs::{Capability, CapabilitySet, ProviderId, capabilities::builtin_capabilities};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionAvailability {
    Available,
    Disabled { reason: String },
    Hidden,
}

impl ActionAvailability {
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Disabled { reason } => Some(reason),
            Self::Available | Self::Hidden => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionContext {
    pub active_provider: ProviderId,
    pub passive_provider: ProviderId,
    pub active_capabilities: CapabilitySet,
    pub passive_capabilities: CapabilitySet,
    pub selection_count: usize,
    pub sync_execute_ready: bool,
    pub sync_confirmation_ready: bool,
    pub sync_cancel_ready: bool,
    pub sync_details_ready: bool,
    pub sync_return_preview_ready: bool,
}

impl ActionContext {
    pub fn from_state(state: &AppState) -> Self {
        let active_provider = state.active_pane().location.provider_id();
        let passive_provider = state.other_pane().location.provider_id();
        let active_capabilities = state
            .registry
            .capabilities(&active_provider)
            .unwrap_or_else(|| builtin_capabilities(active_provider));
        let passive_capabilities = state
            .registry
            .capabilities(&passive_provider)
            .unwrap_or_else(|| builtin_capabilities(passive_provider));

        let sync_execute_ready = matches!(
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
        }
    }
}

fn require_active_capability(
    ctx: &ActionContext,
    capability: Capability,
    label: &str,
) -> ActionAvailability {
    if ctx.active_capabilities.supports(capability) {
        ActionAvailability::Available
    } else {
        ActionAvailability::Disabled {
            reason: format!(
                "{label} is not supported by the active {:?} provider",
                ctx.active_provider
            ),
        }
    }
}

pub fn action_availability(id: ActionId, ctx: &ActionContext) -> ActionAvailability {
    match id {
        ActionId::BeginSymlink => {
            require_active_capability(ctx, Capability::Symlink, "Symbolic links")
        }
        ActionId::BeginChmod => {
            require_active_capability(ctx, Capability::Chmod, "Permission changes")
        }
        // Hard links and chown do not yet have VFS capabilities. Keep them
        // local-only rather than pretending remote providers support them.
        ActionId::BeginHardLink if ctx.active_provider != ProviderId::Local => {
            ActionAvailability::Disabled {
                reason: "Hard links are currently local-only".into(),
            }
        }
        ActionId::BeginChown if ctx.active_provider != ProviderId::Local => {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::capabilities::{LOCAL_CAPABILITIES, SFTP_CAPABILITIES};

    fn context(active_provider: ProviderId, active_capabilities: CapabilitySet) -> ActionContext {
        ActionContext {
            active_provider,
            passive_provider: ProviderId::Local,
            active_capabilities,
            passive_capabilities: LOCAL_CAPABILITIES,
            selection_count: 0,
            sync_execute_ready: false,
            sync_confirmation_ready: false,
            sync_cancel_ready: false,
            sync_details_ready: false,
            sync_return_preview_ready: false,
        }
    }

    #[test]
    fn remote_hard_link_is_disabled_instead_of_failing_late() {
        let ctx = context(ProviderId::Sftp, SFTP_CAPABILITIES);
        assert!(matches!(
            action_availability(ActionId::BeginHardLink, &ctx),
            ActionAvailability::Disabled { .. }
        ));
    }

    #[test]
    fn unavailable_capability_has_explanation() {
        let ctx = context(ProviderId::Sftp, SFTP_CAPABILITIES);
        let availability = action_availability(ActionId::BeginChmod, &ctx);
        assert!(availability.reason().is_some());
    }

    #[test]
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

    #[test]
    fn ordinary_navigation_actions_remain_available() {
        let ctx = context(ProviderId::Local, LOCAL_CAPABILITIES);
        assert_eq!(
            action_availability(ActionId::Enter, &ctx),
            ActionAvailability::Available
        );
    }
}
