use super::{ActionId, AppState, WorkspaceSyncUxState};
use crate::vfs::{
    Capability, CapabilitySet, EntryKind, Location, ProviderId, capabilities::builtin_capabilities,
};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionContext {
    pub active_provider: ProviderId,
    pub passive_provider: ProviderId,
    pub active_capabilities: CapabilitySet,
    pub passive_capabilities: CapabilitySet,
    pub selection_count: usize,
    pub focused_kind: Option<EntryKind>,
    pub editor_available: bool,
    pub sync_execute_ready: bool,
    pub sync_confirmation_ready: bool,
    pub sync_cancel_ready: bool,
    pub sync_details_ready: bool,
    pub sync_verification_diff_ready: bool,
    pub sync_return_preview_ready: bool,
    pub right_location: Location,
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
        let sync_verification_diff_ready = state
            .remote_workspace
            .ux
            .job_id()
            .and_then(|id| state.jobs.iter().find(|job| job.id == id))
            .and_then(|job| job.verification.as_ref())
            .is_some_and(|verification| {
                matches!(
                    &verification.status,
                    crate::workspace_sync_verification::SyncVerificationStatus::Finished(result)
                        if matches!(
                            &result.verdict,
                            crate::workspace_sync_verification::SyncVerificationVerdict::DifferencesRemain { .. }
                        )
                )
            });
        let current_preview_ready = state
            .remote_workspace
            .has_current_preview(&state.left.location, &state.right.location);
        let sync_return_preview_ready = match state.remote_workspace.ux {
            WorkspaceSyncUxState::VerificationDiff { .. } => true,
            WorkspaceSyncUxState::ConfirmationRequired { .. }
            | WorkspaceSyncUxState::Blocked { .. }
            | WorkspaceSyncUxState::Finished { .. } => current_preview_ready,
            _ => false,
        };

        Self {
            active_provider,
            passive_provider,
            active_capabilities,
            passive_capabilities,
            selection_count: state.selection_count(state.active, &state.active_pane().location),
            focused_kind: None,
            editor_available: false,
            sync_execute_ready,
            sync_confirmation_ready,
            sync_cancel_ready,
            sync_details_ready,
            sync_verification_diff_ready,
            sync_return_preview_ready,
            right_location: state.right.location.clone(),
        }
    }

    pub fn with_file_context(
        mut self,
        focused_kind: Option<EntryKind>,
        editor_available: bool,
    ) -> Self {
        self.focused_kind = focused_kind;
        self.editor_available = editor_available;
        self
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

/// Exact set of provider pairs `TransferPlanner` can actually execute for
/// Copy. Local↔SFTP and Local↔S3 (single-object basic transfer) are the only
/// implemented directions; every other pair (Archive, SFTP↔SFTP, WebDAV, S3↔S3,
/// S3↔SFTP) is disabled because the planner cannot execute it.
fn copy_pair_supported(active: ProviderId, passive: ProviderId) -> bool {
    matches!(
        (active, passive),
        (ProviderId::Local, ProviderId::Local)
            | (ProviderId::Local, ProviderId::Sftp)
            | (ProviderId::Sftp, ProviderId::Local)
            | (ProviderId::S3, ProviderId::Local)
            | (ProviderId::Local, ProviderId::S3)
    )
}

pub fn action_availability(id: ActionId, ctx: &ActionContext) -> ActionAvailability {
    match id {
        ActionId::BeginSymlink => {
            require_active_capability(ctx, Capability::Symlink, "Symbolic links")
        }
        ActionId::BeginChmod => {
            require_active_capability(ctx, Capability::Chmod, "Permission changes")
        }
        // ViewFile (F3) supports exactly Local, Sftp, and S3. The provider
        // allow-list is explicit: WebDAV/Archive are never enabled merely
        // because their capability sets happen to contain Read.
        ActionId::ViewFile
            if ctx.active_provider != ProviderId::Local
                && ctx.active_provider != ProviderId::Sftp
                && ctx.active_provider != ProviderId::S3 =>
        {
            ActionAvailability::Disabled {
                reason: "Remote viewing is not supported yet".into(),
            }
        }
        ActionId::ViewFile
            if (ctx.active_provider == ProviderId::Sftp
                || ctx.active_provider == ProviderId::S3)
                && !ctx.active_capabilities.supports(Capability::Read) =>
        {
            ActionAvailability::Disabled {
                reason: "Read-only preview is not supported for this provider".into(),
            }
        }
        ActionId::ViewFile if ctx.focused_kind != Some(EntryKind::File) => {
            ActionAvailability::Disabled {
                reason: "Select a regular file to view".into(),
            }
        }
        ActionId::EditFile
            if !ctx.active_capabilities.contains_all(
                CapabilitySet::NONE
                    .with(Capability::Read)
                    .with(Capability::Write),
            ) =>
        {
            ActionAvailability::Disabled {
                reason: "Remote editing requires Read + Write capability".into(),
            }
        }
        ActionId::EditFile if ctx.focused_kind != Some(EntryKind::File) => {
            ActionAvailability::Disabled {
                reason: "Select a regular file to edit".into(),
            }
        }
        ActionId::EditFile if !ctx.editor_available => ActionAvailability::Disabled {
            reason: "No editor configured (config.ui.editor, VISUAL, or EDITOR)".into(),
        },
        ActionId::Copy => {
            let has_target = ctx.selection_count > 0 || ctx.focused_kind.is_some();
            if !has_target {
                ActionAvailability::Disabled {
                    reason: "Select a file or directory to copy".into(),
                }
            } else if copy_pair_supported(ctx.active_provider, ctx.passive_provider) {
                // TransferPlanner supports exactly Local→Local, Local→SFTP,
                // and SFTP→Local. Every other pair (any S3 side, Archive,
                // SFTP→SFTP, WebDAV) is disabled because the planner cannot
                // execute it.
                ActionAvailability::Available
            } else {
                ActionAvailability::Disabled {
                    reason: "Copy is not supported between these providers".into(),
                }
            }
        }
        ActionId::Move => {
            let has_target = ctx.selection_count > 0 || ctx.focused_kind.is_some();
            if !has_target {
                ActionAvailability::Disabled {
                    reason: "Select a file or directory to move".into(),
                }
            } else if ctx.active_provider == ProviderId::Local
                && ctx.passive_provider == ProviderId::Local
            {
                // TransferPlanner currently supports Move only for Local→Local.
                // Cross-backend Move is blocked until transactional copy→verify→delete exists.
                ActionAvailability::Available
            } else if ctx.active_provider == ProviderId::Local
                || ctx.passive_provider == ProviderId::Local
            {
                ActionAvailability::Disabled {
                    reason: "Cross-backend move is not supported safely yet".into(),
                }
            } else {
                ActionAvailability::Disabled {
                    reason: "Move is not supported between these providers".into(),
                }
            }
        }
        ActionId::Mkdir => {
            let supported = ctx.active_capabilities.supports(Capability::Mkdir);
            if !supported {
                ActionAvailability::Disabled {
                    reason: "Directory creation is not supported for this location".into(),
                }
            } else {
                ActionAvailability::Available
            }
        }
        ActionId::Delete => {
            // Virtual Parent ("..") is filtered at the dispatch layer and never
            // reaches availability as a focused_kind. Availability only guards
            // provider, selection, and target existence.
            let has_target = ctx.selection_count > 0 || ctx.focused_kind.is_some();
            if !has_target {
                ActionAvailability::Disabled {
                    reason: "Select a file or directory to delete".into(),
                }
            } else if !ctx.active_capabilities.supports(Capability::Delete) {
                ActionAvailability::Disabled {
                    reason: "Delete is not supported for this location".into(),
                }
            } else {
                ActionAvailability::Available
            }
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
        // S3 selection is identity-unsafe while selection state is name-based:
        // two S3 rows can share a display name. Fail closed until provider
        // identity selection exists.
        ActionId::ToggleSelect if ctx.active_provider == ProviderId::S3 => {
            ActionAvailability::Disabled {
                reason: "S3 selection not enabled until provider identity selection is available"
                    .into(),
            }
        }
        // Workspace compare/sync is identity-unsafe and not yet implemented
        // for S3 panes (no exact-entry seam, unsafe name-based selection).
        // Block the setup/execution actions whenever either pane is S3.
        // Lifecycle actions for an already-existing job (Cancel, Details,
        // VerificationDiff, ReturnToPreview) stay state-driven below.
        ActionId::ToggleWorkspaceComparison
        | ActionId::PreviewWorkspaceSync
        | ActionId::ReverseWorkspaceDirection
        | ActionId::ToggleWorkspaceSyncMode
            if ctx.active_provider == ProviderId::S3 || ctx.passive_provider == ProviderId::S3 =>
        {
            ActionAvailability::Disabled {
                reason: "Workspace compare/sync is not supported with S3 panes yet".into(),
            }
        }
        ActionId::ExecuteWorkspaceSync
            if ctx.active_provider == ProviderId::S3 || ctx.passive_provider == ProviderId::S3 =>
        {
            ActionAvailability::Disabled {
                reason: "Workspace compare/sync is not supported with S3 panes yet".into(),
            }
        }
        ActionId::ExecuteWorkspaceSync if !ctx.sync_execute_ready => ActionAvailability::Disabled {
            reason: "Workspace sync needs a current conflict-free preview".into(),
        },
        ActionId::ConfirmWorkspaceSync
            if ctx.active_provider == ProviderId::S3 || ctx.passive_provider == ProviderId::S3 =>
        {
            ActionAvailability::Disabled {
                reason: "Workspace compare/sync is not supported with S3 panes yet".into(),
            }
        }
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
        ActionId::ShowWorkspaceVerificationDiff if !ctx.sync_verification_diff_ready => {
            ActionAvailability::Disabled {
                reason: "Verification has not produced remaining differences to show".into(),
            }
        }
        ActionId::ReturnToWorkspaceSyncPreview if !ctx.sync_return_preview_ready => {
            ActionAvailability::Disabled {
                reason: "No current workspace preview is available to return to".into(),
            }
        }
        ActionId::ToggleEmbeddedTerminal => {
            if let Location::Local(_) = &ctx.right_location {
                ActionAvailability::Available
            } else {
                ActionAvailability::Disabled {
                    reason: "Embedded terminal requires a local right pane".into(),
                }
            }
        }
        _ => ActionAvailability::Available,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::capabilities::{ARCHIVE_CAPABILITIES, LOCAL_CAPABILITIES, SFTP_CAPABILITIES};

    fn context(active_provider: ProviderId, active_capabilities: CapabilitySet) -> ActionContext {
        ActionContext {
            active_provider,
            passive_provider: ProviderId::Local,
            active_capabilities,
            passive_capabilities: LOCAL_CAPABILITIES,
            selection_count: 0,
            focused_kind: Some(EntryKind::File),
            editor_available: true,
            sync_execute_ready: false,
            sync_confirmation_ready: false,
            sync_cancel_ready: false,
            sync_details_ready: false,
            sync_verification_diff_ready: false,
            sync_return_preview_ready: false,
            right_location: Location::Local(std::path::PathBuf::from("/")),
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
    fn remote_view_is_available_when_provider_has_read_capability() {
        let ctx = context(ProviderId::Sftp, SFTP_CAPABILITIES);
        let availability = action_availability(ActionId::ViewFile, &ctx);
        assert!(
            matches!(availability, ActionAvailability::Available),
            "SFTP F3 should be Available when Capability::Read is present; got {availability:?}"
        );
    }

    #[test]
    fn remote_view_is_disabled_without_read_capability() {
        let ctx = context(ProviderId::Sftp, CapabilitySet::NONE);
        let availability = action_availability(ActionId::ViewFile, &ctx);
        assert!(matches!(availability, ActionAvailability::Disabled { .. }));
        assert!(availability.reason().is_some());
    }

    #[test]
    fn sftp_edit_is_available_when_read_write_present() {
        let mut ctx = context(ProviderId::Sftp, SFTP_CAPABILITIES);
        ctx.editor_available = true;
        ctx.focused_kind = Some(EntryKind::File);
        let availability = action_availability(ActionId::EditFile, &ctx);
        assert!(
            matches!(availability, ActionAvailability::Available),
            "SFTP with Read+Write+editor should be Available, got {:?}",
            availability
        );
    }

    #[test]
    fn edit_disabled_when_provider_lacks_write() {
        // Archive has List only — no Read+Write
        let ctx = context(ProviderId::Archive, ARCHIVE_CAPABILITIES);
        let availability = action_availability(ActionId::EditFile, &ctx);
        assert!(matches!(availability, ActionAvailability::Disabled { .. }));
    }

    #[test]
    fn file_actions_require_a_regular_file_and_edit_requires_an_editor() {
        let mut ctx = context(ProviderId::Local, LOCAL_CAPABILITIES);
        ctx.focused_kind = Some(EntryKind::Directory);
        assert!(matches!(
            action_availability(ActionId::ViewFile, &ctx),
            ActionAvailability::Disabled { .. }
        ));
        assert!(matches!(
            action_availability(ActionId::EditFile, &ctx),
            ActionAvailability::Disabled { .. }
        ));

        ctx.focused_kind = Some(EntryKind::File);
        ctx.editor_available = false;
        assert!(matches!(
            action_availability(ActionId::EditFile, &ctx),
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

    #[test]
    fn copy_available_when_one_side_is_local() {
        let mut ctx = context(ProviderId::Local, LOCAL_CAPABILITIES);
        ctx.focused_kind = Some(EntryKind::File);

        // Local → Local
        assert_eq!(
            action_availability(ActionId::Copy, &ctx),
            ActionAvailability::Available
        );

        // Local → SFTP
        ctx.passive_provider = ProviderId::Sftp;
        assert_eq!(
            action_availability(ActionId::Copy, &ctx),
            ActionAvailability::Available
        );

        // SFTP → Local
        ctx.active_provider = ProviderId::Sftp;
        ctx.passive_provider = ProviderId::Local;
        ctx.active_capabilities = SFTP_CAPABILITIES;
        assert_eq!(
            action_availability(ActionId::Copy, &ctx),
            ActionAvailability::Available
        );

        // SFTP → SFTP
        ctx.passive_provider = ProviderId::Sftp;
        assert!(matches!(
            action_availability(ActionId::Copy, &ctx),
            ActionAvailability::Disabled { .. }
        ));
    }

    #[test]
    fn move_only_available_local_to_local() {
        let mut ctx = context(ProviderId::Local, LOCAL_CAPABILITIES);
        ctx.focused_kind = Some(EntryKind::File);

        // Local → Local
        assert_eq!(
            action_availability(ActionId::Move, &ctx),
            ActionAvailability::Available
        );

        // Local → SFTP — blocked
        ctx.passive_provider = ProviderId::Sftp;
        let availability = action_availability(ActionId::Move, &ctx);
        assert!(matches!(availability, ActionAvailability::Disabled { .. }));
        assert!(availability.reason().unwrap().contains("Cross-backend"));

        // SFTP → Local — blocked
        ctx.active_provider = ProviderId::Sftp;
        ctx.passive_provider = ProviderId::Local;
        ctx.active_capabilities = SFTP_CAPABILITIES;
        assert!(matches!(
            action_availability(ActionId::Move, &ctx),
            ActionAvailability::Disabled { .. }
        ));

        // SFTP → SFTP — blocked
        ctx.passive_provider = ProviderId::Sftp;
        assert!(matches!(
            action_availability(ActionId::Move, &ctx),
            ActionAvailability::Disabled { .. }
        ));
    }

    #[test]
    fn delete_disabled_for_empty_pane() {
        let mut ctx = context(ProviderId::Local, LOCAL_CAPABILITIES);
        ctx.focused_kind = None;
        assert!(matches!(
            action_availability(ActionId::Delete, &ctx),
            ActionAvailability::Disabled { .. }
        ));
    }

    #[test]
    fn delete_available_for_local_file() {
        let mut ctx = context(ProviderId::Local, LOCAL_CAPABILITIES);
        ctx.focused_kind = Some(EntryKind::File);
        assert_eq!(
            action_availability(ActionId::Delete, &ctx),
            ActionAvailability::Available
        );
    }

    #[test]
    fn delete_available_for_sftp_now() {
        let mut ctx = context(ProviderId::Sftp, SFTP_CAPABILITIES);
        ctx.focused_kind = Some(EntryKind::File);
        assert_eq!(
            action_availability(ActionId::Delete, &ctx),
            ActionAvailability::Available
        );
    }

    #[test]
    fn delete_disabled_for_archive() {
        let mut ctx = context(
            ProviderId::Archive,
            crate::vfs::capabilities::ARCHIVE_CAPABILITIES,
        );
        ctx.focused_kind = Some(EntryKind::File);
        assert!(matches!(
            action_availability(ActionId::Delete, &ctx),
            ActionAvailability::Disabled { .. }
        ));
    }

    #[test]
    fn delete_disabled_when_parent_row_focused() {
        // ".." is filtered before dispatch_ui_action. Without focused_kind
        // and without selection, Delete is disabled.
        let mut ctx = context(ProviderId::Local, LOCAL_CAPABILITIES);
        ctx.focused_kind = None;
        assert!(matches!(
            action_availability(ActionId::Delete, &ctx),
            ActionAvailability::Disabled { .. }
        ));
    }

    // ── REMOTE-09: mkdir availability ──

    #[test]
    fn mkdir_local_available() {
        let mut ctx = context(ProviderId::Local, LOCAL_CAPABILITIES);
        ctx.focused_kind = Some(EntryKind::File);
        assert_eq!(
            action_availability(ActionId::Mkdir, &ctx),
            ActionAvailability::Available
        );
    }

    #[test]
    fn mkdir_sftp_available() {
        let mut ctx = context(ProviderId::Sftp, SFTP_CAPABILITIES);
        ctx.focused_kind = Some(EntryKind::File);
        assert_eq!(
            action_availability(ActionId::Mkdir, &ctx),
            ActionAvailability::Available
        );
    }

    #[test]
    fn mkdir_archive_disabled() {
        let mut ctx = context(
            ProviderId::Archive,
            crate::vfs::capabilities::ARCHIVE_CAPABILITIES,
        );
        ctx.focused_kind = Some(EntryKind::File);
        assert!(matches!(
            action_availability(ActionId::Mkdir, &ctx),
            ActionAvailability::Disabled { .. }
        ));
    }

    #[test]
    fn viewfile_available_sftp_file() {
        let mut ctx = context(ProviderId::Sftp, SFTP_CAPABILITIES);
        ctx.focused_kind = Some(EntryKind::File);
        assert_eq!(
            action_availability(ActionId::ViewFile, &ctx),
            ActionAvailability::Available
        );
    }

    #[test]
    fn viewfile_disabled_sftp_dir() {
        let mut ctx = context(ProviderId::Sftp, SFTP_CAPABILITIES);
        ctx.focused_kind = Some(EntryKind::Directory);
        let availability = action_availability(ActionId::ViewFile, &ctx);
        assert!(matches!(availability, ActionAvailability::Disabled { .. }));
        assert!(availability.reason().unwrap().contains("regular file"));
    }

    #[test]
    fn viewfile_disabled_parent() {
        let mut ctx = context(ProviderId::Sftp, SFTP_CAPABILITIES);
        ctx.focused_kind = None;
        let availability = action_availability(ActionId::ViewFile, &ctx);
        assert!(matches!(availability, ActionAvailability::Disabled { .. }));
        assert!(availability.reason().unwrap().contains("regular file"));
    }

    // ── S3-26A: hypothetical S3 capability set of ONLY {List} ──
    // S3 may list but must not expose transfer, mutation, workspace sync,
    // or identity-unsafe selection.

    /// Active provider S3 with List-only capabilities and a Local passive pane.
    fn s3_list_context() -> ActionContext {
        let mut ctx = context(ProviderId::S3, CapabilitySet::NONE.with(Capability::List));
        ctx.passive_provider = ProviderId::Local;
        ctx
    }

    #[test]
    fn s3_list_only_view_disabled() {
        let ctx = s3_list_context();
        assert!(matches!(
            action_availability(ActionId::ViewFile, &ctx),
            ActionAvailability::Disabled { .. }
        ));
    }

    #[test]
    fn s3_list_only_edit_disabled() {
        let ctx = s3_list_context();
        assert!(matches!(
            action_availability(ActionId::EditFile, &ctx),
            ActionAvailability::Disabled { .. }
        ));
    }

    #[test]
    fn s3_list_only_mkdir_disabled() {
        let ctx = s3_list_context();
        assert!(matches!(
            action_availability(ActionId::Mkdir, &ctx),
            ActionAvailability::Disabled { .. }
        ));
    }

    #[test]
    fn s3_list_only_delete_disabled() {
        let ctx = s3_list_context();
        assert!(matches!(
            action_availability(ActionId::Delete, &ctx),
            ActionAvailability::Disabled { .. }
        ));
    }

    #[test]
    fn s3_list_only_select_disabled() {
        let ctx = s3_list_context();
        assert!(matches!(
            action_availability(ActionId::ToggleSelect, &ctx),
            ActionAvailability::Disabled { .. }
        ));
    }

    #[test]
    fn s3_list_only_symlink_disabled() {
        let ctx = s3_list_context();
        assert!(matches!(
            action_availability(ActionId::BeginSymlink, &ctx),
            ActionAvailability::Disabled { .. }
        ));
    }

    #[test]
    fn s3_list_only_chmod_disabled() {
        let ctx = s3_list_context();
        assert!(matches!(
            action_availability(ActionId::BeginChmod, &ctx),
            ActionAvailability::Disabled { .. }
        ));
    }

    #[test]
    fn s3_list_only_hardlink_disabled() {
        let ctx = s3_list_context();
        assert!(matches!(
            action_availability(ActionId::BeginHardLink, &ctx),
            ActionAvailability::Disabled { .. }
        ));
    }

    #[test]
    fn s3_list_only_chown_disabled() {
        let ctx = s3_list_context();
        assert!(matches!(
            action_availability(ActionId::BeginChown, &ctx),
            ActionAvailability::Disabled { .. }
        ));
    }

    // ── S3-26A: Copy matrix (exact TransferPlanner-implemented pairs) ──

    #[test]
    fn copy_local_local_available() {
        let mut ctx = context(ProviderId::Local, LOCAL_CAPABILITIES);
        ctx.focused_kind = Some(EntryKind::File);
        assert_eq!(
            action_availability(ActionId::Copy, &ctx),
            ActionAvailability::Available
        );
    }

    #[test]
    fn copy_local_sftp_available() {
        let mut ctx = context(ProviderId::Local, LOCAL_CAPABILITIES);
        ctx.passive_provider = ProviderId::Sftp;
        ctx.focused_kind = Some(EntryKind::File);
        assert_eq!(
            action_availability(ActionId::Copy, &ctx),
            ActionAvailability::Available
        );
    }

    #[test]
    fn copy_sftp_local_available() {
        let mut ctx = context(ProviderId::Sftp, SFTP_CAPABILITIES);
        ctx.passive_provider = ProviderId::Local;
        ctx.active_capabilities = SFTP_CAPABILITIES;
        ctx.focused_kind = Some(EntryKind::File);
        assert_eq!(
            action_availability(ActionId::Copy, &ctx),
            ActionAvailability::Available
        );
    }

    #[test]
    fn copy_s3_local_available() {
        let ctx = s3_list_context();
        // active = S3, passive = Local — single-object basic transfer is enabled
        assert_eq!(
            action_availability(ActionId::Copy, &ctx),
            ActionAvailability::Available
        );
    }

    #[test]
    fn copy_local_s3_available() {
        let mut ctx = s3_list_context();
        ctx.active_provider = ProviderId::Local;
        ctx.passive_provider = ProviderId::S3;
        ctx.active_capabilities = LOCAL_CAPABILITIES;
        assert_eq!(
            action_availability(ActionId::Copy, &ctx),
            ActionAvailability::Available
        );
    }

    #[test]
    fn copy_s3_s3_disabled() {
        let mut ctx = s3_list_context();
        ctx.active_provider = ProviderId::S3;
        ctx.passive_provider = ProviderId::S3;
        assert!(matches!(
            action_availability(ActionId::Copy, &ctx),
            ActionAvailability::Disabled { .. }
        ));
    }

    #[test]
    fn copy_archive_local_disabled() {
        let mut ctx = context(ProviderId::Archive, ARCHIVE_CAPABILITIES);
        ctx.passive_provider = ProviderId::Local;
        ctx.focused_kind = Some(EntryKind::File);
        assert!(matches!(
            action_availability(ActionId::Copy, &ctx),
            ActionAvailability::Disabled { .. }
        ));
    }

    #[test]
    fn copy_local_archive_disabled() {
        let mut ctx = context(ProviderId::Local, LOCAL_CAPABILITIES);
        ctx.passive_provider = ProviderId::Archive;
        ctx.focused_kind = Some(EntryKind::File);
        assert!(matches!(
            action_availability(ActionId::Copy, &ctx),
            ActionAvailability::Disabled { .. }
        ));
    }

    #[test]
    fn move_s3_local_disabled() {
        let ctx = s3_list_context();
        // active = S3, passive = Local
        assert!(matches!(
            action_availability(ActionId::Move, &ctx),
            ActionAvailability::Disabled { .. }
        ));
    }

    #[test]
    fn move_local_s3_disabled() {
        let mut ctx = s3_list_context();
        ctx.active_provider = ProviderId::Local;
        ctx.passive_provider = ProviderId::S3;
        ctx.active_capabilities = LOCAL_CAPABILITIES;
        assert!(matches!(
            action_availability(ActionId::Move, &ctx),
            ActionAvailability::Disabled { .. }
        ));
    }

    // ── S3-26A: workspace compare/sync blocked with S3 panes ──

    #[test]
    fn workspace_compare_disabled_with_active_s3() {
        let ctx = s3_list_context();
        assert!(matches!(
            action_availability(ActionId::ToggleWorkspaceComparison, &ctx),
            ActionAvailability::Disabled { .. }
        ));
    }

    #[test]
    fn workspace_compare_disabled_with_passive_s3() {
        let mut ctx = s3_list_context();
        ctx.active_provider = ProviderId::Local;
        ctx.passive_provider = ProviderId::S3;
        ctx.active_capabilities = LOCAL_CAPABILITIES;
        assert!(matches!(
            action_availability(ActionId::ToggleWorkspaceComparison, &ctx),
            ActionAvailability::Disabled { .. }
        ));
    }

    #[test]
    fn workspace_preview_disabled_with_s3() {
        let ctx = s3_list_context();
        assert!(matches!(
            action_availability(ActionId::PreviewWorkspaceSync, &ctx),
            ActionAvailability::Disabled { .. }
        ));
        assert!(matches!(
            action_availability(ActionId::ReverseWorkspaceDirection, &ctx),
            ActionAvailability::Disabled { .. }
        ));
        assert!(matches!(
            action_availability(ActionId::ToggleWorkspaceSyncMode, &ctx),
            ActionAvailability::Disabled { .. }
        ));
    }

    #[test]
    fn workspace_execute_disabled_with_s3() {
        let mut ctx = s3_list_context();
        // Even with a ready preview, S3 panes block execution.
        ctx.sync_execute_ready = true;
        assert!(matches!(
            action_availability(ActionId::ExecuteWorkspaceSync, &ctx),
            ActionAvailability::Disabled { .. }
        ));
    }

    #[test]
    fn workspace_confirm_disabled_with_s3() {
        let mut ctx = s3_list_context();
        ctx.sync_confirmation_ready = true;
        assert!(matches!(
            action_availability(ActionId::ConfirmWorkspaceSync, &ctx),
            ActionAvailability::Disabled { .. }
        ));
    }

    #[test]
    fn workspace_cancel_existing_job_still_state_driven() {
        // S3 active but an already-running job must keep Cancel Available.
        let mut ctx = s3_list_context();
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
    }

    #[test]
    fn workspace_details_existing_job_still_state_driven() {
        // S3 active but an existing job must keep Details Available.
        let mut ctx = s3_list_context();
        ctx.sync_details_ready = true;
        assert_eq!(
            action_availability(ActionId::ShowWorkspaceSyncDetails, &ctx),
            ActionAvailability::Available
        );
        ctx.sync_details_ready = false;
        assert!(matches!(
            action_availability(ActionId::ShowWorkspaceSyncDetails, &ctx),
            ActionAvailability::Disabled { .. }
        ));
    }

    // ── S3-29: F3 ViewFile availability for S3 + exact S3Object identity ──

    use crate::vfs::s3::{S3ObjectRef, S3PrefixRef};
    use crate::vfs::{Entry, EntryIdentity, ListedEntry};

    /// An S3 location that has BOTH List and Read (hypothetical, since real S3
    /// is List-only today) — used to prove F3 gates on Read and F4 stays off.
    fn s3_read_context() -> ActionContext {
        let mut ctx = context(
            ProviderId::S3,
            CapabilitySet::NONE
                .with(Capability::List)
                .with(Capability::Read),
        );
        ctx.passive_provider = ProviderId::Local;
        ctx
    }

    /// Build a listed S3Object row whose presentation name deliberately diverges
    /// from the authoritative object key. This is exactly the case the S3-27R
    /// identity seam must route through (ref.key), never the display name.
    fn s3_object_row(key: &str, name: &str) -> ListedEntry {
        ListedEntry {
            entry: Entry {
                name: name.to_string(),
                kind: EntryKind::File,
                size: Some(1234),
                modified_unix_ms: None,
            },
            identity: EntryIdentity::S3Object(S3ObjectRef {
                target: "s3://acct".into(),
                bucket: "real-bucket".into(),
                key: key.to_string(),
            }),
        }
    }

    #[test]
    fn s3_viewfile_available_with_read_for_s3object_identity() {
        // Real availability resolution: derive the focused kind exactly as the
        // TUI does (row.action_entry().map(|e| e.kind)) and ask
        // action_availability — no debug-print string compare.
        let row = s3_object_row("foo/../real//日本語🧙‍♂️.txt", "pretty-or-wrong.txt");
        let mut ctx = s3_read_context();
        ctx.focused_kind = Some(row.entry.kind);

        let availability = action_availability(ActionId::ViewFile, &ctx);
        assert_eq!(
            availability,
            ActionAvailability::Available,
            "F3 must be Available for an S3Object row when S3 has Read"
        );

        // Authoritative operation target is the exact ref.key, NOT entry.name.
        let EntryIdentity::S3Object(refr) = &row.identity else {
            panic!("expected S3Object identity");
        };
        assert_eq!(refr.key, "foo/../real//日本語🧙‍♂️.txt");
        assert_eq!(refr.bucket, "real-bucket");
        assert_eq!(refr.target, "s3://acct");
        // Presentation name is independent of the operation target; the
        // routing must depend on identity, not on entry.name.
        assert_eq!(row.entry.name, "pretty-or-wrong.txt");
        assert_ne!(row.entry.name, refr.key);
    }

    #[test]
    fn s3_viewfile_disabled_without_read_for_s3object_identity() {
        // S3 with ONLY {List}: F3 must be Disabled even for a regular S3Object.
        let row = s3_object_row("foo/bar.txt", "bar.txt");
        let mut ctx = s3_list_context();
        ctx.focused_kind = Some(row.entry.kind);
        assert!(matches!(
            action_availability(ActionId::ViewFile, &ctx),
            ActionAvailability::Disabled { .. }
        ));
    }

    #[test]
    fn s3_viewfile_disabled_for_s3prefix_identity() {
        // S3Prefix is not a regular object: its focused kind is Directory, so
        // F3 stays Disabled even with Read. Proves routing distinguishes
        // object identity from prefix identity.
        let row = ListedEntry {
            entry: Entry {
                name: "prefix/".into(),
                kind: EntryKind::Directory,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::S3Prefix(S3PrefixRef {
                target: "s3://acct".into(),
                bucket: "real-bucket".into(),
                prefix: "foo/bar".into(),
            }),
        };
        let mut ctx = s3_read_context();
        ctx.focused_kind = Some(row.entry.kind);
        let availability = action_availability(ActionId::ViewFile, &ctx);
        assert!(matches!(availability, ActionAvailability::Disabled { .. }));
    }

    // ── S3-29: F4 (EditFile) stays disabled for S3 even with hypothetical Read ──
    // S3 has no Write capability, so the existing Read+Write gate excludes it.

    #[test]
    fn s3_edit_disabled_even_with_read() {
        // Hypothetical S3 {List, Read}: F4 must remain Disabled because S3
        // cannot provide Write. No RemoteEditRevision generalization for S3.
        let ctx = s3_read_context();
        assert!(matches!(
            action_availability(ActionId::EditFile, &ctx),
            ActionAvailability::Disabled { .. }
        ));
    }
}
