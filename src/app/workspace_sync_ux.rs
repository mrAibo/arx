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
            | Self::Finished { job_id } => Some(job_id),
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
