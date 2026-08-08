from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f'{label}: expected 1 match, found {count}')
    return text.replace(old, new, 1)

# app/mod.rs: export the presentation state.
path = Path('src/app/mod.rs')
text = path.read_text()
text = replace_once(
    text,
    'mod remote_workspace;\npub use remote_workspace::RemoteWorkspaceState;\n',
    'mod remote_workspace;\npub use remote_workspace::RemoteWorkspaceState;\nmod workspace_sync_ux;\npub use workspace_sync_ux::WorkspaceSyncUxState;\n',
    'app UX export',
)
path.write_text(text)

# overlay.rs: hiding SyncPreview must only hide that overlay; it must not erase
# the workflow/job state and opening another overlay may reveal it again later.
path = Path('src/app/overlay.rs')
text = path.read_text()
old = '''    pub fn close_overlay(&mut self, overlay: OverlayKind) {
        if self.active_overlay() == Some(overlay) {
            self.close_all_overlays();
        }
    }

    pub fn toggle_overlay(&mut self, overlay: OverlayKind) {
        if self.active_overlay() == Some(overlay) {
            self.close_all_overlays();
        } else {
            self.open_overlay(overlay);
        }
    }
'''
new = '''    pub fn close_overlay(&mut self, overlay: OverlayKind) {
        if self.active_overlay() != Some(overlay) {
            return;
        }
        if overlay == OverlayKind::SyncPreview {
            self.remote_workspace.preview_open = false;
        } else {
            self.close_all_overlays();
        }
    }

    pub fn toggle_overlay(&mut self, overlay: OverlayKind) {
        if self.active_overlay() == Some(overlay) {
            self.close_overlay(overlay);
        } else {
            self.open_overlay(overlay);
        }
    }
'''
text = replace_once(text, old, new, 'overlay close semantics')
marker = '''    #[test]
    fn opening_cursor_overlay_resets_cursor() {'''
insert = '''    #[test]
    fn hiding_sync_overlay_preserves_sync_workflow_state() {
        let mut state = AppState::default();
        state.remote_workspace.ux = super::super::WorkspaceSyncUxState::Running {
            job_id: "sync-1".into(),
        };
        state.open_overlay(OverlayKind::SyncPreview);
        state.close_overlay(OverlayKind::SyncPreview);
        assert!(!state.remote_workspace.preview_open);
        assert!(matches!(
            state.remote_workspace.ux,
            super::super::WorkspaceSyncUxState::Running { .. }
        ));
    }

'''
if text.count(marker) != 1:
    raise SystemExit('overlay test anchor mismatch')
text = text.replace(marker, insert + marker, 1)
path.write_text(text)

