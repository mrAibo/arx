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
    ToggleEmbeddedTerminal,
    ViewFile,
    EditFile,
    Copy,
    Move,
    Mkdir,
    Delete,
    ListTmuxSessions,
    Refresh,
    OpenCommandCenter,
    OpenBookmarks,
    OpenJobs,
    OpenHosts,
    OpenSshHosts,
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
    ConfirmRemoteDelete,
    CancelRemoteDelete,
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
    ToggleEmbeddedTerminal,
    ViewFile,
    EditFile,
    Copy,
    Move,
    Mkdir,
    Delete,
    ListTmuxSessions,
    Refresh,
    OpenCommandCenter,
    OpenBookmarks,
    OpenJobs,
    OpenHosts,
    OpenSshHosts,
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
    ConfirmRemoteDelete,
    CancelRemoteDelete,
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
            Self::ToggleEmbeddedTerminal => ActionId::ToggleEmbeddedTerminal,
            Self::ViewFile => ActionId::ViewFile,
            Self::EditFile => ActionId::EditFile,
            Self::Copy => ActionId::Copy,
            Self::Move => ActionId::Move,
            Self::Mkdir => ActionId::Mkdir,
            Self::Delete => ActionId::Delete,
            Self::ListTmuxSessions => ActionId::ListTmuxSessions,
            Self::Refresh => ActionId::Refresh,
            Self::OpenCommandCenter => ActionId::OpenCommandCenter,
            Self::OpenBookmarks => ActionId::OpenBookmarks,
            Self::OpenJobs => ActionId::OpenJobs,
            Self::OpenHosts => ActionId::OpenHosts,
            Self::OpenSshHosts => ActionId::OpenSshHosts,
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
            Self::ConfirmRemoteDelete => ActionId::ConfirmRemoteDelete,
            Self::CancelRemoteDelete => ActionId::CancelRemoteDelete,
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
    Action::ToggleEmbeddedTerminal,
    Action::ViewFile,
    Action::EditFile,
    Action::Copy,
    Action::Move,
    Action::Mkdir,
    Action::Delete,
    Action::ListTmuxSessions,
    Action::Refresh,
    Action::OpenCommandCenter,
    Action::OpenBookmarks,
    Action::OpenJobs,
    Action::OpenHosts,
    Action::OpenSshHosts,
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
    Action::ConfirmRemoteDelete,
    Action::CancelRemoteDelete,
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
        id: ActionId::ViewFile,
        label: "View file",
        description: "Open a read-only preview of the focused file",
        category: ActionCategory::Files,
        destructive: false,
    },
    ActionMeta {
        id: ActionId::EditFile,
        label: "Edit file",
        description: "Edit the focused supported text file",
        category: ActionCategory::Files,
        destructive: false,
    },
    ActionMeta {
        id: ActionId::Copy,
        label: "Copy",
        description: "Copy file selection to other pane",
        category: ActionCategory::Files,
        destructive: false,
    },
    ActionMeta {
        id: ActionId::Move,
        label: "Move",
        description: "Move file selection to other pane",
        category: ActionCategory::Files,
        destructive: false,
    },
    ActionMeta {
        id: ActionId::Mkdir,
        label: "New directory",
        description: "Create a new directory",
        category: ActionCategory::Files,
        destructive: false,
    },
    ActionMeta {
        id: ActionId::Delete,
        label: "Delete",
        description: "Delete the selected items using the provider's safe delete flow",
        category: ActionCategory::Files,
        destructive: true,
    },
    ActionMeta {
        id: ActionId::ListTmuxSessions,
        label: "List tmux sessions",
        description: "Discover and attach to a tmux session",
        category: ActionCategory::Application,
        destructive: false,
    },
    ActionMeta {
        id: ActionId::ToggleEmbeddedTerminal,
        label: "Embedded Terminal",
        description: "Open an embedded terminal in the right pane",
        category: ActionCategory::Panels,
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
        id: ActionId::OpenSshHosts,
        label: "SSH Hosts",
        description: "Open the managed SSH host configuration manager",
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
        label: "Back in workspace sync",
        description: "Return from verification details or to the current preview when it still exists",
        category: ActionCategory::Workspace,
        destructive: false,
    },
    ActionMeta {
        id: ActionId::ConfirmRemoteDelete,
        label: "Confirm remote delete",
        description: "Confirm permanent deletion of the selected remote files",
        category: ActionCategory::Files,
        destructive: true,
    },
    ActionMeta {
        id: ActionId::CancelRemoteDelete,
        label: "Cancel remote delete",
        description: "Cancel the pending remote delete operation",
        category: ActionCategory::Files,
        destructive: false,
    },
];

