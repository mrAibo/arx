use super::WorkspaceSyncUxState;
use crate::jobs::{Job, JobResult, JobStatus};
use crate::services::{WorkspaceScanId, WorkspaceScanResponse};
use crate::vfs::{Entry, Location};
use crate::workspace_sync::{
    DiffState, SyncDirection, SyncMode, SyncPolicy, WorkspaceDiff, WorkspaceEntry,
    WorkspaceFingerprint, WorkspaceSide, WorkspaceSyncPlan,
};
use crate::workspace_sync_execution::FrozenWorkspaceSyncPlan;
use crate::workspace_sync_verification::{SyncVerificationSnapshot, SyncVerificationStatus};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

#[derive(Debug, Clone)]
pub struct RemoteWorkspaceState {
    pub enabled: bool,
    pub preview_open: bool,
    pub diff: Option<WorkspaceDiff>,
    pub plan: Option<WorkspaceSyncPlan>,
    /// Frozen preview selected for explicit execution/confirmation. It is
    /// invalidated whenever the diff or policy changes.
    pub frozen_plan: Option<FrozenWorkspaceSyncPlan>,
    /// Presentation-only stage. Runtime truth remains in JobManager and the
    /// verification coordinator.
    pub ux: WorkspaceSyncUxState,
    pub policy: SyncPolicy,
    pub left_scan: Option<WorkspaceScanId>,
    pub right_scan: Option<WorkspaceScanId>,
    pub left_entries: Option<Vec<WorkspaceEntry>>,
    pub right_entries: Option<Vec<WorkspaceEntry>>,
    pub scan_cancel: Arc<AtomicBool>,
    /// Latest post-sync verification for the currently displayed roots.
    pub verification: Option<SyncVerificationSnapshot>,
}

impl Default for RemoteWorkspaceState {
    fn default() -> Self {
        Self {
            enabled: false,
            preview_open: false,
            diff: None,
            plan: None,
            frozen_plan: None,
            ux: WorkspaceSyncUxState::Idle,
            policy: SyncPolicy::default(),
            left_scan: None,
            right_scan: None,
            left_entries: None,
            right_entries: None,
            scan_cancel: Arc::new(AtomicBool::new(false)),
            verification: None,
        }
    }
}

impl RemoteWorkspaceState {
    pub fn disable(&mut self) {
        self.scan_cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let preserve_session =
            self.ux.is_job_flow() || matches!(self.ux, WorkspaceSyncUxState::Launching { .. });
        let preserved_ux = preserve_session.then(|| self.ux.clone());
        let preserved_frozen = matches!(self.ux, WorkspaceSyncUxState::Launching { .. })
            .then(|| self.frozen_plan.clone())
            .flatten();

        self.enabled = false;
        self.preview_open = false;
        self.diff = None;
        self.plan = None;
        self.frozen_plan = preserved_frozen;
        self.ux = preserved_ux.unwrap_or(WorkspaceSyncUxState::Idle);
        self.left_scan = None;
        self.right_scan = None;
        self.left_entries = None;
        self.right_entries = None;
        self.verification = None;
    }

    pub fn begin_recursive_scan(&mut self) -> Arc<AtomicBool> {
        self.scan_cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.scan_cancel = Arc::new(AtomicBool::new(false));
        self.supersede_active_verification();
        self.left_entries = None;
        self.right_entries = None;
        self.diff = None;
        self.plan = None;
        self.frozen_plan = None;
        if self.preview_open {
            self.ux = WorkspaceSyncUxState::Scanning;
        }
        Arc::clone(&self.scan_cancel)
    }

    pub fn register_scan(&mut self, side: WorkspaceSide, id: WorkspaceScanId) {
        match side {
            WorkspaceSide::Left => self.left_scan = Some(id),
            WorkspaceSide::Right => self.right_scan = Some(id),
        }
    }

    pub fn accepts_scan(&self, response: &WorkspaceScanResponse) -> bool {
        match response.side {
            WorkspaceSide::Left => self.left_scan == Some(response.id),
            WorkspaceSide::Right => self.right_scan == Some(response.id),
        }
    }

    pub fn finish_scan(&mut self, side: WorkspaceSide, id: WorkspaceScanId) {
        match side {
            WorkspaceSide::Left if self.left_scan == Some(id) => self.left_scan = None,
            WorkspaceSide::Right if self.right_scan == Some(id) => self.right_scan = None,
            _ => {}
        }
    }

    pub fn try_build_recursive_diff(&mut self, left_root: Location, right_root: Location) -> bool {
        let (Some(left), Some(right)) = (&self.left_entries, &self.right_entries) else {
            return false;
        };
        self.diff = Some(WorkspaceDiff::compare(
            left_root,
            right_root,
            left.clone(),
            right.clone(),
        ));
        self.rebuild_plan();
        true
    }

