pub mod executor;
pub mod probe;

use crate::vfs::{Capability, CapabilitySet, Location, ProviderId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferMethod {
    Native,
    Rsync,
    Sftp,
    Scp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferIntent {
    Copy,
    Move,
    Synchronize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferPlan {
    pub source: Location,
    pub destination: Location,
    pub intent: TransferIntent,
    pub method: TransferMethod,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutorAvailability {
    pub native: bool,
    pub rsync: bool,
    pub sftp: bool,
}

impl ExecutorAvailability {
    pub const NONE: Self = Self {
        native: false,
        rsync: false,
        sftp: false,
    };

    pub const fn local() -> Self {
        Self {
            native: true,
            rsync: false,
            sftp: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferRequest {
    pub source: Location,
    pub destination: Location,
    pub source_provider: ProviderId,
    pub destination_provider: ProviderId,
    pub source_capabilities: CapabilitySet,
    pub destination_capabilities: CapabilitySet,
    pub intent: TransferIntent,
    pub executors: ExecutorAvailability,
    pub delete_extraneous: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferPlanError {
    PreviewRequired,
    Unsupported {
        source: ProviderId,
        destination: ProviderId,
        intent: TransferIntent,
    },
}

impl std::fmt::Display for TransferPlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PreviewRequired => write!(
                f,
                "destructive synchronization requires preview and explicit confirmation"
            ),
            Self::Unsupported {
                source,
                destination,
                intent,
            } => write!(
                f,
                "no safe transfer strategy for {intent:?}: {source:?} -> {destination:?}"
            ),
        }
    }
}

impl std::error::Error for TransferPlanError {}

#[derive(Debug, Default)]
pub struct TransferPlanner;

impl TransferPlanner {
    pub fn plan(request: TransferRequest) -> Result<TransferPlan, TransferPlanError> {
        if request.intent == TransferIntent::Synchronize && request.delete_extraneous {
            return Err(TransferPlanError::PreviewRequired);
        }

        let method = Self::choose_method(&request)?;
        Ok(TransferPlan {
            source: request.source,
            destination: request.destination,
            intent: request.intent,
            method,
        })
    }

    fn choose_method(request: &TransferRequest) -> Result<TransferMethod, TransferPlanError> {
        if request.source_provider == ProviderId::Local
            && request.destination_provider == ProviderId::Local
            && request.executors.native
            && Self::native_operation_supported(request)
        {
            return Ok(TransferMethod::Native);
        }

        if Self::is_local_remote_pair(request) {
            return match request.intent {
                TransferIntent::Copy if request.executors.rsync => Ok(TransferMethod::Rsync),
                TransferIntent::Copy if request.executors.sftp => Ok(TransferMethod::Sftp),
                // Cross-backend move must become copy -> verify -> delete-source.
                // Until that transaction exists, refusing the plan is safer than
                // silently degrading Move into Copy.
                TransferIntent::Move => Err(Self::unsupported(request)),
                TransferIntent::Synchronize if request.executors.rsync => Ok(TransferMethod::Rsync),
                _ => Err(Self::unsupported(request)),
            };
        }

        if request.source_provider == request.destination_provider
            && request.source_provider != ProviderId::Sftp
            && request.executors.native
            && Self::native_operation_supported(request)
        {
            return Ok(TransferMethod::Native);
        }

        Err(Self::unsupported(request))
    }

    fn is_local_remote_pair(request: &TransferRequest) -> bool {
        matches!(
            (request.source_provider, request.destination_provider),
            (ProviderId::Local, ProviderId::Sftp) | (ProviderId::Sftp, ProviderId::Local)
        )
    }

    fn native_operation_supported(request: &TransferRequest) -> bool {
        match request.intent {
            TransferIntent::Copy => request.source_capabilities.supports(Capability::Copy),
            TransferIntent::Move => request.source_capabilities.supports(Capability::Move),
            TransferIntent::Synchronize => false,
        }
    }

    fn unsupported(request: &TransferRequest) -> TransferPlanError {
        TransferPlanError::Unsupported {
            source: request.source_provider,
            destination: request.destination_provider,
            intent: request.intent,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::capabilities::{LOCAL_CAPABILITIES, SFTP_CAPABILITIES};
    use std::path::PathBuf;

    fn local(path: &str) -> Location {
        Location::Local(PathBuf::from(path))
    }

    fn sftp(host: &str, path: &str) -> Location {
        Location::Sftp {
            host: host.into(),
            path: path.into(),
        }
    }

    #[test]
    fn local_copy_prefers_native() {
        let plan = TransferPlanner::plan(TransferRequest {
            source: local("/src"),
            destination: local("/dst"),
            source_provider: ProviderId::Local,
            destination_provider: ProviderId::Local,
            source_capabilities: LOCAL_CAPABILITIES,
            destination_capabilities: LOCAL_CAPABILITIES,
            intent: TransferIntent::Copy,
            executors: ExecutorAvailability::local(),
            delete_extraneous: false,
        })
        .unwrap();
        assert_eq!(plan.method, TransferMethod::Native);
    }

    #[test]
    fn remote_copy_prefers_rsync_then_sftp() {
        let base = TransferRequest {
            source: local("/src"),
            destination: sftp("prod", "/dst"),
            source_provider: ProviderId::Local,
            destination_provider: ProviderId::Sftp,
            source_capabilities: LOCAL_CAPABILITIES,
            destination_capabilities: SFTP_CAPABILITIES,
            intent: TransferIntent::Copy,
            executors: ExecutorAvailability {
                native: true,
                rsync: true,
                sftp: true,
            },
            delete_extraneous: false,
        };

        assert_eq!(
            TransferPlanner::plan(base.clone()).unwrap().method,
            TransferMethod::Rsync
        );

        let mut fallback = base;
        fallback.executors.rsync = false;
        assert_eq!(
            TransferPlanner::plan(fallback).unwrap().method,
            TransferMethod::Sftp
        );
    }

    #[test]
    fn remote_move_is_rejected_until_transactional_cleanup_exists() {
        let request = TransferRequest {
            source: local("/src"),
            destination: sftp("prod", "/dst"),
            source_provider: ProviderId::Local,
            destination_provider: ProviderId::Sftp,
            source_capabilities: LOCAL_CAPABILITIES,
            destination_capabilities: SFTP_CAPABILITIES,
            intent: TransferIntent::Move,
            executors: ExecutorAvailability {
                native: false,
                rsync: true,
                sftp: true,
            },
            delete_extraneous: false,
        };

        assert!(matches!(
            TransferPlanner::plan(request),
            Err(TransferPlanError::Unsupported {
                source: ProviderId::Local,
                destination: ProviderId::Sftp,
                intent: TransferIntent::Move
            })
        ));
    }

    #[test]
    fn remote_to_remote_is_not_routed_through_rsync() {
        let error = TransferPlanner::plan(TransferRequest {
            source: sftp("prod-a", "/src"),
            destination: sftp("prod-b", "/dst"),
            source_provider: ProviderId::Sftp,
            destination_provider: ProviderId::Sftp,
            source_capabilities: SFTP_CAPABILITIES,
            destination_capabilities: SFTP_CAPABILITIES,
            intent: TransferIntent::Copy,
            executors: ExecutorAvailability {
                native: false,
                rsync: true,
                sftp: true,
            },
            delete_extraneous: false,
        })
        .unwrap_err();

        assert!(matches!(
            error,
            TransferPlanError::Unsupported {
                source: ProviderId::Sftp,
                destination: ProviderId::Sftp,
                intent: TransferIntent::Copy
            }
        ));
    }

    #[test]
    fn destructive_sync_requires_preview() {
        let error = TransferPlanner::plan(TransferRequest {
            source: local("/src"),
            destination: sftp("prod", "/dst"),
            source_provider: ProviderId::Local,
            destination_provider: ProviderId::Sftp,
            source_capabilities: LOCAL_CAPABILITIES,
            destination_capabilities: SFTP_CAPABILITIES,
            intent: TransferIntent::Synchronize,
            executors: ExecutorAvailability {
                native: false,
                rsync: true,
                sftp: true,
            },
            delete_extraneous: true,
        })
        .unwrap_err();
        assert_eq!(error, TransferPlanError::PreviewRequired);
    }

    #[test]
    fn planner_refuses_missing_executor() {
        let error = TransferPlanner::plan(TransferRequest {
            source: local("/src"),
            destination: sftp("prod", "/dst"),
            source_provider: ProviderId::Local,
            destination_provider: ProviderId::Sftp,
            source_capabilities: LOCAL_CAPABILITIES,
            destination_capabilities: SFTP_CAPABILITIES,
            intent: TransferIntent::Copy,
            executors: ExecutorAvailability::NONE,
            delete_extraneous: false,
        })
        .unwrap_err();

        assert!(matches!(
            error,
            TransferPlanError::Unsupported {
                source: ProviderId::Local,
                destination: ProviderId::Sftp,
                intent: TransferIntent::Copy
            }
        ));
    }
}
