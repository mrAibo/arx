use crate::workspace_sync_execution::{PlanDigest, SyncPlanId};

/// Presentation-only state for one Remote Workspace sync flow.
///
/// Job lifecycle, execution results and verification remain owned by their
/// existing runtime layers. This enum only tells the overlay which stage to
/// present and which already-owned object to read.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum WorkspaceSyncUxState {
    #[default]
    Idle,
    Scanning,
    Preview {
        plan_id: Option<SyncPlanId>,
    },
    ConfirmationRequired {
        plan_id: SyncPlanId,
        digest: PlanDigest,
        destructive_operations: usize,
    },
    Launching {
        plan_id: SyncPlanId,
    },
    Queued {
        job_id: String,
    },
    Running {
        job_id: String,
    },
    Cancelling {
        job_id: String,
    },
    Verifying {
        job_id: String,
    },
    Finished {
        job_id: String,
    },
    VerificationDiff {
        job_id: String,
    },
    Blocked {
        message: String,
    },
}

impl WorkspaceSyncUxState {
    pub fn job_id(&self) -> Option<&str> {
        match self {
            Self::Queued { job_id }
            | Self::Running { job_id }
            | Self::Cancelling { job_id }
            | Self::Verifying { job_id }
            | Self::Finished { job_id }
            | Self::VerificationDiff { job_id } => Some(job_id),
            _ => None,
        }
    }

    pub fn is_preview_editable(&self) -> bool {
        matches!(
            self,
            Self::Scanning | Self::Preview { .. } | Self::Blocked { .. }
        )
    }

    pub fn is_job_flow(&self) -> bool {
        self.job_id().is_some()
    }

    /// Once a Job exists, actions that rebuild/disable the workspace
    /// comparison are presentation-unsafe. `Launching` is intentionally
    /// supersedable: a newer workspace action invalidates the old launch before
    /// it can queue a Job.
    pub fn is_locked_flow(&self) -> bool {
        matches!(
            self,
            Self::Queued { .. }
                | Self::Running { .. }
                | Self::Cancelling { .. }
                | Self::Verifying { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_jobs_lock_preview_mutation_but_launching_can_be_superseded() {
        let launching = WorkspaceSyncUxState::Launching {
            plan_id: crate::workspace_sync_execution::SyncPlanValidator::freeze(
                &crate::workspace_sync::WorkspaceSyncPlan::build(
                    &crate::workspace_sync::WorkspaceDiff::compare(
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
                    ),
                    crate::workspace_sync::SyncPolicy::default(),
                ),
                &crate::workspace_sync::WorkspaceDiff::compare(
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
                ),
                &crate::vfs::default_registry(),
            )
            .unwrap()
            .id(),
        };
        assert!(!launching.is_locked_flow());
        assert!(
            WorkspaceSyncUxState::Running {
                job_id: "sync-1".into()
            }
            .is_locked_flow()
        );
        assert!(!WorkspaceSyncUxState::Preview { plan_id: None }.is_locked_flow());
        assert!(
            !WorkspaceSyncUxState::Finished {
                job_id: "sync-1".into()
            }
            .is_locked_flow()
        );
    }

    #[test]
    fn only_runtime_backed_states_expose_a_job_id() {
        assert_eq!(
            WorkspaceSyncUxState::Running {
                job_id: "sync-1".into()
            }
            .job_id(),
            Some("sync-1")
        );
        assert!(
            WorkspaceSyncUxState::Preview { plan_id: None }
                .job_id()
                .is_none()
        );
    }

    #[test]
    fn verification_diff_keeps_the_job_identity() {
        let state = WorkspaceSyncUxState::VerificationDiff {
            job_id: "sync-old".into(),
        };
        assert_eq!(state.job_id(), Some("sync-old"));
        assert!(state.is_job_flow());
        assert!(!state.is_locked_flow());
    }
}
