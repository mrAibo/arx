use super::{AppState, Pane};

/// Stable identifiers for user-visible application actions.
///
/// `ActionId` deliberately contains no payload. It is safe to use from the
/// command palette, keymap, Which-Key/help UI, configuration, and plugins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionId {
    Quit,
    Up,
    Down,
    Enter,
    Back,
    SwitchPane,
    ToggleSelect,
    Refresh,
    OpenCommandCenter,
    OpenBookmarks,
    OpenJobs,
    OpenHosts,
    OpenHelp,
    BeginSymlink,
    BeginChmod,
    BeginHardLink,
    BeginChown,
    ToggleWorkspaceComparison,
    PreviewWorkspaceSync,
    ReverseWorkspaceDirection,
    ToggleWorkspaceSyncMode,
    CloseWorkspaceSyncOverlay,
    ExecuteWorkspaceSync,
    ConfirmWorkspaceSync,
    CancelWorkspaceSync,
    ShowWorkspaceSyncDetails,
    ShowWorkspaceVerificationDiff,
    ReturnToWorkspaceSyncPreview,
}

/// A concrete action invocation.
///
/// Payload-bearing variants can be added later without forcing presentation
/// layers to depend on their data because those layers reference `ActionId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Quit,
    Up,
    Down,
    Enter,
    Back,
    SwitchPane,
    ToggleSelect,
    Refresh,
    OpenCommandCenter,
    OpenBookmarks,
    OpenJobs,
    OpenHosts,
    OpenHelp,
    BeginSymlink,
    BeginChmod,
    BeginHardLink,
    BeginChown,
    ToggleWorkspaceComparison,
    PreviewWorkspaceSync,
    ReverseWorkspaceDirection,
    ToggleWorkspaceSyncMode,
    CloseWorkspaceSyncOverlay,
    ExecuteWorkspaceSync,
    ConfirmWorkspaceSync,
    CancelWorkspaceSync,
    ShowWorkspaceSyncDetails,
    ShowWorkspaceVerificationDiff,
    ReturnToWorkspaceSyncPreview,
}

impl Action {
    pub const fn id(self) -> ActionId {
        match self {
            Self::Quit => ActionId::Quit,
            Self::Up => ActionId::Up,
            Self::Down => ActionId::Down,
            Self::Enter => ActionId::Enter,
            Self::Back => ActionId::Back,
            Self::SwitchPane => ActionId::SwitchPane,
            Self::ToggleSelect => ActionId::ToggleSelect,
            Self::Refresh => ActionId::Refresh,
            Self::OpenCommandCenter => ActionId::OpenCommandCenter,
            Self::OpenBookmarks => ActionId::OpenBookmarks,
            Self::OpenJobs => ActionId::OpenJobs,
            Self::OpenHosts => ActionId::OpenHosts,
            Self::OpenHelp => ActionId::OpenHelp,
            Self::BeginSymlink => ActionId::BeginSymlink,
            Self::BeginChmod => ActionId::BeginChmod,
            Self::BeginHardLink => ActionId::BeginHardLink,
            Self::BeginChown => ActionId::BeginChown,
            Self::ToggleWorkspaceComparison => ActionId::ToggleWorkspaceComparison,
            Self::PreviewWorkspaceSync => ActionId::PreviewWorkspaceSync,
            Self::ReverseWorkspaceDirection => ActionId::ReverseWorkspaceDirection,
            Self::ToggleWorkspaceSyncMode => ActionId::ToggleWorkspaceSyncMode,
            Self::CloseWorkspaceSyncOverlay => ActionId::CloseWorkspaceSyncOverlay,
            Self::ExecuteWorkspaceSync => ActionId::ExecuteWorkspaceSync,
            Self::ConfirmWorkspaceSync => ActionId::ConfirmWorkspaceSync,
            Self::CancelWorkspaceSync => ActionId::CancelWorkspaceSync,
            Self::ShowWorkspaceSyncDetails => ActionId::ShowWorkspaceSyncDetails,
            Self::ShowWorkspaceVerificationDiff => ActionId::ShowWorkspaceVerificationDiff,
            Self::ReturnToWorkspaceSyncPreview => ActionId::ReturnToWorkspaceSyncPreview,
        }
    }
}

