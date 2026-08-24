//! Canonical internal action registration (PACK R).
//!
//! One table owns every registered fact about an action: the `Action`, its
//! presentation `ActionMeta`, and its availability policy. Command Center
//! iterates this table directly; `action_meta` resolves through it. There is
//! deliberately no second metadata list and no third availability authority.
//!
//! This is an internal app-layer registry keyed by the stable `ActionId`.
//! It is not a plugin framework: no public feature trait, no dynamic loading,
//! no cross-crate feature identifier.

use super::actions::{Action, ActionCategory, ActionId, ActionMeta};

/// Declarative availability policy evaluated by [`action_availability`].
///
/// `Default` delegates to the centralized evaluator; every other variant
/// replaces the need for bespoke branches in that evaluator for this action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvailabilityPolicy {
    /// Centralized legacy evaluator decides.
    Default,
    /// Unconditionally available.
    Always,
    /// Local-pane-only with a truthful disabled reason.
    LocalOnly(&'static str),
    /// Quick Action: local-only, then requires a focused/selected regular file.
    QuickSha256,
    /// Quick Action: local-only, otherwise available.
    QuickTouch,
    /// Quick Action: local-only, then requires a selection or focused entry.
    QuickCompress,
}

/// One canonical registration entry: action + metadata + availability policy.
#[derive(Debug, Clone, Copy)]
pub struct Registration {
    pub action: Action,
    pub meta: ActionMeta,
    pub policy: AvailabilityPolicy,
}

