pub mod executor;
pub mod probe;
#[allow(dead_code)]
pub mod sftp_copy;

use crate::vfs::{
    Capability, CapabilitySet, Location, ProviderId, S3ObjectRef, validate_child_name,
};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferMethod {
    Native,
    Rsync,
    Sftp,
    Scp,
    /// S3 data-movement executor. Reserved by S3-31R; the planner does not
    /// select it yet (returns Unsupported for any S3 pair). Constructible so
    /// later S3-31/32/33 cards can populate it without touching this seam.
    // ponytail: variant only; no executor wired in S3-31R
    S3,
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
    /// Frozen S3 payload for this plan. Plumbing only in S3-31R; populated by
    /// later S3-31/32/33 planner cards. None for non-S3 transfers.
    pub s3_spec: Option<S3TransferSpec>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutorAvailability {
    pub native: bool,
    pub rsync: bool,
    pub sftp: bool,
    /// S3 data-movement executor availability. Reserved by S3-31R; not enabled
    /// by the planner yet.
    // ponytail: field only; planner ignores until the S3 executor lands
    pub s3: bool,
}

impl ExecutorAvailability {
    pub const NONE: Self = Self {
        native: false,
        rsync: false,
        sftp: false,
        s3: false,
    };

    pub const fn local() -> Self {
        Self {
            native: true,
            rsync: false,
            sftp: false,
            s3: false,
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
    /// Frozen S3 payload for this request. Plumbing only in S3-31R; populated by
    /// later cards. None for non-S3 transfers.
    pub s3_spec: Option<S3TransferSpec>,
}

/// Frozen, transferred S3 identity/payload. The `S3ObjectRef` is the sole
/// authority for bucket/key — never reconstructed from a `Location` + name.
// ponytail: identity boundary only; no AWS behavior here (S3-34/37 cores)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum S3TransferSpec {
    UploadOne {
        local_source: PathBuf,
        destination: S3ObjectRef,
    },
    DownloadOne {
        source: S3ObjectRef,
        local_destination: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferPlanError {
    PreviewRequired,
    Unsupported {
        source: ProviderId,
        destination: ProviderId,
        intent: TransferIntent,
    },
    /// A name could not be represented as a single safe local child.
    InvalidLocalName(String),
    /// S3 basic transfer supports exactly one object per operation.
    TooManyObjects(String),
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
            Self::InvalidLocalName(msg) => write!(f, "{msg}"),
            Self::TooManyObjects(msg) => write!(f, "{msg}"),
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
            s3_spec: request.s3_spec,
        })
    }

    fn choose_method(request: &TransferRequest) -> Result<TransferMethod, TransferPlanError> {
        // S3 data movement is not enabled in S3-31R: refuse any plan that
        // touches an S3 provider until the executor + later planner cards land.
        if request.source_provider == ProviderId::S3
            || request.destination_provider == ProviderId::S3
        {
            return Err(Self::unsupported(request));
        }

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

// ── Frozen S3 identity helpers (pure, no AWS) ──

/// Validate `presentation_name` is exactly one safe local child name and return
/// it. Fails closed with a factual error for unsafe names; never sanitizes.
pub fn s3_download_local_name(
    _object: &S3ObjectRef,
    presentation_name: &str,
) -> Result<String, TransferPlanError> {
    validate_child_name(presentation_name).map_err(|_| {
        TransferPlanError::InvalidLocalName(
            "object cannot be represented as a single local filename".to_string(),
        )
    })?;
    Ok(presentation_name.to_string())
}

/// Build a frozen S3 destination `S3ObjectRef` for an upload.
///
/// The key is `filename` when `nav_prefix` is empty, else `nav_prefix + "/" +
/// filename` with EXACTLY one `/` appended unconditionally (no normalization of
/// repeated slashes, `.`, `..`, or unicode). `filename` must be a safe single
/// local child (fail closed otherwise). The ref is constructed from the
/// authoritative `target`/`bucket`, never from a `Location` + name.
pub fn s3_upload_destination_ref(
    target: &str,
    bucket: &str,
    nav_prefix: &str,
    filename: &str,
) -> Result<S3ObjectRef, TransferPlanError> {
    validate_child_name(filename).map_err(|_| {
        TransferPlanError::InvalidLocalName(
            "object cannot be represented as a single local filename".to_string(),
        )
    })?;
    let key = if nav_prefix.is_empty() {
        filename.to_string()
    } else {
        format!("{}/{}", nav_prefix, filename)
    };
    Ok(S3ObjectRef {
        target: target.to_string(),
        bucket: bucket.to_string(),
        key,
    })
}

/// Assert exactly one object for an S3 basic transfer, returning it.
/// Fails closed otherwise — S3 basic transfer moves one file per operation.
pub fn s3_spec_for_objects(objects: &[S3ObjectRef]) -> Result<&S3ObjectRef, TransferPlanError> {
    match objects {
        [only] => Ok(only),
        _ => Err(TransferPlanError::TooManyObjects(
            "S3 basic transfer currently supports one file per operation".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::capabilities::{LOCAL_CAPABILITIES, S3_CAPABILITIES, SFTP_CAPABILITIES};
    use std::path::PathBuf;

    fn local(path: &str) -> Location {
        Location::Local(PathBuf::from(path))
    }

    fn s3(target: &str, bucket: Option<&str>) -> Location {
        Location::S3 {
            target: target.to_string(),
            bucket: bucket.map(|b| b.to_string()),
            prefix: String::new(),
        }
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
            s3_spec: None,
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
                s3: false,
            },
            delete_extraneous: false,
            s3_spec: None,
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
                s3: false,
            },
            delete_extraneous: false,
            s3_spec: None,
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
                s3: false,
            },
            delete_extraneous: false,
            s3_spec: None,
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
                s3: false,
            },
            delete_extraneous: true,
            s3_spec: None,
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
            s3_spec: None,
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

    // ── S3-31R: S3 seam ──

    #[test]
    fn planner_refuses_s3_pair_in_s3_31r() {
        let error = TransferPlanner::plan(TransferRequest {
            source: local("/src"),
            destination: s3("tgt", Some("bk")),
            source_provider: ProviderId::Local,
            destination_provider: ProviderId::S3,
            source_capabilities: LOCAL_CAPABILITIES,
            destination_capabilities: S3_CAPABILITIES,
            intent: TransferIntent::Copy,
            executors: ExecutorAvailability {
                native: true,
                rsync: false,
                sftp: false,
                s3: true,
            },
            delete_extraneous: false,
            s3_spec: None,
        })
        .unwrap_err();
        assert!(matches!(
            error,
            TransferPlanError::Unsupported {
                source: ProviderId::Local,
                destination: ProviderId::S3,
                intent: TransferIntent::Copy
            }
        ));
        // The S3 executor variant is constructible but never selected here.
        let _ = TransferMethod::S3;
    }

    #[test]
    fn s3_upload_destination_ref_builds_key() {
        let mk = |prefix: &str| s3_upload_destination_ref("tgt", "bk", prefix, "a.txt").unwrap();
        assert_eq!(mk("").key, "a.txt");
        assert_eq!(mk("foo").key, "foo/a.txt");
        assert_eq!(mk("foo/").key, "foo//a.txt");
        assert_eq!(mk("foo//").key, "foo///a.txt");
        // Authoritative target/bucket preserved verbatim.
        let r = mk("");
        assert_eq!(r.target, "tgt");
        assert_eq!(r.bucket, "bk");
    }

    #[test]
    fn s3_upload_destination_ref_rejects_unsafe_filename() {
        for bad in ["", ".", "..", "/", "../", "a/b"] {
            assert!(
                s3_upload_destination_ref("tgt", "bk", "p", bad).is_err(),
                "{bad} must be rejected"
            );
        }
    }

    #[test]
    fn s3_download_local_name_validation() {
        let obj = S3ObjectRef {
            target: "t".into(),
            bucket: "b".into(),
            key: "k".into(),
        };
        assert_eq!(s3_download_local_name(&obj, "a.txt").unwrap(), "a.txt");
        for bad in ["../", "/", "", ".", ".."] {
            assert!(
                s3_download_local_name(&obj, bad).is_err(),
                "{bad} must be rejected"
            );
        }
    }

    #[test]
    fn s3_spec_round_trips_into_request_and_plan() {
        let spec = S3TransferSpec::DownloadOne {
            source: S3ObjectRef {
                target: "t".into(),
                bucket: "b".into(),
                key: "k".into(),
            },
            local_destination: PathBuf::from("/d/a.txt"),
        };
        let build = |s3_spec| TransferRequest {
            source: local("/src"),
            destination: local("/dst"),
            source_provider: ProviderId::Local,
            destination_provider: ProviderId::Local,
            source_capabilities: LOCAL_CAPABILITIES,
            destination_capabilities: LOCAL_CAPABILITIES,
            intent: TransferIntent::Copy,
            executors: ExecutorAvailability::local(),
            delete_extraneous: false,
            s3_spec,
        };
        // Some: copied into the plan by the planner.
        let plan = TransferPlanner::plan(build(Some(spec.clone()))).unwrap();
        assert_eq!(plan.s3_spec, Some(spec));
        // None: preserved as None.
        let plan = TransferPlanner::plan(build(None)).unwrap();
        assert_eq!(plan.s3_spec, None);
    }

    #[test]
    fn s3_spec_for_objects_enforces_single_object() {
        let one = S3ObjectRef {
            target: "t".into(),
            bucket: "b".into(),
            key: "k".into(),
        };
        assert!(s3_spec_for_objects(std::slice::from_ref(&one)).is_ok());
        assert!(s3_spec_for_objects(&[]).is_err());
        assert!(s3_spec_for_objects(&[one.clone(), one.clone()]).is_err());
    }
}