pub fn action_meta(id: ActionId) -> Option<&'static ActionMeta> {
    ACTION_CATALOG.iter().find(|meta| meta.id == id)
}

pub fn listed_entry_navigation_target(
    current: &crate::vfs::Location,
    listed: &crate::vfs::ListedEntry,
) -> Option<crate::vfs::Location> {
    match &listed.identity {
        crate::vfs::EntryIdentity::S3Bucket(reference) => Some(crate::vfs::Location::S3 {
            target: reference.target.clone(),
            bucket: Some(reference.bucket.clone()),
            prefix: String::new(),
        }),
        crate::vfs::EntryIdentity::S3Prefix(reference) => {
            // S3-20R guarantees every provider-produced CommonPrefix ends in the
            // Delimiter="/"; here we remove exactly one final protocol delimiter
            // to recover the navigation prefix. A malformed ref without it fails
            // closed (None) rather than inventing or preserving a delimiter.
            let nav_prefix = reference.prefix.strip_suffix('/')?;
            Some(crate::vfs::Location::S3 {
                target: reference.target.clone(),
                bucket: Some(reference.bucket.clone()),
                prefix: nav_prefix.to_owned(),
            })
        }
        crate::vfs::EntryIdentity::S3Object(_) => None,
        crate::vfs::EntryIdentity::Other
            if listed.entry.kind == crate::vfs::EntryKind::Directory
                && matches!(
                    current,
                    crate::vfs::Location::Local(_)
                        | crate::vfs::Location::Sftp { .. }
                        | crate::vfs::Location::Archive { .. }
                ) =>
        {
            Some(current.child(&listed.entry.name))
        }
        crate::vfs::EntryIdentity::Other => None,
    }
}

/// Contextual parent resolver — the single authority for virtual `..` navigation.
///
/// `Location::parent()` stays context-free and fail-closed for `Location::S3`
/// (it cannot know account-root vs bucket-bound semantics). This helper consults
/// the one authoritative configured-target inventory (`ProviderRegistry`) so S3
/// parent navigation is correct for both binding modes, while Local/SFTP/Archive
/// keep their existing `parent()` behavior unchanged.
///
/// Bucket-bound targets NEVER escape to the account root (would enable
/// ListBuckets / s3:ListAllMyBuckets): at a bucket-bound root the parent is
/// `None`. Target root (`bucket: None`, `prefix: ""`) is terminal. Unknown
/// targets fail closed. Navigation prefixes use string/namespace semantics:
/// remove the final segment via `rfind('/')` — no `trim_end_matches`, `Path`,
/// canonicalize, `//`-collapse, or `.`/`..` resolution.
pub fn navigation_parent_target(
    current: &crate::vfs::Location,
    registry: &crate::vfs::ProviderRegistry,
) -> Option<crate::vfs::Location> {
    use crate::vfs::{Location, S3TargetBinding};
    match current {
        Location::Local(_) | Location::Sftp { .. } | Location::Archive { .. } => current.parent(),
        Location::S3 {
            target,
            bucket,
            prefix,
        } => {
            let binding = registry.s3_target_binding(target)?;
            match (bucket, &binding) {
                // Bucket root of a bucket-bound target: terminal, no escape.
                (Some(current_bucket), S3TargetBinding::BucketBound(bound)) => {
                    if current_bucket != bound {
                        return None;
                    }
                    if prefix.is_empty() {
                        return None;
                    }
                    Some(location_with_parent_prefix(target, bucket, prefix))
                }
                // Bucket root of an account-style target: expose target root.
                (Some(_), S3TargetBinding::AccountRoot) => {
                    if prefix.is_empty() {
                        return Some(Location::S3 {
                            target: target.clone(),
                            bucket: None,
                            prefix: String::new(),
                        });
                    }
                    Some(location_with_parent_prefix(target, bucket, prefix))
                }
                // Target root: terminal.
                (None, _) => None,
            }
        }
    }
}