pub const ALL_ACTIONS: &[Action] = &[
    Action::Quit,
    Action::Up,
    Action::Down,
    Action::Enter,
    Action::Back,
    Action::SwitchPane,
    Action::ToggleSelect,
    Action::Refresh,
    Action::OpenCommandCenter,
    Action::OpenBookmarks,
    Action::OpenJobs,
    Action::OpenHosts,
    Action::OpenHelp,
    Action::BeginSymlink,
    Action::BeginChmod,
    Action::BeginHardLink,
    Action::BeginChown,
    Action::ToggleWorkspaceComparison,
    Action::PreviewWorkspaceSync,
    Action::ReverseWorkspaceDirection,
    Action::ToggleWorkspaceSyncMode,
    Action::CloseWorkspaceSyncOverlay,
    Action::ExecuteWorkspaceSync,
    Action::ConfirmWorkspaceSync,
    Action::CancelWorkspaceSync,
    Action::ShowWorkspaceSyncDetails,
    Action::ShowWorkspaceVerificationDiff,
    Action::ReturnToWorkspaceSyncPreview,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionCategory {
    Application,
    Navigation,
    Selection,
    Panels,
    Files,
    Workspace,
}

/// Presentation metadata shared by Command Center, help, Which-Key, context
/// menus, and future generated documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionMeta {
    pub id: ActionId,
    pub label: &'static str,
    pub description: &'static str,
    pub category: ActionCategory,
    pub destructive: bool,
}