# RemoteWorkspaceState: presentation state + frozen plan live beside the
# logical diff/plan, but never replace runtime Job/verification truth.
path = Path('src/app/remote_workspace.rs')
text = path.read_text()
text = replace_once(
    text,
    'use crate::services::{WorkspaceScanId, WorkspaceScanResponse};\n',
    'use super::WorkspaceSyncUxState;\nuse crate::jobs::{Job, JobResult, JobStatus};\nuse crate::services::{WorkspaceScanId, WorkspaceScanResponse};\n',
    'remote workspace UX imports 1',
)
text = replace_once(
    text,
    'use crate::workspace_sync_verification::{SyncVerificationSnapshot, SyncVerificationStatus};\n',
    'use crate::workspace_sync_execution::FrozenWorkspaceSyncPlan;\nuse crate::workspace_sync_verification::{SyncVerificationSnapshot, SyncVerificationStatus};\n',
    'remote workspace UX imports 2',
)
text = replace_once(
    text,
    '    pub plan: Option<WorkspaceSyncPlan>,\n    pub policy: SyncPolicy,\n',
    '    pub plan: Option<WorkspaceSyncPlan>,\n    /// Frozen preview selected for explicit execution/confirmation. It is\n    /// invalidated whenever the diff or policy changes.\n    pub frozen_plan: Option<FrozenWorkspaceSyncPlan>,\n    /// Presentation-only stage. Runtime truth remains in JobManager and the\n    /// verification coordinator.\n    pub ux: WorkspaceSyncUxState,\n    pub policy: SyncPolicy,\n',
    'remote workspace UX fields',
)
text = replace_once(
    text,
    '            plan: None,\n            policy: SyncPolicy::default(),\n',
    '            plan: None,\n            frozen_plan: None,\n            ux: WorkspaceSyncUxState::Idle,\n            policy: SyncPolicy::default(),\n',
    'remote workspace UX defaults',
)
text = replace_once(
    text,
    '        self.plan = None;\n        self.left_scan = None;\n',
    '        self.plan = None;\n        self.frozen_plan = None;\n        self.ux = WorkspaceSyncUxState::Idle;\n        self.left_scan = None;\n',
    'disable UX reset',
)
text = replace_once(
    text,
    '        self.diff = None;\n        self.plan = None;\n        Arc::clone(&self.scan_cancel)\n',
    '        self.diff = None;\n        self.plan = None;\n        self.frozen_plan = None;\n        if self.preview_open {\n            self.ux = WorkspaceSyncUxState::Scanning;\n        }\n        Arc::clone(&self.scan_cancel)\n',
    'scan UX reset',
)
# Rebuild invalidates any frozen permission/confirmation.
text = replace_once(
    text,
    '''    pub fn rebuild_plan(&mut self) {
        self.plan = self
            .diff
            .as_ref()
            .map(|diff| WorkspaceSyncPlan::build(diff, self.policy));
    }
''',
    '''    pub fn rebuild_plan(&mut self) {
        self.frozen_plan = None;
        self.plan = self
            .diff
            .as_ref()
            .map(|diff| WorkspaceSyncPlan::build(diff, self.policy));
        if self.preview_open && !self.ux.is_job_flow() {
            self.ux = WorkspaceSyncUxState::Preview { plan_id: None };
        }
    }

    pub fn set_frozen_plan(&mut self, frozen: FrozenWorkspaceSyncPlan) {
        let plan_id = frozen.id();
        if frozen.requires_confirmation() {
            self.ux = WorkspaceSyncUxState::ConfirmationRequired {
                plan_id,
                digest: frozen.digest(),
                destructive_operations: frozen.destructive_operations(),
            };
        } else {
            self.ux = WorkspaceSyncUxState::Launching { plan_id };
        }
        self.frozen_plan = Some(frozen);
    }

    pub fn mark_launching(&mut self) {
        if let Some(frozen) = &self.frozen_plan {
            self.ux = WorkspaceSyncUxState::Launching {
                plan_id: frozen.id(),
            };
        }
    }

    pub fn mark_blocked(&mut self, message: impl Into<String>) {
        self.ux = WorkspaceSyncUxState::Blocked {
            message: message.into(),
        };
    }

    pub fn mark_preview(&mut self) {
        self.frozen_plan = None;
        self.ux = WorkspaceSyncUxState::Preview { plan_id: None };
    }

    pub fn sync_from_job(&mut self, job: &Job) {
        let Some(context) = &job.sync_context else {
            return;
        };
        let same_workspace = self
            .diff
            .as_ref()
            .is_some_and(|diff| diff.left_root == context.left_root && diff.right_root == context.right_root);
        let current_job = self.ux.job_id().is_some_and(|id| id == job.id);
        if !same_workspace && !current_job {
            return;
        }

        self.ux = match job.status {
            JobStatus::Pending => WorkspaceSyncUxState::Queued {
                job_id: job.id.clone(),
            },
            JobStatus::Running => WorkspaceSyncUxState::Running {
                job_id: job.id.clone(),
            },
            JobStatus::Cancelling => WorkspaceSyncUxState::Cancelling {
                job_id: job.id.clone(),
            },
            JobStatus::Paused => WorkspaceSyncUxState::Running {
                job_id: job.id.clone(),
            },
            JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled => {
                let needs_verification = match &job.result {
                    Some(JobResult::WorkspaceSync(outcome)) => match outcome.terminal {
                        crate::workspace_sync_executor::SyncTerminalState::Completed => true,
                        crate::workspace_sync_executor::SyncTerminalState::Cancelled { .. }
                        | crate::workspace_sync_executor::SyncTerminalState::Failed { .. } => {
                            outcome.workspace_may_have_changed
                        }
                    },
                    _ => false,
                };
                if needs_verification && job.verification.as_ref().is_none_or(|item| !item.status.is_terminal()) {
                    WorkspaceSyncUxState::Verifying {
                        job_id: job.id.clone(),
                    }
                } else {
                    WorkspaceSyncUxState::Finished {
                        job_id: job.id.clone(),
                    }
                }
            }
        };
    }

    pub fn sync_verification_stage(&mut self, job_id: &str) {
        let Some(verification) = &self.verification else {
            return;
        };
        self.ux = if verification.status.is_terminal() {
            WorkspaceSyncUxState::Finished {
                job_id: job_id.to_string(),
            }
        } else {
            WorkspaceSyncUxState::Verifying {
                job_id: job_id.to_string(),
            }
        };
    }
''',
    'remote workspace UX methods',
)
# tests
marker = '''    #[test]
    fn mirror_is_never_the_default() {'''
insert = '''    #[test]
    fn policy_change_invalidates_frozen_execution_context() {
        let mut state = RemoteWorkspaceState::default();
        state.preview_open = true;
        state.refresh_visible(
            Location::Local(PathBuf::from("/left")),
            Location::Local(PathBuf::from("/right")),
            &[file("local.txt", 10)],
            &[],
        );
        assert!(matches!(state.ux, WorkspaceSyncUxState::Preview { .. }));
        state.toggle_mode();
        assert!(state.frozen_plan.is_none());
        assert!(matches!(state.ux, WorkspaceSyncUxState::Preview { .. }));
    }

'''
if text.count(marker) != 1:
    raise SystemExit('remote UX test anchor mismatch')
text = text.replace(marker, insert + marker, 1)
path.write_text(text)