/// Parent prefix = remove the final navigation segment via the final literal
/// '/'. Repeated slashes, literal '.' / '..' segments, and Unicode are preserved
/// verbatim; only the last segment is dropped.
fn location_with_parent_prefix(
    target: &str,
    bucket: &Option<String>,
    prefix: &str,
) -> crate::vfs::Location {
    let parent_prefix = match prefix.rfind('/') {
        Some(index) => &prefix[..index],
        None => "",
    };
    crate::vfs::Location::S3 {
        target: target.to_string(),
        bucket: bucket.clone(),
        prefix: parent_prefix.to_string(),
    }
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
    DeleteConfirmation,
}

impl AppState {
    pub fn input_context(&self) -> InputContext {
        if self.pending_delete.is_some() {
            return InputContext::DeleteConfirmation;
        }
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
                    | super::WorkspaceSyncUxState::Finished { .. }
                    | super::WorkspaceSyncUxState::VerificationDiff { .. } => InputContext::SyncJob,
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
    use crate::vfs::{
        Entry, EntryIdentity, EntryKind, ListedEntry, Location,
        s3::{S3BucketRef, S3ObjectRef, S3PrefixRef},
    };

    fn listed(name: &str, kind: EntryKind, identity: EntryIdentity) -> ListedEntry {
        ListedEntry {
            entry: Entry {
                name: name.into(),
                kind,
                size: None,
                modified_unix_ms: None,
            },
            identity,
        }
    }

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
    fn bucket_navigation_uses_ref_not_presentation() {
        let listed = listed(
            "DISPLAY-NAME-THAT-MUST-NOT-BE-USED",
            EntryKind::Directory,
            EntryIdentity::S3Bucket(S3BucketRef {
                target: "aws-prod".into(),
                bucket: "Company-Artifacts".into(),
            }),
        );

        assert_eq!(
            listed_entry_navigation_target(
                &Location::S3 {
                    target: "other-target".into(),
                    bucket: None,
                    prefix: "ignored".into(),
                },
                &listed,
            ),
            Some(Location::S3 {
                target: "aws-prod".into(),
                bucket: Some("Company-Artifacts".into()),
                prefix: String::new(),
            })
        );
    }

    #[test]
    fn bucket_case_preserved_exactly() {
        let listed = listed(
            "display",
            EntryKind::Directory,
            EntryIdentity::S3Bucket(S3BucketRef {
                target: "Aws-PROD".into(),
                bucket: "Company-ARTIFACTS".into(),
            }),
        );

        assert_eq!(
            listed_entry_navigation_target(&Location::Local("/ignored".into()), &listed),
            Some(Location::S3 {
                target: "Aws-PROD".into(),
                bucket: Some("Company-ARTIFACTS".into()),
                prefix: String::new(),
            })
        );
    }

    #[test]
    fn s3_prefix_navigation_uses_exact_ref_and_removes_one_final_delimiter() {
        for (display, target, bucket, prefix, expected_prefix) in [
            (
                "DISPLAY-NAME-THAT-MUST-NOT-BE-USED",
                "aws-prod",
                "Company-Artifacts",
                "foo/bar/",
                "foo/bar",
            ),
            ("display", "target", "bucket", "foo//bar/", "foo//bar"),
            ("display", "target", "bucket", "foo/../bar/", "foo/../bar"),
            ("display", "target", "bucket", "foo/./bar/", "foo/./bar"),
            ("display", "target", "bucket", "日本語/資料/", "日本語/資料"),
            ("display", "target", "bucket", "foo//", "foo/"),
        ] {
            let listed = listed(
                display,
                EntryKind::Directory,
                EntryIdentity::S3Prefix(S3PrefixRef {
                    target: target.into(),
                    bucket: bucket.into(),
                    prefix: prefix.into(),
                }),
            );

            assert_eq!(
                listed_entry_navigation_target(&Location::Local("/ignored".into()), &listed),
                Some(Location::S3 {
                    target: target.into(),
                    bucket: Some(bucket.into()),
                    prefix: expected_prefix.into(),
                })
            );
        }
    }

    #[test]
    fn s3_prefix_malformed_without_final_delimiter_fails_closed() {
        // S3-20R validates CommonPrefix delimiter provider-side, so any ref
        // without a trailing '/' is malformed; navigation must not preserve it.
        let listed = listed(
            "DISPLAY-NAME-THAT-MUST-NOT-BE-USED",
            EntryKind::Directory,
            EntryIdentity::S3Prefix(S3PrefixRef {
                target: "target".into(),
                bucket: "bucket".into(),
                prefix: "no-final-delimiter".into(),
            }),
        );

        assert_eq!(
            listed_entry_navigation_target(&Location::Local("/ignored".into()), &listed),
            None
        );
    }

    #[test]
    fn s3_object_not_directory_navigation() {
        let listed = listed(
            "display",
            EntryKind::Directory,
            EntryIdentity::S3Object(S3ObjectRef {
                target: "aws-prod".into(),
                bucket: "Company-Artifacts".into(),
                key: "exact/object".into(),
            }),
        );

        assert_eq!(
            listed_entry_navigation_target(&Location::Local("/ignored".into()), &listed),
            None
        );
    }

    #[test]
    fn s3_other_does_not_fallback_to_name() {
        let listed = listed(
            "DISPLAY-NAME-THAT-MUST-NOT-BE-USED",
            EntryKind::Directory,
            EntryIdentity::Other,
        );

        assert_eq!(
            listed_entry_navigation_target(
                &Location::S3 {
                    target: "aws-prod".into(),
                    bucket: Some("Company-Artifacts".into()),
                    prefix: String::new(),
                },
                &listed,
            ),
            None
        );
    }

    #[test]
    fn local_other_directory_unchanged() {
        let listed = listed("child", EntryKind::Directory, EntryIdentity::Other);

        assert_eq!(
            listed_entry_navigation_target(&Location::Local("/parent".into()), &listed),
            Some(Location::Local("/parent/child".into()))
        );
    }

    #[test]
    fn sftp_other_directory_unchanged() {
        let listed = listed("child", EntryKind::Directory, EntryIdentity::Other);

        assert_eq!(
            listed_entry_navigation_target(
                &Location::Sftp {
                    host: "host".into(),
                    path: "/parent".into(),
                },
                &listed,
            ),
            Some(Location::Sftp {
                host: "host".into(),
                path: "/parent/child".into(),
            })
        );
    }

    #[test]
    fn archive_other_directory_unchanged() {
        let listed = listed("child", EntryKind::Directory, EntryIdentity::Other);

        assert_eq!(
            listed_entry_navigation_target(
                &Location::Archive {
                    archive: "/archive.zip".into(),
                    inner_path: "parent".into(),
                },
                &listed,
            ),
            Some(Location::Archive {
                archive: "/archive.zip".into(),
                inner_path: "parent/child".into(),
            })
        );
    }

    #[test]
    fn other_non_directory_is_not_entered() {
        let listed = listed("file", EntryKind::File, EntryIdentity::Other);

        assert_eq!(
            listed_entry_navigation_target(&Location::Local("/parent".into()), &listed),
            None
        );
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

    // ── S3-25: navigation_parent_target ──

    fn nav_registry() -> crate::vfs::ProviderRegistry {
        let registry = crate::vfs::ProviderRegistry::new();
        registry.register_s3_targets(&[
            crate::config::S3TargetConfig {
                id: "acc".into(),
                name: "acc".into(),
                bucket: None,
                region: None,
                profile: None,
                endpoint_url: None,
                force_path_style: false,
            },
            crate::config::S3TargetConfig {
                id: "bkt".into(),
                name: "bkt".into(),
                bucket: Some("company-artifacts".into()),
                region: None,
                profile: None,
                endpoint_url: None,
                force_path_style: false,
            },
        ]);
        registry
    }

    fn s3(target: &str, bucket: Option<&str>, prefix: &str) -> Location {
        Location::S3 {
            target: target.to_string(),
            bucket: bucket.map(|b| b.to_string()),
            prefix: prefix.to_string(),
        }
    }

    #[test]
    fn target_root_has_no_parent() {
        let registry = nav_registry();
        let current = s3("acc", None, "");
        assert_eq!(navigation_parent_target(&current, &registry), None);
    }

    #[test]
    fn account_bucket_root_parent_is_target_root() {
        let registry = nav_registry();
        let current = s3("acc", Some("anything"), "");
        assert_eq!(
            navigation_parent_target(&current, &registry),
            Some(s3("acc", None, ""))
        );
    }

    #[test]
    fn bucket_bound_root_has_no_parent() {
        let registry = nav_registry();
        let current = s3("bkt", Some("company-artifacts"), "");
        assert_eq!(navigation_parent_target(&current, &registry), None);
    }

    #[test]
    fn bucket_bound_wrong_bucket_fails_closed() {
        let registry = nav_registry();
        let current = s3("bkt", Some("other-bucket"), "");
        assert_eq!(navigation_parent_target(&current, &registry), None);
    }

    #[test]
    fn unknown_target_fails_closed() {
        let registry = nav_registry();
        let current = s3("ghost", Some("x"), "");
        assert_eq!(navigation_parent_target(&current, &registry), None);
    }

    #[test]
    fn prefix_single_segment_to_bucket_root() {
        let registry = nav_registry();
        let current = s3("bkt", Some("company-artifacts"), "foo");
        assert_eq!(
            navigation_parent_target(&current, &registry),
            Some(s3("bkt", Some("company-artifacts"), ""))
        );
    }

    #[test]
    fn nested_prefix_strips_last_segment() {
        let registry = nav_registry();
        let current = s3("bkt", Some("company-artifacts"), "foo/bar");
        assert_eq!(
            navigation_parent_target(&current, &registry),
            Some(s3("bkt", Some("company-artifacts"), "foo"))
        );
    }

    #[test]
    fn repeated_slash_literal_prefix() {
        let registry = nav_registry();
        let current = s3("bkt", Some("company-artifacts"), "foo/");
        assert_eq!(
            navigation_parent_target(&current, &registry),
            Some(s3("bkt", Some("company-artifacts"), "foo"))
        );
    }

    #[test]
    fn repeated_double_slash_prefix() {
        let registry = nav_registry();
        let current = s3("bkt", Some("company-artifacts"), "foo//");
        assert_eq!(
            navigation_parent_target(&current, &registry),
            Some(s3("bkt", Some("company-artifacts"), "foo/"))
        );
    }

    #[test]
    fn awkward_double_slash_nested_prefix() {
        let registry = nav_registry();
        let current = s3("bkt", Some("company-artifacts"), "foo//bar");
        assert_eq!(
            navigation_parent_target(&current, &registry),
            Some(s3("bkt", Some("company-artifacts"), "foo/"))
        );
    }

    #[test]
    fn dotdot_is_literal_prefix() {
        let registry = nav_registry();
        let current = s3("bkt", Some("company-artifacts"), "foo/../bar");
        assert_eq!(
            navigation_parent_target(&current, &registry),
            Some(s3("bkt", Some("company-artifacts"), "foo/.."))
        );
    }

    #[test]
    fn dot_is_literal_prefix() {
        let registry = nav_registry();
        let current = s3("bkt", Some("company-artifacts"), "foo/./bar");
        assert_eq!(
            navigation_parent_target(&current, &registry),
            Some(s3("bkt", Some("company-artifacts"), "foo/."))
        );
    }

    #[test]
    fn unicode_parent_prefix() {
        let registry = nav_registry();
        let current = s3("bkt", Some("company-artifacts"), "日本語/資料");
        assert_eq!(
            navigation_parent_target(&current, &registry),
            Some(s3("bkt", Some("company-artifacts"), "日本語"))
        );
    }

    #[test]
    fn local_parent_unchanged() {
        let registry = nav_registry();
        let current = Location::Local("/a/b".into());
        assert_eq!(
            navigation_parent_target(&current, &registry),
            Some(Location::Local("/a".into()))
        );
    }

    #[test]
    fn sftp_parent_unchanged() {
        let registry = nav_registry();
        let current = Location::Sftp {
            host: "h".into(),
            path: "/a/b".into(),
        };
        assert_eq!(
            navigation_parent_target(&current, &registry),
            Some(Location::Sftp {
                host: "h".into(),
                path: "/a".into()
            })
        );
    }

    #[test]
    fn archive_parent_unchanged() {
        let registry = nav_registry();
        let current = Location::Archive {
            archive: "/x.zip".into(),
            inner_path: "a/b".into(),
        };
        assert_eq!(
            navigation_parent_target(&current, &registry),
            Some(Location::Archive {
                archive: "/x.zip".into(),
                inner_path: "a".into()
            })
        );
    }
}