pub const ACTION_CATALOG: &[ActionMeta] = &[
    ActionMeta {
        id: ActionId::Quit,
        label: "Quit",
        description: "Exit ARX",
        category: ActionCategory::Application,
        destructive: false,
    },
    ActionMeta {
        id: ActionId::Up,
        label: "Move up",
        description: "Move the cursor up",
        category: ActionCategory::Navigation,
        destructive: false,
    },
    ActionMeta {
        id: ActionId::Down,
        label: "Move down",
        description: "Move the cursor down",
        category: ActionCategory::Navigation,
        destructive: false,
    },
    ActionMeta {
        id: ActionId::Enter,
        label: "Open",
        description: "Open the focused item",
        category: ActionCategory::Navigation,
        destructive: false,
    },
    ActionMeta {
        id: ActionId::Back,
        label: "Back",
        description: "Navigate to the parent or previous location",
        category: ActionCategory::Navigation,
        destructive: false,
    },
    ActionMeta {
        id: ActionId::SwitchPane,
        label: "Switch pane",
        description: "Move focus to the opposite pane",
        category: ActionCategory::Panels,
        destructive: false,
    },
    ActionMeta {
        id: ActionId::ToggleSelect,
        label: "Toggle selection",
        description: "Select or deselect the focused item",
        category: ActionCategory::Selection,
        destructive: false,
    },
    ActionMeta {
        id: ActionId::Refresh,
        label: "Refresh",
        description: "Refresh visible entries",
        category: ActionCategory::Navigation,
        destructive: false,
    },
    ActionMeta {
        id: ActionId::OpenCommandCenter,
        label: "Command Center",
        description: "Search actions, hosts, bookmarks, and history",
        category: ActionCategory::Panels,
        destructive: false,
    },
    ActionMeta {
        id: ActionId::OpenBookmarks,
        label: "Bookmarks",
        description: "Open the bookmarks panel",
        category: ActionCategory::Panels,
        destructive: false,
    },
    ActionMeta {
        id: ActionId::OpenJobs,
        label: "Jobs",
        description: "Open the background jobs panel",
        category: ActionCategory::Panels,
        destructive: false,
    },
    ActionMeta {
        id: ActionId::OpenHosts,
        label: "Hosts",
        description: "Open the SSH hosts panel",
        category: ActionCategory::Panels,
        destructive: false,
    },
    ActionMeta {
        id: ActionId::OpenHelp,
        label: "Help",
        description: "Open or close contextual help",
        category: ActionCategory::Panels,
        destructive: false,
    },
    ActionMeta {
        id: ActionId::BeginSymlink,
        label: "Create symbolic link",
        description: "Prepare a symbolic link command for the focused item",
        category: ActionCategory::Files,
        destructive: false,
    },
    ActionMeta {
        id: ActionId::BeginChmod,
        label: "Change permissions",
        description: "Prepare a chmod command",
        category: ActionCategory::Files,
        destructive: true,
    },
    ActionMeta {
        id: ActionId::BeginHardLink,
        label: "Create hard link",
        description: "Prepare a hard link command for the focused item",
        category: ActionCategory::Files,
        destructive: false,
    },
    ActionMeta {
        id: ActionId::BeginChown,
        label: "Change owner",
        description: "Prepare a chown command",
        category: ActionCategory::Files,
        destructive: true,
    },
    ActionMeta {
        id: ActionId::ToggleWorkspaceComparison,
        label: "Compare panes",
        description: "Compare the left and right locations as one workspace",
        category: ActionCategory::Workspace,
        destructive: false,
    },
    ActionMeta {
        id: ActionId::PreviewWorkspaceSync,
        label: "Preview workspace sync",
        description: "Build a safe sync plan without changing files",
        category: ActionCategory::Workspace,
        destructive: false,
    },
    ActionMeta {
        id: ActionId::ReverseWorkspaceDirection,
        label: "Reverse sync direction",
        description: "Switch between left → right and right → left",
        category: ActionCategory::Workspace,
        destructive: false,
    },
    ActionMeta {
        id: ActionId::ToggleWorkspaceSyncMode,
        label: "Toggle update/mirror",
        description: "Switch safe update mode or destructive mirror mode",
        category: ActionCategory::Workspace,
        destructive: true,
    },
    ActionMeta {
        id: ActionId::CloseWorkspaceSyncOverlay,
        label: "Hide workspace sync",
        description: "Hide the sync overlay without cancelling the job",
        category: ActionCategory::Workspace,
        destructive: false,
    },
    ActionMeta {
        id: ActionId::ExecuteWorkspaceSync,
        label: "Execute workspace sync",
        description: "Freeze the current preview and execute it when safe",
        category: ActionCategory::Workspace,
        destructive: false,
    },
    ActionMeta {
        id: ActionId::ConfirmWorkspaceSync,
        label: "Confirm workspace sync",
        description: "Explicitly confirm this exact destructive frozen plan",
        category: ActionCategory::Workspace,
        destructive: true,
    },
    ActionMeta {
        id: ActionId::CancelWorkspaceSync,
        label: "Cancel workspace sync",
        description: "Request cancellation of the active sync job",
        category: ActionCategory::Workspace,
        destructive: false,
    },
    ActionMeta {
        id: ActionId::ShowWorkspaceSyncDetails,
        label: "Show workspace sync details",
        description: "Reopen the current sync progress or verification overlay",
        category: ActionCategory::Workspace,
        destructive: false,
    },
    ActionMeta {
        id: ActionId::ShowWorkspaceVerificationDiff,
        label: "Show verification diff",
        description: "Show workspace differences found by post-sync verification",
        category: ActionCategory::Workspace,
        destructive: false,
    },
    ActionMeta {
        id: ActionId::ReturnToWorkspaceSyncPreview,
        label: "Return to sync preview",
        description: "Return from confirmation or result details to the current diff preview",
        category: ActionCategory::Workspace,
        destructive: false,
    },
];

pub fn action_meta(id: ActionId) -> Option<&'static ActionMeta> {
    ACTION_CATALOG.iter().find(|meta| meta.id == id)
}