# Actions/catalog/input contexts.
path = Path('src/app/actions.rs')
text = path.read_text()
for enum_name in ('ActionId', 'Action'):
    old = '    CloseWorkspaceSyncPreview,\n}'
    new = '''    CloseWorkspaceSyncPreview,
    ExecuteWorkspaceSync,
    ConfirmWorkspaceSync,
    CancelWorkspaceSync,
    ShowWorkspaceSyncDetails,
    ReturnToWorkspaceSyncPreview,
}'''
    # first replacement hits ActionId, second hits Action
    text = replace_once(text, old, new, f'{enum_name} sync action variants')
text = replace_once(
    text,
    '            Self::CloseWorkspaceSyncPreview => ActionId::CloseWorkspaceSyncPreview,\n',
    '''            Self::CloseWorkspaceSyncPreview => ActionId::CloseWorkspaceSyncPreview,
            Self::ExecuteWorkspaceSync => ActionId::ExecuteWorkspaceSync,
            Self::ConfirmWorkspaceSync => ActionId::ConfirmWorkspaceSync,
            Self::CancelWorkspaceSync => ActionId::CancelWorkspaceSync,
            Self::ShowWorkspaceSyncDetails => ActionId::ShowWorkspaceSyncDetails,
            Self::ReturnToWorkspaceSyncPreview => ActionId::ReturnToWorkspaceSyncPreview,
''',
    'action id mapping',
)
text = replace_once(
    text,
    '    Action::CloseWorkspaceSyncPreview,\n];\n',
    '''    Action::CloseWorkspaceSyncPreview,
    Action::ExecuteWorkspaceSync,
    Action::ConfirmWorkspaceSync,
    Action::CancelWorkspaceSync,
    Action::ShowWorkspaceSyncDetails,
    Action::ReturnToWorkspaceSyncPreview,
];
''',
    'all actions',
)
# append catalog entries before closing catalog
catalog_anchor = '''    ActionMeta {
        id: ActionId::CloseWorkspaceSyncPreview,
        label: "Close sync preview",
        description: "Return to the two-pane workspace",
        category: ActionCategory::Workspace,
        destructive: false,
    },
];'''
catalog_replacement = '''    ActionMeta {
        id: ActionId::CloseWorkspaceSyncPreview,
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
        id: ActionId::ReturnToWorkspaceSyncPreview,
        label: "Return to sync preview",
        description: "Return from confirmation or result details to the current diff preview",
        category: ActionCategory::Workspace,
        destructive: false,
    },
];'''
text = replace_once(text, catalog_anchor, catalog_replacement, 'action catalog sync entries')
text = replace_once(
    text,
    '    SyncPreview,\n    Bookmarks,\n',
    '    SyncPreview,\n    SyncConfirmation,\n    SyncJob,\n    Bookmarks,\n',
    'input context variants',
)
text = replace_once(
    text,
    '                Some(super::OverlayKind::SyncPreview) => InputContext::SyncPreview,\n',
    '''                Some(super::OverlayKind::SyncPreview) => match self.remote_workspace.ux {
                    super::WorkspaceSyncUxState::ConfirmationRequired { .. } => {
                        InputContext::SyncConfirmation
                    }
                    super::WorkspaceSyncUxState::Queued { .. }
                    | super::WorkspaceSyncUxState::Running { .. }
                    | super::WorkspaceSyncUxState::Cancelling { .. }
                    | super::WorkspaceSyncUxState::Verifying { .. }
                    | super::WorkspaceSyncUxState::Finished { .. } => InputContext::SyncJob,
                    _ => InputContext::SyncPreview,
                },
''',
    'input context sync routing',
)
path.write_text(text)

# keymap: all interaction flows through Actions rather than ad-hoc branches.
path = Path('src/input/keymap.rs')
text = path.read_text()
text = replace_once(
    text,
    '        use InputContext::{Browser, Help, SyncPreview};\n',
    '        use InputContext::{Browser, Help, SyncConfirmation, SyncJob, SyncPreview};\n',
    'keymap sync contexts import',
)
anchor = '''            KeyBinding::new(
                SyncPreview,
                vec![KeyStroke::new(KeyCode::Esc, NONE)],
                Action::CloseWorkspaceSyncPreview,
            ),
'''
replacement = anchor + '''            KeyBinding::new(
                SyncPreview,
                vec![KeyStroke::new(KeyCode::Enter, NONE)],
                Action::ExecuteWorkspaceSync,
            ),
            KeyBinding::new(
                SyncConfirmation,
                vec![KeyStroke::new(KeyCode::Enter, NONE)],
                Action::ConfirmWorkspaceSync,
            ),
            KeyBinding::new(
                SyncConfirmation,
                vec![KeyStroke::new(KeyCode::Esc, NONE)],
                Action::ReturnToWorkspaceSyncPreview,
            ),
            KeyBinding::new(SyncJob, vec![plain('c')], Action::CancelWorkspaceSync),
            KeyBinding::new(
                SyncJob,
                vec![KeyStroke::new(KeyCode::Esc, NONE)],
                Action::CloseWorkspaceSyncPreview,
            ),
            KeyBinding::new(
                SyncJob,
                vec![plain('b')],
                Action::ReturnToWorkspaceSyncPreview,
            ),
'''
text = replace_once(text, anchor, replacement, 'sync key bindings')
path.write_text(text)
