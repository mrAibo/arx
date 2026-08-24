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
            description: "Toggle split view for the active pane",
            category: ActionCategory::Panels,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::OpenVerticalSplit,
        meta: ActionMeta {
            id: ActionId::OpenVerticalSplit,
            label: "Vertical split",
            description: "Open or switch the active pane to a vertical split",
            category: ActionCategory::Panels,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::OpenHorizontalSplit,
        meta: ActionMeta {
            id: ActionId::OpenHorizontalSplit,
            label: "Horizontal split",
            description: "Open or switch the active pane to a horizontal split",
            category: ActionCategory::Panels,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::CloseSplitPane,
        meta: ActionMeta {
            id: ActionId::CloseSplitPane,
            label: "Close split",
            description: "Close the split view in the active pane",
            category: ActionCategory::Panels,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::DecreaseSplitRatio,
        meta: ActionMeta {
            id: ActionId::DecreaseSplitRatio,
            label: "Move split boundary backward",
            description: "Decrease the primary split share",
            category: ActionCategory::Panels,
            destructive: false,
        },
        policy: AvailabilityPolicy::Default,
    },
    Registration {
        action: Action::IncreaseSplitRatio,
        meta: ActionMeta {
            id: ActionId::IncreaseSplitRatio,
            label: "Move split boundary forward",
            description: "Increase the primary split share",
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
        action: Action::ListScreenSessions,
        meta: ActionMeta {
            id: ActionId::ListScreenSessions,
            label: "List screen sessions",
            description: "Discover and attach to a GNU Screen session",
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

impl ActionId {
    /// Stable snake_case configuration identity (#214).
    ///
    /// Explicitly enumerated — never derived from Debug formatting or
    /// automatic case conversion — so renaming a Rust variant cannot silently
    /// change an accepted config file's meaning.
    pub fn config_name(self) -> &'static str {
        match self {
            ActionId::Quit => "quit",
            ActionId::Up => "up",
            ActionId::Down => "down",
            ActionId::Enter => "enter",
            ActionId::Back => "back",
            ActionId::SwitchPane => "switch_pane",
            ActionId::ToggleSplitPane => "toggle_split_pane",
            ActionId::OpenVerticalSplit => "open_vertical_split",
            ActionId::OpenHorizontalSplit => "open_horizontal_split",
            ActionId::CloseSplitPane => "close_split_pane",
            ActionId::DecreaseSplitRatio => "decrease_split_ratio",
            ActionId::IncreaseSplitRatio => "increase_split_ratio",
            ActionId::ToggleSelect => "toggle_select",
            ActionId::ViewFile => "view_file",
            ActionId::EditFile => "edit_file",
            ActionId::Copy => "copy",
            ActionId::Move => "move",
            ActionId::Mkdir => "mkdir",
            ActionId::Delete => "delete",
            ActionId::ComputeSha256 => "compute_sha256",
            ActionId::TouchFile => "touch_file",
            ActionId::CompressTarGz => "compress_tar_gz",
            ActionId::ListTmuxSessions => "list_tmux_sessions",
            ActionId::ListScreenSessions => "list_screen_sessions",
            ActionId::ToggleEmbeddedTerminal => "toggle_embedded_terminal",
            ActionId::Refresh => "refresh",
            ActionId::OpenCommandCenter => "open_command_center",
            ActionId::OpenBookmarks => "open_bookmarks",
            ActionId::OpenJobs => "open_jobs",
            ActionId::OpenSmartTree => "open_smart_tree",
            ActionId::OpenInfrastructureCenter => "open_infrastructure_center",
            ActionId::OpenHosts => "open_hosts",
            ActionId::OpenSshHosts => "open_ssh_hosts",
            ActionId::OpenHelp => "open_help",
            ActionId::OpenHotlist => "open_hotlist",
            ActionId::OpenInFileManager => "open_in_file_manager",
            ActionId::BeginSymlink => "begin_symlink",
            ActionId::BeginChmod => "begin_chmod",
            ActionId::BeginHardLink => "begin_hard_link",
            ActionId::BeginChown => "begin_chown",
            ActionId::ToggleWorkspaceComparison => "toggle_workspace_comparison",
            ActionId::PreviewWorkspaceSync => "preview_workspace_sync",
            ActionId::ReverseWorkspaceDirection => "reverse_workspace_direction",
            ActionId::ToggleWorkspaceSyncMode => "toggle_workspace_sync_mode",
            ActionId::CloseWorkspaceSyncOverlay => "close_workspace_sync_overlay",
            ActionId::ExecuteWorkspaceSync => "execute_workspace_sync",
            ActionId::ConfirmWorkspaceSync => "confirm_workspace_sync",
            ActionId::CancelWorkspaceSync => "cancel_workspace_sync",
            ActionId::ShowWorkspaceSyncDetails => "show_workspace_sync_details",
            ActionId::ShowWorkspaceVerificationDiff => "show_workspace_verification_diff",
            ActionId::ReturnToWorkspaceSyncPreview => "return_to_workspace_sync_preview",
            ActionId::ConfirmRemoteDelete => "confirm_remote_delete",
            ActionId::CancelRemoteDelete => "cancel_remote_delete",
            ActionId::OpenStorageInspector => "open_storage_inspector",
        }
    }
}

impl std::str::FromStr for ActionId {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "quit" => Ok(ActionId::Quit),
            "up" => Ok(ActionId::Up),
            "down" => Ok(ActionId::Down),
            "enter" => Ok(ActionId::Enter),
            "back" => Ok(ActionId::Back),
            "switch_pane" => Ok(ActionId::SwitchPane),
            "toggle_split_pane" => Ok(ActionId::ToggleSplitPane),
            "open_vertical_split" => Ok(ActionId::OpenVerticalSplit),
            "open_horizontal_split" => Ok(ActionId::OpenHorizontalSplit),
            "close_split_pane" => Ok(ActionId::CloseSplitPane),
            "decrease_split_ratio" => Ok(ActionId::DecreaseSplitRatio),
            "increase_split_ratio" => Ok(ActionId::IncreaseSplitRatio),
            "toggle_select" => Ok(ActionId::ToggleSelect),
            "view_file" => Ok(ActionId::ViewFile),
            "edit_file" => Ok(ActionId::EditFile),
            "copy" => Ok(ActionId::Copy),
            "move" => Ok(ActionId::Move),
            "mkdir" => Ok(ActionId::Mkdir),
            "delete" => Ok(ActionId::Delete),
            "compute_sha256" => Ok(ActionId::ComputeSha256),
            "touch_file" => Ok(ActionId::TouchFile),
            "compress_tar_gz" => Ok(ActionId::CompressTarGz),
            "list_tmux_sessions" => Ok(ActionId::ListTmuxSessions),
            "list_screen_sessions" => Ok(ActionId::ListScreenSessions),
            "toggle_embedded_terminal" => Ok(ActionId::ToggleEmbeddedTerminal),
            "refresh" => Ok(ActionId::Refresh),
            "open_command_center" => Ok(ActionId::OpenCommandCenter),
            "open_bookmarks" => Ok(ActionId::OpenBookmarks),
            "open_jobs" => Ok(ActionId::OpenJobs),
            "open_smart_tree" => Ok(ActionId::OpenSmartTree),
            "open_infrastructure_center" => Ok(ActionId::OpenInfrastructureCenter),
            "open_hosts" => Ok(ActionId::OpenHosts),
            "open_ssh_hosts" => Ok(ActionId::OpenSshHosts),
            "open_help" => Ok(ActionId::OpenHelp),
            "open_hotlist" => Ok(ActionId::OpenHotlist),
            "open_in_file_manager" => Ok(ActionId::OpenInFileManager),
            "begin_symlink" => Ok(ActionId::BeginSymlink),
            "begin_chmod" => Ok(ActionId::BeginChmod),
            "begin_hard_link" => Ok(ActionId::BeginHardLink),
            "begin_chown" => Ok(ActionId::BeginChown),
            "toggle_workspace_comparison" => Ok(ActionId::ToggleWorkspaceComparison),
            "preview_workspace_sync" => Ok(ActionId::PreviewWorkspaceSync),
            "reverse_workspace_direction" => Ok(ActionId::ReverseWorkspaceDirection),
            "toggle_workspace_sync_mode" => Ok(ActionId::ToggleWorkspaceSyncMode),
            "close_workspace_sync_overlay" => Ok(ActionId::CloseWorkspaceSyncOverlay),
            "execute_workspace_sync" => Ok(ActionId::ExecuteWorkspaceSync),
            "confirm_workspace_sync" => Ok(ActionId::ConfirmWorkspaceSync),
            "cancel_workspace_sync" => Ok(ActionId::CancelWorkspaceSync),
            "show_workspace_sync_details" => Ok(ActionId::ShowWorkspaceSyncDetails),
            "show_workspace_verification_diff" => Ok(ActionId::ShowWorkspaceVerificationDiff),
            "return_to_workspace_sync_preview" => Ok(ActionId::ReturnToWorkspaceSyncPreview),
            "confirm_remote_delete" => Ok(ActionId::ConfirmRemoteDelete),
            "cancel_remote_delete" => Ok(ActionId::CancelRemoteDelete),
            "open_storage_inspector" => Ok(ActionId::OpenStorageInspector),
            other => Err(format!("unknown action: {}", sanitize_config_token(other))),
        }
    }
}

/// Output-only escaping so hostile control characters in config text can never
/// inject newlines/ESC into terminal diagnostics (#214).
pub(crate) fn sanitize_config_token(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u8));
            }
            c => out.push(c),
        }
    }
    out
}

/// Resolve the registration for an exact action id.
pub fn registration_for(id: ActionId) -> Option<&'static Registration> {
    REGISTRATIONS.iter().find(|r| r.meta.id == id)
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