    /// Build an immediate shallow comparison from the entries already visible
    /// in both panes. Rich metadata/hashes may replace this snapshot
    /// asynchronously without changing the UI model.
    pub fn refresh_visible(
        &mut self,
        left_root: Location,
        right_root: Location,
        left_entries: &[Entry],
        right_entries: &[Entry],
    ) {
        self.supersede_active_verification();
        self.enabled = true;
        self.diff = Some(WorkspaceDiff::compare(
            left_root,
            right_root,
            left_entries.iter().map(workspace_entry),
            right_entries.iter().map(workspace_entry),
        ));
        self.rebuild_plan();
    }

    pub fn apply_verification(
        &mut self,
        verification: &SyncVerificationSnapshot,
        current_left_root: &Location,
        current_right_root: &Location,
    ) -> bool {
        if !self.enabled
            || verification.left_root != *current_left_root
            || verification.right_root != *current_right_root
        {
            if let Some(current) = &mut self.verification
                && current.id == verification.id
                && !current.status.is_terminal()
            {
                current.status = SyncVerificationStatus::Superseded;
            }
            return false;
        }

        match &self.verification {
            None => {
                if !matches!(verification.status, SyncVerificationStatus::Pending) {
                    return false;
                }
            }
            Some(current) if verification.id < current.id => return false,
            Some(current) if verification.id > current.id => {
                if !matches!(verification.status, SyncVerificationStatus::Pending) {
                    return false;
                }
            }
            Some(current) => {
                if current.plan_id != verification.plan_id
                    || current.left_root != verification.left_root
                    || current.right_root != verification.right_root
                    || !current.status.can_transition_to(&verification.status)
                {
                    return false;
                }
            }
        }

        if let SyncVerificationStatus::Finished(result) = &verification.status {
            if result.plan_id != verification.plan_id
                || result.left_root != verification.left_root
                || result.right_root != verification.right_root
            {
                return false;
            }
            self.diff = Some(result.diff.clone());
            self.rebuild_plan();
        }
        self.verification = Some(verification.clone());
        true
    }

    fn supersede_active_verification(&mut self) {
        let Some(mut verification) = self.verification.take() else {
            return;
        };
        if verification.status.is_terminal() {
            return;
        }
        verification.status = SyncVerificationStatus::Superseded;
        self.verification = Some(verification);
    }