/// THE canonical registration table. Order is presentation order.
static REGISTRATIONS: &[Registration] = &[
    Registration {
        action: Action::Quit,
        meta: ActionMeta {
            id: ActionId::Quit,
            label: "Quit",
            description: "Exit ARX",
            category: ActionCategory::Application,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::Up,
        meta: ActionMeta {
            id: ActionId::Up,
            label: "Move up",
            description: "Move the cursor up",
            category: ActionCategory::Navigation,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::Down,
        meta: ActionMeta {
            id: ActionId::Down,
            label: "Move down",
            description: "Move the cursor down",
            category: ActionCategory::Navigation,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::Enter,
        meta: ActionMeta {
            id: ActionId::Enter,
            label: "Open",
            description: "Open the focused item",
            category: ActionCategory::Navigation,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::Back,
        meta: ActionMeta {
            id: ActionId::Back,
            label: "Back",
            description: "Navigate to the parent or previous location",
            category: ActionCategory::Navigation,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::SwitchPane,
        meta: ActionMeta {
            id: ActionId::SwitchPane,
            label: "Switch pane",
            description: "Move focus to the opposite pane",
            category: ActionCategory::Panels,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::ToggleSplitPane,
        meta: ActionMeta {
            id: ActionId::ToggleSplitPane,
            label: "Toggle split pane",
            description: "toggle vertical split for active pane",
            category: ActionCategory::Panels,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::ToggleSelect,
        meta: ActionMeta {
            id: ActionId::ToggleSelect,
            label: "Toggle selection",
            description: "Select or deselect the focused item",
            category: ActionCategory::Selection,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::ViewFile,
        meta: ActionMeta {
            id: ActionId::ViewFile,
            label: "View file",
            description: "Open a read-only preview of the focused file",
            category: ActionCategory::Files,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::EditFile,
        meta: ActionMeta {
            id: ActionId::EditFile,
            label: "Edit file",
            description: "Edit the focused supported text file",
            category: ActionCategory::Files,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::Copy,
        meta: ActionMeta {
            id: ActionId::Copy,
            label: "Copy",
            description: "Copy file selection to other pane",
            category: ActionCategory::Files,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::Move,
        meta: ActionMeta {
            id: ActionId::Move,
            label: "Move",
            description: "Move file selection to other pane",
            category: ActionCategory::Files,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::Mkdir,
        meta: ActionMeta {
            id: ActionId::Mkdir,
            label: "New directory",
            description: "Create a new directory",
            category: ActionCategory::Files,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::Delete,
        meta: ActionMeta {
            id: ActionId::Delete,
            label: "Delete",
            description: "Delete the selected items using the provider's safe delete flow",
            category: ActionCategory::Files,
            destructive: true,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::ComputeSha256,
        meta: ActionMeta {
            id: ActionId::ComputeSha256,
            label: "Compute SHA-256",
            description: "Hash focused or selected local regular files",
            category: ActionCategory::Files,
            destructive: false,
        },
        policy: AvailabilityPolicy::QuickSha256,
    },
    Registration {
        action: Action::TouchFile,
        meta: ActionMeta {
            id: ActionId::TouchFile,
            label: "Touch file",
            description: "Create or update a local regular file timestamp",
            category: ActionCategory::Files,
            destructive: false,
        },
        policy: AvailabilityPolicy::QuickTouch,
    },
    Registration {
        action: Action::CompressTarGz,
        meta: ActionMeta {
            id: ActionId::CompressTarGz,
            label: "Compress to tar.gz",
            description: "Create a noclobber tar.gz from focused or selected local entries",
            category: ActionCategory::Files,
            destructive: false,
        },
        policy: AvailabilityPolicy::QuickCompress,
    },
    Registration {
        action: Action::ListTmuxSessions,
        meta: ActionMeta {
            id: ActionId::ListTmuxSessions,
            label: "List tmux sessions",
            description: "Discover and attach to a tmux session",
            category: ActionCategory::Application,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::ToggleEmbeddedTerminal,
        meta: ActionMeta {
            id: ActionId::ToggleEmbeddedTerminal,
            label: "Embedded Terminal",
            description: "Open an embedded terminal in the right pane",
            category: ActionCategory::Panels,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::Refresh,
        meta: ActionMeta {
            id: ActionId::Refresh,
            label: "Refresh",
            description: "Refresh visible entries",
            category: ActionCategory::Navigation,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::OpenCommandCenter,
        meta: ActionMeta {
            id: ActionId::OpenCommandCenter,
            label: "Command Center",
            description: "Search actions, hosts, bookmarks, and history",
            category: ActionCategory::Panels,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::OpenBookmarks,
        meta: ActionMeta {
            id: ActionId::OpenBookmarks,
            label: "Bookmarks",
            description: "Open the bookmarks panel",
            category: ActionCategory::Panels,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::OpenJobs,
        meta: ActionMeta {
            id: ActionId::OpenJobs,
            label: "Jobs",
            description: "Open the background jobs panel",
            category: ActionCategory::Panels,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::OpenSmartTree,
        meta: ActionMeta {
            id: ActionId::OpenSmartTree,
            label: "Smart Tree",
            description: "Open the filtered directory tree",
            category: ActionCategory::Panels,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::OpenInfrastructureCenter,
        meta: ActionMeta {
            id: ActionId::OpenInfrastructureCenter,
            label: "Infrastructure Center",
            description: "Open infrastructure and SSH host status",
            category: ActionCategory::Panels,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::OpenHosts,
        meta: ActionMeta {
            id: ActionId::OpenHosts,
            label: "Hosts",
            description: "Open the SSH hosts panel",
            category: ActionCategory::Panels,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::OpenSshHosts,
        meta: ActionMeta {
            id: ActionId::OpenSshHosts,
            label: "SSH Hosts",
            description: "Open the managed SSH host configuration manager",
            category: ActionCategory::Panels,
            destructive: false,
        },
        policy: AvailabilityPolicy::Always,
    },
    Registration {
        action: Action::OpenHelp,
        meta: ActionMeta {
            id: ActionId::OpenHelp,
            label: "Help",
            description: "Open or close contextual help",
            category: ActionCategory::Panels,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::OpenHotlist,
        meta: ActionMeta {
            id: ActionId::OpenHotlist,
            label: "Directory Hotlist",
            description: "open configured favorite directories",
            category: ActionCategory::Navigation,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::OpenInFileManager,
        meta: ActionMeta {
            id: ActionId::OpenInFileManager,
            label: "Open in file manager",
            description: "open active local directory in desktop file manager",
            category: ActionCategory::Files,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::BeginSymlink,
        meta: ActionMeta {
            id: ActionId::BeginSymlink,
            label: "Create symbolic link",
            description: "Prepare a symbolic link command for the focused item",
            category: ActionCategory::Files,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::BeginChmod,
        meta: ActionMeta {
            id: ActionId::BeginChmod,
            label: "Change permissions",
            description: "Prepare a chmod command",
            category: ActionCategory::Files,
            destructive: true,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::BeginHardLink,
        meta: ActionMeta {
            id: ActionId::BeginHardLink,
            label: "Create hard link",
            description: "Prepare a hard link command for the focused item",
            category: ActionCategory::Files,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::BeginChown,
        meta: ActionMeta {
            id: ActionId::BeginChown,
            label: "Change owner",
            description: "Prepare a chown command",
            category: ActionCategory::Files,
            destructive: true,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::ToggleWorkspaceComparison,
        meta: ActionMeta {
            id: ActionId::ToggleWorkspaceComparison,
            label: "Compare panes",
            description: "Compare the left and right locations as one workspace",
            category: ActionCategory::Workspace,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::PreviewWorkspaceSync,
        meta: ActionMeta {
            id: ActionId::PreviewWorkspaceSync,
            label: "Preview workspace sync",
            description: "Build a safe sync plan without changing files",
            category: ActionCategory::Workspace,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::ReverseWorkspaceDirection,
        meta: ActionMeta {
            id: ActionId::ReverseWorkspaceDirection,
            label: "Reverse sync direction",
            description: "Switch between left → right and right → left",
            category: ActionCategory::Workspace,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::ToggleWorkspaceSyncMode,
        meta: ActionMeta {
            id: ActionId::ToggleWorkspaceSyncMode,
            label: "Toggle update/mirror",
            description: "Switch safe update mode or destructive mirror mode",
            category: ActionCategory::Workspace,
            destructive: true,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::CloseWorkspaceSyncOverlay,
        meta: ActionMeta {
            id: ActionId::CloseWorkspaceSyncOverlay,
            label: "Hide workspace sync",
            description: "Hide the sync overlay without cancelling the job",
            category: ActionCategory::Workspace,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::ExecuteWorkspaceSync,
        meta: ActionMeta {
            id: ActionId::ExecuteWorkspaceSync,
            label: "Execute workspace sync",
            description: "Freeze the current preview and execute it when safe",
            category: ActionCategory::Workspace,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::ConfirmWorkspaceSync,
        meta: ActionMeta {
            id: ActionId::ConfirmWorkspaceSync,
            label: "Confirm workspace sync",
            description: "Explicitly confirm this exact destructive frozen plan",
            category: ActionCategory::Workspace,
            destructive: true,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::CancelWorkspaceSync,
        meta: ActionMeta {
            id: ActionId::CancelWorkspaceSync,
            label: "Cancel workspace sync",
            description: "Request cancellation of the active sync job",
            category: ActionCategory::Workspace,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::ShowWorkspaceSyncDetails,
        meta: ActionMeta {
            id: ActionId::ShowWorkspaceSyncDetails,
            label: "Show workspace sync details",
            description: "Reopen the current sync progress or verification overlay",
            category: ActionCategory::Workspace,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::ShowWorkspaceVerificationDiff,
        meta: ActionMeta {
            id: ActionId::ShowWorkspaceVerificationDiff,
            label: "Show verification diff",
            description: "Show workspace differences found by post-sync verification",
            category: ActionCategory::Workspace,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::ReturnToWorkspaceSyncPreview,
        meta: ActionMeta {
            id: ActionId::ReturnToWorkspaceSyncPreview,
            label: "Back in workspace sync",
            description: "Return from verification details or to the current preview when it still exists",
            category: ActionCategory::Workspace,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::ConfirmRemoteDelete,
        meta: ActionMeta {
            id: ActionId::ConfirmRemoteDelete,
            label: "Confirm remote delete",
            description: "Confirm permanent deletion of the selected remote files",
            category: ActionCategory::Files,
            destructive: true,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::CancelRemoteDelete,
        meta: ActionMeta {
            id: ActionId::CancelRemoteDelete,
            label: "Cancel remote delete",
            description: "Cancel the pending remote delete operation",
            category: ActionCategory::Files,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::OpenStorageInspector,
        meta: ActionMeta {
            id: ActionId::OpenStorageInspector,
            label: "Storage Inspector",
            description: "Read-only local disk usage analysis by JobManager scan",
            category: ActionCategory::Panels,
            destructive: false,
        },
        policy: AvailabilityPolicy::LocalOnly(
            "Storage Inspector is available for local paths only",
        ),
    },
];

/// All registered actions in canonical order.
pub fn registrations() -> &'static [Registration] {
    REGISTRATIONS
}

/// Resolve the registration for an exact action id.
pub fn registration_for(id: ActionId) -> Option<&'static Registration> {
    REGISTRATIONS.iter().find(|r| r.meta.id == id)
}

/// Test-facing lookup proving an id is registered (no second authority).
#[doc(hidden)]
pub fn registration_lookup(id: ActionId) -> bool {
    registration_for(id).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r_meta_id_matches_action_id_for_every_registration() {
        for r in registrations() {
            assert_eq!(r.action.id(), r.meta.id, "registration drift");
        }
    }

    #[test]
    fn r_registration_ids_are_unique() {
        let ids: std::collections::HashSet<_> = registrations().iter().map(|r| r.meta.id).collect();
        assert_eq!(
            ids.len(),
            registrations().len(),
            "duplicate ActionId registration"
        );
    }

    #[test]
    fn r_proof_features_are_registered() {
        for id in [
            ActionId::ComputeSha256,
            ActionId::TouchFile,
            ActionId::CompressTarGz,
            ActionId::OpenStorageInspector,
            ActionId::OpenSshHosts,
        ] {
            assert!(registration_for(id).is_some(), "{id:?} must be registered");
        }
    }

    #[test]
    fn r_storage_inspector_metadata_truth() {
        let r = registration_for(ActionId::OpenStorageInspector).unwrap();
        assert_eq!(r.meta.label, "Storage Inspector");
        assert_eq!(r.meta.category, ActionCategory::Panels);
        assert!(!r.meta.destructive);
        assert_eq!(
            r.policy,
            AvailabilityPolicy::LocalOnly("Storage Inspector is available for local paths only")
        );
    }

    #[test]
    fn r_proof_policies_are_policy_backed_not_default_match() {
        assert_eq!(
            registration_for(ActionId::ComputeSha256).unwrap().policy,
            AvailabilityPolicy::QuickSha256
        );
        assert_eq!(
            registration_for(ActionId::TouchFile).unwrap().policy,
            AvailabilityPolicy::QuickTouch
        );
        assert_eq!(
            registration_for(ActionId::CompressTarGz).unwrap().policy,
            AvailabilityPolicy::QuickCompress
        );
        assert_eq!(
            registration_for(ActionId::OpenSshHosts).unwrap().policy,
            AvailabilityPolicy::Always
        );
    }

    #[test]
    fn r_lookup_resolves_every_registered_id() {
        for r in registrations() {
            assert!(registration_for(r.meta.id).is_some());
        }
        // Closed-world: the table size equals the number of distinct ids.
        let ids: std::collections::HashSet<_> = registrations().iter().map(|r| r.meta.id).collect();
        assert_eq!(ids.len(), registrations().len());
    }
}