/// High-level input ownership.
///
/// This is intentionally derived from the current `AppState` instead of
/// introducing a second mutable overlay state. The keymap migration can use
/// this immediately while the existing booleans are removed incrementally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InputContext {
    Terminal,
    CommandCenter,
    TextInput,
    Viewer,
    Help,
    SyncPreview,
    SyncConfirmation,
    SyncJob,
    Bookmarks,
    Hosts,
    Jobs,
    UserMenu,
    Browser,
}

impl AppState {
    pub fn input_context(&self) -> InputContext {
        if self.show_terminal && self.active == Pane::Right {
            InputContext::Terminal
        } else if matches!(
            self.active_overlay(),
            Some(super::OverlayKind::CommandCenter)
        ) {
            InputContext::CommandCenter
        } else if self.filtering || self.glob_input || self.go_input || self.cmd_input {
            InputContext::TextInput
        } else if !self.viewer_content.is_empty() {
            InputContext::Viewer
        } else {
            match self.active_overlay() {
                Some(super::OverlayKind::CommandCenter) => InputContext::CommandCenter,
                Some(super::OverlayKind::Help) => InputContext::Help,
                Some(super::OverlayKind::Bookmarks) => InputContext::Bookmarks,
                Some(super::OverlayKind::Hosts) => InputContext::Hosts,
                Some(super::OverlayKind::Jobs) => InputContext::Jobs,
                Some(super::OverlayKind::UserMenu) => InputContext::UserMenu,
                Some(super::OverlayKind::SyncPreview) => match self.remote_workspace.ux {
                    super::WorkspaceSyncUxState::ConfirmationRequired { .. } => {
                        InputContext::SyncConfirmation
                    }
                    super::WorkspaceSyncUxState::Launching { .. }
                    | super::WorkspaceSyncUxState::Queued { .. }
                    | super::WorkspaceSyncUxState::Running { .. }
                    | super::WorkspaceSyncUxState::Cancelling { .. }
                    | super::WorkspaceSyncUxState::Verifying { .. }
                    | super::WorkspaceSyncUxState::Finished { .. } => InputContext::SyncJob,
                    _ => InputContext::SyncPreview,
                },
                _ => InputContext::Browser,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_has_catalog_metadata() {
        assert_eq!(ALL_ACTIONS.len(), ACTION_CATALOG.len());
        for action in ALL_ACTIONS {
            let meta = action_meta(action.id())
                .unwrap_or_else(|| panic!("missing metadata for {:?}", action.id()));
            assert_eq!(meta.id, action.id());
            assert!(!meta.label.is_empty());
            assert!(!meta.description.is_empty());
        }
    }

    #[test]
    fn action_ids_are_unique_in_catalog() {
        for (index, meta) in ACTION_CATALOG.iter().enumerate() {
            assert!(
                ACTION_CATALOG[index + 1..]
                    .iter()
                    .all(|other| other.id != meta.id),
                "duplicate action metadata for {:?}",
                meta.id
            );
        }
    }

    #[test]
    fn command_center_owns_input_before_generic_text_input() {
        let state = AppState {
            show_command_center: true,
            filtering: true,
            ..AppState::default()
        };
        assert_eq!(state.input_context(), InputContext::CommandCenter);
    }

    #[test]
    fn terminal_owns_input_on_the_right_pane() {
        let state = AppState {
            show_terminal: true,
            active: Pane::Right,
            show_command_center: true,
            ..AppState::default()
        };
        assert_eq!(state.input_context(), InputContext::Terminal);
    }

    #[test]
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
        state.remote_workspace.ux = crate::app::WorkspaceSyncUxState::Launching { plan_id };
        assert_eq!(state.input_context(), InputContext::SyncJob);
    }

    #[test]
    fn browser_is_the_default_input_context() {
        assert_eq!(AppState::default().input_context(), InputContext::Browser);
    }

    #[test]
    fn existing_reducer_behavior_is_characterized() {
        let mut state = AppState::default();
        assert_eq!(state.active, Pane::Left);

        state.apply(Action::SwitchPane);
        assert_eq!(state.active, Pane::Right);

        state.apply(Action::Quit);
        assert!(state.should_quit);
    }
}