    pub fn rebuild_plan(&mut self) {
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

    pub fn supersede_launch_presentation(&mut self) {
        if !matches!(self.ux, WorkspaceSyncUxState::Launching { .. }) {
            return;
        }
        self.frozen_plan = None;
        self.ux = if self.preview_open && self.plan.is_some() {
            WorkspaceSyncUxState::Preview { plan_id: None }
        } else {
            WorkspaceSyncUxState::Idle
        };
    }

    pub fn show_verification_diff(&mut self, job_id: impl Into<String>) {
        self.ux = WorkspaceSyncUxState::VerificationDiff {
            job_id: job_id.into(),
        };
    }

    pub fn return_from_verification_diff(&mut self) -> bool {
        let WorkspaceSyncUxState::VerificationDiff { job_id } = &self.ux else {
            return false;
        };
        self.ux = WorkspaceSyncUxState::Finished {
            job_id: job_id.clone(),
        };
        true
    }

    pub fn has_current_preview(
        &self,
        current_left_root: &Location,
        current_right_root: &Location,
    ) -> bool {
        self.enabled
            && self.diff.as_ref().is_some_and(|diff| {
                diff.left_root == *current_left_root && diff.right_root == *current_right_root
            })
            && self.plan.as_ref().is_some_and(|plan| {
                plan.left_root == *current_left_root && plan.right_root == *current_right_root
            })
    }

    pub fn sync_from_job(&mut self, job: &Job) {
        let Some(context) = &job.sync_context else {
            return;
        };
        let same_workspace = self.diff.as_ref().is_some_and(|diff| {
            diff.left_root == context.left_root && diff.right_root == context.right_root
        });
        let current_job = self.ux.job_id().is_some_and(|id| id == job.id);
        if !same_workspace && !current_job {
            return;
        }

        self.ux = match job.status {
            JobStatus::Pending => WorkspaceSyncUxState::Queued {
                job_id: job.id.clone(),
            },
            JobStatus::Running | JobStatus::PausePending => WorkspaceSyncUxState::Running {
                job_id: job.id.clone(),
            },
            JobStatus::Cancelling => WorkspaceSyncUxState::Cancelling {
                job_id: job.id.clone(),
            },
            JobStatus::Paused => WorkspaceSyncUxState::Running {
                job_id: job.id.clone(),
            },
            JobStatus::RetryWaiting => WorkspaceSyncUxState::Running {
                job_id: job.id.clone(),
            },
            JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled => {
                let needs_verification = match &job.result {
                    Some(JobResult::WorkspaceSync(outcome)) => outcome.needs_verification(),
                    _ => false,
                };
                if needs_verification
                    && job
                        .verification
                        .as_ref()
                        .is_none_or(|item| !item.status.is_terminal())
                {
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

    /// A verification result can belong to a Job that is still shown while the
    /// panes already point at a newer workspace. In that case the old result
    /// must not replace the new diff, but a terminal result must still settle
    /// the old Job presentation instead of leaving it stuck in Verifying.
    pub fn settle_rejected_verification(
        &mut self,
        job_id: &str,
        verification: &SyncVerificationSnapshot,
    ) {
        if verification.status.is_terminal()
            && self.ux.job_id().is_some_and(|current| current == job_id)
        {
            self.ux = WorkspaceSyncUxState::Finished {
                job_id: job_id.to_string(),
            };
        }
    }

    pub fn reverse_direction(&mut self) {
        self.policy.direction = match self.policy.direction {
            SyncDirection::LeftToRight => SyncDirection::RightToLeft,
            SyncDirection::RightToLeft => SyncDirection::LeftToRight,
        };
        self.rebuild_plan();
    }

    pub fn toggle_mode(&mut self) {
        self.policy.mode = match self.policy.mode {
            SyncMode::Update => SyncMode::Mirror,
            SyncMode::Mirror => SyncMode::Update,
        };
        self.rebuild_plan();
    }

    pub fn direction_label(&self) -> &'static str {
        match self.policy.direction {
            SyncDirection::LeftToRight => "LEFT → RIGHT",
            SyncDirection::RightToLeft => "RIGHT → LEFT",
        }
    }

    pub fn mode_label(&self) -> &'static str {
        match self.policy.mode {
            SyncMode::Update => "UPDATE",
            SyncMode::Mirror => "MIRROR",
        }
    }

    pub fn summary(&self) -> String {
        let Some(diff) = &self.diff else {
            return "workspace: waiting for comparison".into();
        };
        let mut left = 0usize;
        let mut right = 0usize;
        let mut different = 0usize;
        for entry in &diff.entries {
            match entry.state {
                DiffState::OnlyLeft | DiffState::LeftNewer => left += 1,
                DiffState::OnlyRight | DiffState::RightNewer => right += 1,
                DiffState::Different => different += 1,
                DiffState::SameFingerprint => {}
            }
        }

        format!(
            "workspace: {} | {} | ←{} →{} ≠{}",
            self.direction_label(),
            self.mode_label(),
            right,
            left,
            different
        )
    }
}

fn workspace_entry(entry: &Entry) -> WorkspaceEntry {
    WorkspaceEntry {
        relative_path: entry.name.clone(),
        fingerprint: WorkspaceFingerprint {
            kind: entry.kind,
            size: entry.size,
            // Entry currently does not expose these fields. The async metadata
            // enrichment effect introduced next upgrades this fingerprint.
            modified_unix_ms: None,
            content_hash: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::EntryKind;
    use std::path::PathBuf;

    fn file(name: &str, size: u64) -> Entry {
        Entry {
            name: name.into(),
            kind: EntryKind::File,
            size: Some(size),
            modified_unix_ms: None,
        }
    }

    #[test]
    fn visible_refresh_produces_plan_without_executing_anything() {
        let mut state = RemoteWorkspaceState::default();
        state.refresh_visible(
            Location::Local(PathBuf::from("/left")),
            Location::Local(PathBuf::from("/right")),
            &[file("local.txt", 10)],
            &[file("remote.txt", 20)],
        );

        assert!(state.enabled);
        assert!(state.diff.is_some());
        assert!(state.plan.is_some());
        assert_eq!(
            state.policy.conflicts,
            crate::workspace_sync::ConflictPolicy::RequireResolution
        );
    }

    #[test]
    fn rejected_terminal_verification_settles_old_job_without_replacing_new_workspace() {
        let mut state = RemoteWorkspaceState {
            enabled: true,
            ux: WorkspaceSyncUxState::Verifying {
                job_id: "sync-old".into(),
            },
            ..RemoteWorkspaceState::default()
        };
        state.refresh_visible(
            Location::Local(PathBuf::from("/new-left")),
            Location::Local(PathBuf::from("/new-right")),
            &[file("new.txt", 1)],
            &[],
        );
        state.ux = WorkspaceSyncUxState::Verifying {
            job_id: "sync-old".into(),
        };
        let new_diff = state.diff.clone().unwrap();
        let current_plan = state.plan.clone().unwrap();
        let plan_id = crate::workspace_sync_execution::SyncPlanValidator::freeze(
            &current_plan,
            &new_diff,
            &crate::vfs::default_registry(),
        )
        .unwrap()
        .id();
        let old_roots = (
            Location::Local(PathBuf::from("/old-left")),
            Location::Local(PathBuf::from("/old-right")),
        );
        let verification = SyncVerificationSnapshot {
            id: crate::workspace_sync_verification::SyncVerificationId(99),
            plan_id,
            left_root: old_roots.0,
            right_root: old_roots.1,
            status: SyncVerificationStatus::Superseded,
        };

        assert!(!state.apply_verification(
            &verification,
            &Location::Local(PathBuf::from("/new-left")),
            &Location::Local(PathBuf::from("/new-right")),
        ));
        state.settle_rejected_verification("sync-old", &verification);

        assert_eq!(state.diff.as_ref(), Some(&new_diff));
        assert!(matches!(
            state.ux,
            WorkspaceSyncUxState::Finished { ref job_id } if job_id == "sync-old"
        ));
    }

    #[test]
    fn policy_change_invalidates_frozen_execution_context() {
        let mut state = RemoteWorkspaceState {
            preview_open: true,
            ..RemoteWorkspaceState::default()
        };
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

    #[test]
    fn navigation_preserves_active_job_session_but_invalidates_confirmation() {
        let mut active = RemoteWorkspaceState {
            enabled: true,
            preview_open: true,
            ux: WorkspaceSyncUxState::Running {
                job_id: "sync-1".into(),
            },
            ..RemoteWorkspaceState::default()
        };
        active.disable();
        assert!(!active.enabled);
        assert!(!active.preview_open);
        assert!(matches!(
            active.ux,
            WorkspaceSyncUxState::Running { ref job_id } if job_id == "sync-1"
        ));

        let mut confirmation = RemoteWorkspaceState {
            enabled: true,
            preview_open: true,
            ..RemoteWorkspaceState::default()
        };
        confirmation.refresh_visible(
            Location::Local(PathBuf::from("/left")),
            Location::Local(PathBuf::from("/right")),
            &[],
            &[file("old.txt", 1)],
        );
        confirmation.toggle_mode();
        let plan = confirmation.plan.clone().unwrap();
        let diff = confirmation.diff.clone().unwrap();
        let frozen = crate::workspace_sync_execution::SyncPlanValidator::freeze(
            &plan,
            &diff,
            &crate::vfs::default_registry(),
        )
        .unwrap();
        confirmation.set_frozen_plan(frozen);
        assert!(matches!(
            confirmation.ux,
            WorkspaceSyncUxState::ConfirmationRequired { .. }
        ));
        confirmation.disable();
        assert!(confirmation.frozen_plan.is_none());
        assert!(matches!(confirmation.ux, WorkspaceSyncUxState::Idle));
    }

    #[test]
    fn mirror_is_never_the_default() {
        assert_eq!(
            RemoteWorkspaceState::default().policy.mode,
            SyncMode::Update
        );
    }

    #[test]
    fn finished_job_after_navigation_has_no_current_preview_to_return_to() {
        let left = Location::Local(PathBuf::from("/left"));
        let right = Location::Local(PathBuf::from("/right"));
        let mut state = RemoteWorkspaceState {
            preview_open: true,
            ..RemoteWorkspaceState::default()
        };
        state.refresh_visible(left.clone(), right.clone(), &[file("a.txt", 1)], &[]);
        state.ux = WorkspaceSyncUxState::Finished {
            job_id: "sync-old".into(),
        };

        assert!(state.has_current_preview(&left, &right));
        state.disable();

        assert!(matches!(
            state.ux,
            WorkspaceSyncUxState::Finished { ref job_id } if job_id == "sync-old"
        ));
        assert!(!state.has_current_preview(&left, &right));
        assert!(state.diff.is_none());
        assert!(state.plan.is_none());
    }

    #[test]
    fn verification_diff_returns_to_the_same_job_result() {
        let mut state = RemoteWorkspaceState {
            ux: WorkspaceSyncUxState::Finished {
                job_id: "sync-old".into(),
            },
            ..RemoteWorkspaceState::default()
        };

        state.show_verification_diff("sync-old");
        assert!(matches!(
            state.ux,
            WorkspaceSyncUxState::VerificationDiff { ref job_id } if job_id == "sync-old"
        ));
        assert!(state.return_from_verification_diff());
        assert!(matches!(
            state.ux,
            WorkspaceSyncUxState::Finished { ref job_id } if job_id == "sync-old"
        ));
    }
}
