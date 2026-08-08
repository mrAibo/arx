use super::{ActionId, AppState};
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

        Self {
            active_provider,
            passive_provider,
            active_capabilities,
            passive_capabilities,
            selection_count: state.selected.len(),
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
    fn ordinary_navigation_actions_remain_available() {
        let ctx = context(ProviderId::Local, LOCAL_CAPABILITIES);
        assert_eq!(
            action_availability(ActionId::Enter, &ctx),
            ActionAvailability::Available
        );
    }
}
