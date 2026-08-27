pub mod executor;
pub mod integrity;
pub mod probe;
#[allow(dead_code)]
pub mod sftp_copy;
// ponytail: S3 cores land in S3-34/37; seam only until then
pub mod s3_download;
pub mod s3_multipart;
pub mod s3_upload;
pub mod webdav_batch;
pub mod webdav_move;
pub mod webdav_transfer;

pub use self::executor::TransferOutcome;
pub use self::webdav_transfer::WebDavOverwritePolicy;

use crate::vfs::{
    Capability, CapabilitySet, EntryIdentity, EntryKind, ListedEntry, Location, ProviderId,
    S3ObjectRef, WebDavObjectRef, validate_child_name,
};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferMethod {
    Native,
    Rsync,
    Sftp,
    Scp,
    /// S3 data-movement executor. Selected only for a Local<->S3 `Copy` with an
    /// available S3 executor and a frozen `S3TransferSpec`; `execute_transfer`
    /// still refuses it until the executor card lands.
    // ponytail: planner selection only (S3-31/32/33); executor stays fail-closed
    S3,
    /// WebDAV data-movement executor. Selected only for a Local<->WebDAV `Copy`
    /// with an available WebDAV executor and a frozen `WebDavTransferSpec`.
    /// GET/PUT address the exact raw href from the listing identity — never a
    /// reconstructed path. Executor is live (B1/B2).
    WebDav,
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
    /// Frozen S3 payload for this plan, carried verbatim from the request.
    /// None for non-S3 transfers.
    pub s3_spec: Option<S3TransferSpec>,
    /// Frozen WebDAV payload for this plan, carried verbatim from the request.
    /// None for non-WebDAV transfers.
    pub webdav_spec: Option<WebDavTransferSpec>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutorAvailability {
    pub native: bool,
    pub rsync: bool,
    pub sftp: bool,
    /// S3 data-movement executor availability. Required (together with a frozen
    /// `S3TransferSpec`) for the planner to select `TransferMethod::S3`.
    pub s3: bool,
    /// WebDAV data-movement executor availability. Required (together with a
    /// frozen `WebDavTransferSpec`) for the planner to select
    /// `TransferMethod::WebDav`.
    pub webdav: bool,
}

impl ExecutorAvailability {
    pub const NONE: Self = Self {
        native: false,
        rsync: false,
        sftp: false,
        s3: false,
        webdav: false,
    };

    pub const fn local() -> Self {
        Self {
            native: true,
            rsync: false,
            sftp: false,
            s3: false,
            webdav: false,
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
    /// Frozen S3 payload for this request, built by the caller (TUI/planner).
    /// The planner never reconstructs it. None for non-S3 transfers.
    pub s3_spec: Option<S3TransferSpec>,
    /// Frozen WebDAV payload for this request, built by the caller.
    /// The planner never reconstructs it. None for non-WebDAV transfers.
    pub webdav_spec: Option<WebDavTransferSpec>,
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
pub struct WebDavWriteTarget {
    pub target: String,
    /// Decoded logical path inside the configured target. The provider owns
    /// wire percent-encoding at the HTTP boundary.
    pub logical_path: String,
}

/// Frozen WebDAV transfer payload. Existing remote resources use exact native
/// href identity; not-yet-existing upload destinations use `WebDavWriteTarget`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebDavTransferSpec {
    UploadOne {
        local_source: PathBuf,
        destination: WebDavWriteTarget,
    },
    UploadTree {
        local_source: PathBuf,
        destination_root: WebDavWriteTarget,
    },
    DownloadOne {
        source: WebDavObjectRef,
        local_destination: PathBuf,
    },
    /// One exact WebDAV collection recursively materialized as one new Local
    /// root. Traversal stays in the existing WebDAV executor authority.
    DownloadTree {
        source: crate::vfs::WebDavCollectionRef,
        local_destination: PathBuf,
    },
    /// One exact WebDAV collection copied recursively into one NEW WebDAV
    /// destination root. Source and destination target ids are independently
    /// frozen; existing source identity is never reconstructed from display.
    CopyTree {
        source: crate::vfs::WebDavCollectionRef,
        destination_root: WebDavWriteTarget,
    },
    /// One exact WebDAV collection moved through the explicit
    /// copy -> verify -> delete-frozen-source transaction. This is never a
    /// server-side recursive MOVE alias.
    MoveTree {
        source: crate::vfs::WebDavCollectionRef,
        destination_root: WebDavWriteTarget,
    },
    /// Two or more frozen sibling roots executed sequentially through the
    /// existing WebDAV executor and one concrete provider authority.
    Batch {
        target: String,
        items: Vec<WebDavTransferSpec>,
    },
}

impl WebDavTransferSpec {
    /// The exact target id this spec is bound to (authority check on execution).
    pub fn target(&self) -> &str {
        match self {
            Self::UploadOne { destination, .. } => &destination.target,
            Self::UploadTree {
                destination_root, ..
            } => &destination_root.target,
            Self::DownloadOne { source, .. } => &source.target,
            Self::DownloadTree { source, .. } => &source.target,
            Self::CopyTree { source, .. } | Self::MoveTree { source, .. } => &source.target,
            Self::Batch { target, .. } => target,
        }
    }
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
            webdav_spec: request.webdav_spec,
        })
    }

    fn choose_method(request: &TransferRequest) -> Result<TransferMethod, TransferPlanError> {
        // Every S3-touching plan is decided here and never falls through to the
        // legacy Local/Local, Local<->Sftp or same-provider native branches.
        if request.source_provider == ProviderId::S3
            || request.destination_provider == ProviderId::S3
        {
            if Self::is_s3_pair(request) && request.executors.s3 {
                // The frozen spec is the sole bucket/key authority; without it
                // there is nothing to execute and no name-based fallback.
                return if request.s3_spec.is_some() {
                    Ok(TransferMethod::S3)
                } else {
                    Err(Self::unsupported(request))
                };
            }
            return Err(Self::unsupported(request));
        }

        // Every WebDAV-touching plan is decided here and never falls through.
        if request.source_provider == ProviderId::WebDAV
            || request.destination_provider == ProviderId::WebDAV
        {
            if Self::is_webdav_pair(request) && request.executors.webdav {
                return if request.webdav_spec.is_some() {
                    Ok(TransferMethod::WebDav)
                } else {
                    Err(Self::unsupported(request))
                };
            }
            return Err(Self::unsupported(request));
        }

        if request.source_provider == ProviderId::Local
            && request.destination_provider == ProviderId::Local
            && request.executors.native
            && Self::native_operation_supported(request)
        {
            return Ok(TransferMethod::Native);
        }

        // Remote-to-remote workspace sync is streamed through ARX's existing
        // SFTP executor. It is deliberately never routed through rsync/native or
        // represented as a server-side copy/rename.
        if Self::is_sftp_remote_pair(request) {
            return match request.intent {
                TransferIntent::Copy if request.executors.sftp => Ok(TransferMethod::Sftp),
                _ => Err(Self::unsupported(request)),
            };
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

    fn is_sftp_remote_pair(request: &TransferRequest) -> bool {
        matches!(
            (request.source_provider, request.destination_provider),
            (ProviderId::Sftp, ProviderId::Sftp)
        )
    }

    fn is_local_remote_pair(request: &TransferRequest) -> bool {
        matches!(
            (request.source_provider, request.destination_provider),
            (ProviderId::Local, ProviderId::Sftp) | (ProviderId::Sftp, ProviderId::Local)
        )
    }

    /// Local<->S3 `Copy` is the only S3 shape the planner supports. `Move`
    /// needs a copy/verify/delete transaction and `Synchronize` needs
    /// destructive-diff support; neither exists for S3.
    fn is_s3_pair(request: &TransferRequest) -> bool {
        request.intent == TransferIntent::Copy
            && matches!(
                (request.source_provider, request.destination_provider),
                (ProviderId::Local, ProviderId::S3) | (ProviderId::S3, ProviderId::Local)
            )
    }

    /// WebDAV supports the existing Local<->WebDAV Copy shapes, exact
    /// WebDAV->WebDAV CopyTree, and exact WebDAV->WebDAV MoveTree. Destructive
    /// Move never falls back to CopyTree or generic provider-native Move.
    fn is_webdav_pair(request: &TransferRequest) -> bool {
        match request.intent {
            TransferIntent::Copy => match (request.source_provider, request.destination_provider) {
                (ProviderId::Local, ProviderId::WebDAV)
                | (ProviderId::WebDAV, ProviderId::Local) => true,
                (ProviderId::WebDAV, ProviderId::WebDAV) => matches!(
                    request.webdav_spec,
                    Some(WebDavTransferSpec::CopyTree { .. })
                ),
                _ => false,
            },
            TransferIntent::Move => {
                matches!(
                    (request.source_provider, request.destination_provider),
                    (ProviderId::WebDAV, ProviderId::WebDAV)
                ) && matches!(
                    request.webdav_spec,
                    Some(WebDavTransferSpec::MoveTree { .. })
                )
            }
            TransferIntent::Synchronize => false,
        }
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

/// Validate `presentation_name` is exactly one safe local child name and return
/// it. Fails closed with a factual error for unsafe names; never sanitizes.
// ponytail: WebDAV download uses the listing identity's exact href; local name
/// is purely for the destination file on disk — same validation as S3.
pub fn validate_webdav_local_component(name: &str) -> Result<(), String> {
    validate_child_name(name).map_err(|_| {
        "WebDAV name cannot be represented as one safe local path component".to_string()
    })?;
    if name.contains('\\') || name.contains(':') {
        return Err("WebDAV name cannot be represented as one safe local path component".into());
    }
    Ok(())
}

pub fn webdav_download_local_name(
    _object: &WebDavObjectRef,
    presentation_name: &str,
) -> Result<String, TransferPlanError> {
    validate_webdav_local_component(presentation_name).map_err(|_| {
        TransferPlanError::InvalidLocalName(
            "object cannot be represented as a single local filename".to_string(),
        )
    })?;
    Ok(presentation_name.to_string())
}

/// Freeze a not-yet-existing WebDAV child write address from a configured
/// target, decoded logical parent path, and one validated decoded component.
pub fn webdav_write_child_target(
    target: &str,
    parent_logical_path: &str,
    name: &str,
) -> Result<WebDavWriteTarget, TransferPlanError> {
    validate_webdav_local_component(name).map_err(|_| {
        TransferPlanError::InvalidLocalName(
            "object cannot be represented as a single WebDAV path component".to_string(),
        )
    })?;
    let parent = parent_logical_path.trim_end_matches('/');
    let logical_path = if parent.is_empty() {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    };
    Ok(WebDavWriteTarget {
        target: target.to_string(),
        logical_path,
    })
}

/// Assert exactly one object for a WebDAV basic transfer, returning it.
/// Fails closed otherwise — WebDAV basic transfer moves one file per operation.
pub fn webdav_spec_for_objects(
    objects: &[WebDavObjectRef],
) -> Result<&WebDavObjectRef, TransferPlanError> {
    match objects {
        [only] => Ok(only),
        _ => Err(TransferPlanError::TooManyObjects(
            "WebDAV basic transfer currently supports one file per operation".to_string(),
        )),
    }
}

/// Resolve exactly one source from the ACTIVE pane for a single-object WebDAV
/// copy. Selection wins over cursor focus and is resolved against CURRENT real
/// listed rows. Stale or ambiguous selection fails closed; it never falls back
/// to cursor focus.
fn resolve_webdav_copy_source<'a>(
    selected_names: &[String],
    focused_source: Option<&'a ListedEntry>,
    current_active_listed: &[&'a ListedEntry],
) -> Result<&'a ListedEntry, String> {
    match selected_names {
        [] => focused_source.ok_or_else(|| "Focus a real file to copy".to_string()),
        [selected] => {
            let mut matches = current_active_listed
                .iter()
                .copied()
                .filter(|listed| listed.entry.name == *selected);
            let source = matches
                .next()
                .ok_or_else(|| format!("Selected item '{selected}' is no longer listed"))?;
            if matches.next().is_some() {
                return Err(format!(
                    "Selected item '{selected}' is ambiguous in the current listing"
                ));
            }
            Ok(source)
        }
        _ => Err("WebDAV copy currently supports one selected item".to_string()),
    }
}

/// Build the frozen one-object WebDAV spec from ONE explicitly resolved ACTIVE
/// source. There is no passive-row parameter, preventing active/passive source
/// inversion by construction.
pub fn build_webdav_copy_spec(
    src_loc: &Location,
    dst_loc: &Location,
    source: &ListedEntry,
) -> Result<WebDavTransferSpec, String> {
    match (src_loc, dst_loc) {
        (
            Location::Local(local_root),
            Location::WebDav {
                target,
                path: destination_path,
            },
        ) => {
            let filename = source.entry.name.as_str();
            let destination = webdav_write_child_target(target, destination_path, filename)
                .map_err(|e| e.to_string())?;
            match source.entry.kind {
                EntryKind::File => Ok(WebDavTransferSpec::UploadOne {
                    local_source: local_root.join(filename),
                    destination,
                }),
                EntryKind::Directory => Ok(WebDavTransferSpec::UploadTree {
                    local_source: local_root.join(filename),
                    destination_root: destination,
                }),
                _ => Err("WebDAV upload requires a regular file or directory".into()),
            }
        }
        (Location::WebDav { target, .. }, Location::Local(local_root)) => {
            match (&source.entry.kind, &source.identity) {
                (EntryKind::File, EntryIdentity::WebDavObject(object)) => {
                    if object.target != *target {
                        return Err(format!(
                            "WebDAV object target '{}' does not match pane target '{}'",
                            object.target, target
                        ));
                    }
                    let local_name = webdav_download_local_name(object, &source.entry.name)
                        .map_err(|e| e.to_string())?;
                    Ok(WebDavTransferSpec::DownloadOne {
                        source: object.clone(),
                        local_destination: local_root.join(local_name),
                    })
                }
                (EntryKind::Directory, EntryIdentity::WebDavCollection(collection)) => {
                    if collection.target != *target {
                        return Err(format!(
                            "WebDAV collection target '{}' does not match pane target '{}'",
                            collection.target, target
                        ));
                    }
                    validate_webdav_local_component(&source.entry.name).map_err(|_| {
                        "collection cannot be represented as one safe local directory name"
                            .to_string()
                    })?;
                    Ok(WebDavTransferSpec::DownloadTree {
                        source: collection.clone(),
                        local_destination: local_root.join(&source.entry.name),
                    })
                }
                (EntryKind::Directory, _) => {
                    Err("Recursive download requires an exact WebDAV collection identity".into())
                }
                _ => Err("WebDAV download requires an exact file or collection identity".into()),
            }
        }
        (
            Location::WebDav {
                target: source_target,
                ..
            },
            Location::WebDav {
                target: destination_target,
                path: destination_path,
            },
        ) => match (&source.entry.kind, &source.identity) {
            (EntryKind::Directory, EntryIdentity::WebDavCollection(collection)) => {
                if collection.target != *source_target {
                    return Err(format!(
                        "WebDAV collection target '{}' does not match source pane target '{}'",
                        collection.target, source_target
                    ));
                }
                let destination_root = webdav_write_child_target(
                    destination_target,
                    destination_path,
                    &source.entry.name,
                )
                .map_err(|error| error.to_string())?;
                Ok(WebDavTransferSpec::CopyTree {
                    source: collection.clone(),
                    destination_root,
                })
            }
            (EntryKind::Directory, _) => {
                Err("WebDAV to WebDAV recursive copy requires an exact collection identity".into())
            }
            _ => Err("WebDAV to WebDAV copy currently supports exactly one collection root".into()),
        },
        _ => Err("WebDAV copy requires Local/WebDAV or WebDAV/WebDAV panes".into()),
    }
}

/// ONE canonical preparation seam shared by product F5 and physical acceptance.
/// Returns the spec and the exact single queue/display name from the SAME
/// resolved source ListedEntry.
pub fn prepare_webdav_copy(
    src_loc: &Location,
    dst_loc: &Location,
    selected_names: &[String],
    focused_source: Option<&ListedEntry>,
    current_active_listed: &[&ListedEntry],
) -> Result<(WebDavTransferSpec, String), String> {
    let source = resolve_webdav_copy_source(selected_names, focused_source, current_active_listed)?;
    let spec = build_webdav_copy_spec(src_loc, dst_loc, source)?;
    Ok((spec, source.entry.name.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::capabilities::{LOCAL_CAPABILITIES, SFTP_CAPABILITIES, builtin_capabilities};
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
            webdav_spec: None,
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
                webdav: false,
            },
            delete_extraneous: false,
            s3_spec: None,
            webdav_spec: None,
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
                webdav: false,
            },
            delete_extraneous: false,
            s3_spec: None,
            webdav_spec: None,
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
    fn webdav_remote_copy_and_move_require_matching_frozen_specs() {
        let source = crate::vfs::WebDavCollectionRef {
            target: "dav-a".into(),
            href: "/dav/source/".into(),
        };
        let destination_root = WebDavWriteTarget {
            target: "dav-b".into(),
            logical_path: "/copied".into(),
        };
        let base = TransferRequest {
            source: Location::WebDav {
                target: "dav-a".into(),
                path: "/source".into(),
            },
            destination: Location::WebDav {
                target: "dav-b".into(),
                path: "/".into(),
            },
            source_provider: ProviderId::WebDAV,
            destination_provider: ProviderId::WebDAV,
            source_capabilities: builtin_capabilities(ProviderId::WebDAV),
            destination_capabilities: builtin_capabilities(ProviderId::WebDAV),
            intent: TransferIntent::Copy,
            executors: ExecutorAvailability {
                native: false,
                rsync: false,
                sftp: false,
                s3: false,
                webdav: true,
            },
            delete_extraneous: false,
            s3_spec: None,
            webdav_spec: Some(WebDavTransferSpec::CopyTree {
                source: source.clone(),
                destination_root: destination_root.clone(),
            }),
        };
        assert_eq!(
            TransferPlanner::plan(base.clone()).unwrap().method,
            TransferMethod::WebDav
        );
        let mut without_spec = base.clone();
        without_spec.webdav_spec = None;
        assert!(TransferPlanner::plan(without_spec).is_err());

        let mut moved_with_copy = base.clone();
        moved_with_copy.intent = TransferIntent::Move;
        assert!(TransferPlanner::plan(moved_with_copy).is_err());

        let mut move_request = base.clone();
        move_request.intent = TransferIntent::Move;
        move_request.webdav_spec = Some(WebDavTransferSpec::MoveTree {
            source,
            destination_root,
        });
        assert_eq!(
            TransferPlanner::plan(move_request.clone()).unwrap().method,
            TransferMethod::WebDav
        );
        move_request.intent = TransferIntent::Copy;
        assert!(TransferPlanner::plan(move_request).is_err());
    }

    #[test]
    fn remote_to_remote_uses_sftp_and_never_rsync() {
        let plan = TransferPlanner::plan(TransferRequest {
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
                webdav: false,
            },
            delete_extraneous: false,
            s3_spec: None,
            webdav_spec: None,
        })
        .unwrap();

        assert_eq!(plan.method, TransferMethod::Sftp);
    }

    #[test]
    fn remote_to_remote_requires_sftp_executor() {
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
                sftp: false,
                s3: false,
                webdav: false,
            },
            delete_extraneous: false,
            s3_spec: None,
            webdav_spec: None,
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
    fn remote_to_remote_move_remains_unsupported() {
        let error = TransferPlanner::plan(TransferRequest {
            source: sftp("prod-a", "/src"),
            destination: sftp("prod-b", "/dst"),
            source_provider: ProviderId::Sftp,
            destination_provider: ProviderId::Sftp,
            source_capabilities: SFTP_CAPABILITIES,
            destination_capabilities: SFTP_CAPABILITIES,
            intent: TransferIntent::Move,
            executors: ExecutorAvailability {
                native: false,
                rsync: false,
                sftp: true,
                s3: false,
                webdav: false,
            },
            delete_extraneous: false,
            s3_spec: None,
            webdav_spec: None,
        })
        .unwrap_err();

        assert!(matches!(
            error,
            TransferPlanError::Unsupported {
                source: ProviderId::Sftp,
                destination: ProviderId::Sftp,
                intent: TransferIntent::Move
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
                webdav: false,
            },
            delete_extraneous: true,
            s3_spec: None,
            webdav_spec: None,
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
            webdav_spec: None,
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

    // ── S3-31/32/33: planner selection matrix (S3-31R seam) ──

    fn object() -> S3ObjectRef {
        S3ObjectRef {
            target: "tgt".into(),
            bucket: "bk".into(),
            key: "a.txt".into(),
        }
    }

    fn upload_spec() -> S3TransferSpec {
        S3TransferSpec::UploadOne {
            local_source: PathBuf::from("/src/a.txt"),
            destination: object(),
        }
    }

    fn download_spec() -> S3TransferSpec {
        S3TransferSpec::DownloadOne {
            source: object(),
            local_destination: PathBuf::from("/dst/a.txt"),
        }
    }

    fn location_for(provider: ProviderId) -> Location {
        match provider {
            ProviderId::Local => local("/src/a.txt"),
            ProviderId::Sftp => sftp("prod", "/dst"),
            ProviderId::Archive => Location::Archive {
                archive: PathBuf::from("/a.zip"),
                inner_path: String::new(),
            },
            ProviderId::S3 => s3("tgt", Some("bk")),
            ProviderId::WebDAV => unreachable!("WebDAV is not part of the S3 planner matrix"),
        }
    }

    /// Every legacy executor is available on purpose: an S3-touching request
    /// must never fall through to the Native/Rsync/Sftp branches.
    fn s3_request(
        source_provider: ProviderId,
        destination_provider: ProviderId,
        intent: TransferIntent,
        s3_executor: bool,
        s3_spec: Option<S3TransferSpec>,
    ) -> TransferRequest {
        TransferRequest {
            source: location_for(source_provider),
            destination: location_for(destination_provider),
            source_provider,
            destination_provider,
            source_capabilities: builtin_capabilities(source_provider),
            destination_capabilities: builtin_capabilities(destination_provider),
            intent,
            executors: ExecutorAvailability {
                native: true,
                rsync: true,
                sftp: true,
                s3: s3_executor,
                webdav: false,
            },
            delete_extraneous: false,
            s3_spec,
            webdav_spec: None,
        }
    }

    #[track_caller]
    fn assert_unsupported(request: TransferRequest, case: &str) {
        let expected = (
            request.source_provider,
            request.destination_provider,
            request.intent,
        );
        match TransferPlanner::plan(request) {
            Err(TransferPlanError::Unsupported {
                source,
                destination,
                intent,
            }) => assert_eq!((source, destination, intent), expected, "{case}"),
            other => panic!("{case}: expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn local_to_s3_copy_with_executor_and_spec_selects_s3() {
        let spec = upload_spec();
        let plan = TransferPlanner::plan(s3_request(
            ProviderId::Local,
            ProviderId::S3,
            TransferIntent::Copy,
            true,
            Some(spec.clone()),
        ))
        .unwrap();
        assert_eq!(plan.method, TransferMethod::S3);
        assert_eq!(plan.s3_spec, Some(spec));
    }

    #[test]
    fn s3_to_local_copy_with_executor_and_spec_selects_s3() {
        let spec = download_spec();
        let plan = TransferPlanner::plan(s3_request(
            ProviderId::S3,
            ProviderId::Local,
            TransferIntent::Copy,
            true,
            Some(spec.clone()),
        ))
        .unwrap();
        assert_eq!(plan.method, TransferMethod::S3);
        assert_eq!(plan.s3_spec, Some(spec));
    }

    #[test]
    fn s3_copy_without_s3_executor_is_unsupported() {
        assert_unsupported(
            s3_request(
                ProviderId::Local,
                ProviderId::S3,
                TransferIntent::Copy,
                false,
                Some(upload_spec()),
            ),
            "local->s3 copy, no executor, spec present",
        );
        assert_unsupported(
            s3_request(
                ProviderId::Local,
                ProviderId::S3,
                TransferIntent::Copy,
                false,
                None,
            ),
            "local->s3 copy, no executor, no spec",
        );
        assert_unsupported(
            s3_request(
                ProviderId::S3,
                ProviderId::Local,
                TransferIntent::Copy,
                false,
                Some(download_spec()),
            ),
            "s3->local copy, no executor",
        );
    }

    #[test]
    fn s3_copy_without_frozen_spec_is_unsupported() {
        // No name-based fallback: the frozen spec is the only bucket/key source.
        assert_unsupported(
            s3_request(
                ProviderId::Local,
                ProviderId::S3,
                TransferIntent::Copy,
                true,
                None,
            ),
            "local->s3 copy, executor, no spec",
        );
        assert_unsupported(
            s3_request(
                ProviderId::S3,
                ProviderId::Local,
                TransferIntent::Copy,
                true,
                None,
            ),
            "s3->local copy, executor, no spec",
        );
    }

    #[test]
    fn s3_pairs_other_than_local_are_unsupported() {
        for (source, destination) in [
            (ProviderId::S3, ProviderId::S3),
            (ProviderId::S3, ProviderId::Sftp),
            (ProviderId::Sftp, ProviderId::S3),
            (ProviderId::Archive, ProviderId::S3),
            (ProviderId::S3, ProviderId::Archive),
        ] {
            assert_unsupported(
                s3_request(
                    source,
                    destination,
                    TransferIntent::Copy,
                    true,
                    Some(upload_spec()),
                ),
                &format!("{source:?}->{destination:?} copy"),
            );
        }
    }

    #[test]
    fn s3_move_and_synchronize_are_unsupported() {
        assert_unsupported(
            s3_request(
                ProviderId::Local,
                ProviderId::S3,
                TransferIntent::Move,
                true,
                Some(upload_spec()),
            ),
            "local->s3 move",
        );
        assert_unsupported(
            s3_request(
                ProviderId::S3,
                ProviderId::Local,
                TransferIntent::Move,
                true,
                Some(download_spec()),
            ),
            "s3->local move",
        );
        assert_unsupported(
            s3_request(
                ProviderId::Local,
                ProviderId::S3,
                TransferIntent::Synchronize,
                true,
                Some(upload_spec()),
            ),
            "local->s3 synchronize",
        );
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
            webdav_spec: None,
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
    #[test]
    fn webdav_copy_source_truth_t1_through_t10() {
        fn file(name: &str) -> ListedEntry {
            ListedEntry {
                entry: crate::vfs::Entry {
                    name: name.into(),
                    kind: EntryKind::File,
                    size: Some(1),
                    modified_unix_ms: None,
                },
                identity: EntryIdentity::Other,
            }
        }
        fn dav(name: &str, target: &str, href: &str) -> ListedEntry {
            ListedEntry {
                entry: crate::vfs::Entry {
                    name: name.into(),
                    kind: EntryKind::File,
                    size: Some(1),
                    modified_unix_ms: None,
                },
                identity: EntryIdentity::WebDavObject(WebDavObjectRef {
                    target: target.into(),
                    href: href.into(),
                }),
            }
        }
        let local_src = Location::Local(PathBuf::from("/active"));
        let local_dst = Location::Local(PathBuf::from("/passive"));
        let dav_root = Location::WebDav {
            target: "target-A".into(),
            path: "/files/".into(),
        };
        let a = file("A.txt");
        let b = file("B.txt");

        // T1: no selection => ACTIVE focused Local A; queue/spec coupled to A.
        let (spec, name) =
            prepare_webdav_copy(&local_src, &dav_root, &[], Some(&a), &[&a, &b]).expect("T1");
        assert_eq!(name, "A.txt");
        match spec {
            WebDavTransferSpec::UploadOne {
                local_source,
                destination,
            } => {
                assert_eq!(local_source, PathBuf::from("/active/A.txt"));
                assert_eq!(destination.target, "target-A");
                assert_eq!(destination.logical_path, "/files/A.txt");
            }
            _ => panic!("T1 expected upload"),
        }

        // T2/T8/T10: selection A beats focus B; passive row cannot enter API;
        // returned queue name comes from the SAME A used for UploadOne.
        let (spec, name) = prepare_webdav_copy(
            &local_src,
            &dav_root,
            &["A.txt".into()],
            Some(&b),
            &[&a, &b],
        )
        .expect("T2");
        assert_eq!(name, "A.txt");
        assert!(matches!(
            spec,
            WebDavTransferSpec::UploadOne { local_source, .. }
                if local_source == std::path::Path::new("/active/A.txt")
        ));

        // T3/T10: selected exact A href beats focused B href; queue name A.
        let da = dav("A.txt", "target-A", "/native/EXACT_A");
        let db = dav("B.txt", "target-A", "/native/EXACT_B");
        let (spec, name) = prepare_webdav_copy(
            &dav_root,
            &local_dst,
            &["A.txt".into()],
            Some(&db),
            &[&da, &db],
        )
        .expect("T3");
        assert_eq!(name, "A.txt");
        assert!(matches!(
            spec,
            WebDavTransferSpec::DownloadOne { source, local_destination }
                if source.href == "/native/EXACT_A"
                    && local_destination == std::path::Path::new("/passive/A.txt")
        ));

        // T4: multi-selection fails closed.
        assert_eq!(
            prepare_webdav_copy(
                &local_src,
                &dav_root,
                &["A.txt".into(), "B.txt".into()],
                Some(&b),
                &[&a, &b]
            )
            .unwrap_err(),
            "WebDAV copy currently supports one selected item"
        );

        // T5: stale selection A with only B current; never falls back to focus B.
        let stale = prepare_webdav_copy(&local_src, &dav_root, &["A.txt".into()], Some(&b), &[&b])
            .unwrap_err();
        assert!(stale.contains("no longer listed"));

        // T6: ambiguous current presentation name; never picks first identity.
        let dup1 = dav("duplicate", "target-A", "/native/one");
        let dup2 = dav("duplicate", "target-A", "/native/two");
        let ambiguous = prepare_webdav_copy(
            &dav_root,
            &local_dst,
            &["duplicate".into()],
            Some(&dup1),
            &[&dup1, &dup2],
        )
        .unwrap_err();
        assert!(ambiguous.contains("ambiguous"));

        // T7: Parent/LoadMore/no row are represented as no real ListedEntry.
        assert!(prepare_webdav_copy(&local_src, &dav_root, &[], None, &[]).is_err());

        // T9: exact object target must equal ACTIVE WebDAV Location target.
        let wrong = dav("A.txt", "target-B", "/native/A");
        assert!(
            prepare_webdav_copy(&dav_root, &local_dst, &[], Some(&wrong), &[&wrong])
                .unwrap_err()
                .contains("does not match")
        );
    }

    #[test]
    fn webdav_builder_rejects_non_file_and_unsupported_pair() {
        let dir = ListedEntry {
            entry: crate::vfs::Entry {
                name: "dir".into(),
                kind: EntryKind::Directory,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::Other,
        };
        assert!(
            build_webdav_copy_spec(
                &Location::Local(PathBuf::from("/a")),
                &Location::Local(PathBuf::from("/b")),
                &dir,
            )
            .is_err()
        );
    }
    #[test]
    fn webdav_recursive_prepare_r1_r4() {
        let collection = ListedEntry {
            entry: crate::vfs::Entry {
                name: "unicodé root".into(),
                kind: EntryKind::Directory,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::WebDavCollection(crate::vfs::WebDavCollectionRef {
                target: "dav-a".into(),
                href: "/remote/exact%20root/?version=7".into(),
            }),
        };
        let source = Location::WebDav {
            target: "dav-a".into(),
            path: "/presentation/ignored/".into(),
        };
        let destination = Location::Local(PathBuf::from("/downloads"));
        let (spec, queue_name) = prepare_webdav_copy(
            &source,
            &destination,
            &[],
            Some(&collection),
            &[&collection],
        )
        .expect("R1 collection download");
        assert_eq!(queue_name, "unicodé root");
        assert!(matches!(
            spec,
            WebDavTransferSpec::DownloadTree { source, local_destination }
                if source.href == "/remote/exact%20root/?version=7"
                    && local_destination == std::path::Path::new("/downloads/unicodé root")
        ));

        let wrong = ListedEntry {
            identity: EntryIdentity::WebDavCollection(crate::vfs::WebDavCollectionRef {
                target: "dav-b".into(),
                href: "/remote/root/".into(),
            }),
            ..collection.clone()
        };
        assert!(
            prepare_webdav_copy(&source, &destination, &[], Some(&wrong), &[&wrong])
                .unwrap_err()
                .contains("does not match")
        ); // R2

        let local_dir = ListedEntry {
            entry: crate::vfs::Entry {
                name: "dir".into(),
                kind: EntryKind::Directory,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::Other,
        };
        let dav_dst = Location::WebDav {
            target: "dav-a".into(),
            path: "/dst/".into(),
        };
        let (tree, queue) = prepare_webdav_copy(
            &Location::Local(PathBuf::from("/src")),
            &dav_dst,
            &[],
            Some(&local_dir),
            &[&local_dir],
        )
        .expect("local directory prepares UploadTree");
        assert_eq!(queue, "dir");
        assert!(matches!(
            tree,
            WebDavTransferSpec::UploadTree { local_source, destination_root }
                if local_source == std::path::Path::new("/src/dir")
                    && destination_root.logical_path == "/dst/dir"
        ));

        // R4: accepted #246 multi-selection remains fail-closed.
        assert_eq!(
            prepare_webdav_copy(
                &source,
                &destination,
                &["a".into(), "b".into()],
                Some(&collection),
                &[&collection]
            )
            .unwrap_err(),
            "WebDAV copy currently supports one selected item"
        );
    }
}

/// Real physical acceptance test for S3 basic transfer (S3-42 gate).
///
/// Runs ONLY when `ARX_TEST_S3_ENDPOINT` is set (e.g. a local MinIO instance).
/// It exercises the actual `upload_one`/`download_one` arx code paths against a
/// live S3-compatible endpoint — no aws-cli, no shell fallback. This is the
/// physical acceptance evidence required to flip `S3_CAPABILITIES::Write`.
#[cfg(test)]
mod physical_acceptance {
    use super::*;
    use crate::config::S3TargetConfig;
    use crate::transfer::s3_upload::S3OverwritePolicy;
    use crate::vfs::s3::S3Provider;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use tempfile::tempdir;

    fn minio_target() -> Option<S3TargetConfig> {
        let endpoint = std::env::var("ARX_TEST_S3_ENDPOINT").ok()?;
        let bucket = std::env::var("ARX_TEST_S3_BUCKET").unwrap_or_else(|_| "arxtest".into());
        Some(S3TargetConfig {
            id: "phys-accept".into(),
            name: "phys-accept".into(),
            bucket: Some(bucket),
            region: Some("us-east-1".into()),
            profile: None,
            endpoint_url: Some(endpoint),
            force_path_style: true,
        })
    }

    #[tokio::test]
    async fn s3_upload_download_roundtrip_against_live_endpoint() {
        let Some(target) = minio_target() else {
            eprintln!("skipping physical acceptance: ARX_TEST_S3_ENDPOINT not set");
            return;
        };
        // Credentials come from the default AWS SDK chain (AWS_ACCESS_KEY_ID /
        // AWS_SECRET_ACCESS_KEY); set them to the MinIO root keys before running.
        let provider = S3Provider::new(target);
        let dir = tempdir().unwrap();
        let src = dir.path().join("upload.bin");
        let payload: Vec<u8> = (0u8..=255).cycle().take(100_000).collect();
        std::fs::write(&src, &payload).unwrap();

        let key = format!("arx-phys-accept/{}.bin", uuid_like());
        let object = S3ObjectRef {
            target: "phys-accept".into(),
            bucket: "arxtest".into(),
            key: key.clone(),
        };

        // Upload through the real arx core (exactly one PutObject).
        let up = S3TransferSpec::UploadOne {
            local_source: src.clone(),
            destination: object.clone(),
        };
        let written = s3_upload::upload_one(
            &provider,
            &up,
            S3OverwritePolicy::Forbid,
            Arc::new(AtomicBool::new(false)),
            &mut |_| {},
        )
        .await
        .expect("upload must succeed against live endpoint");
        assert_eq!(written as usize, payload.len());

        // Download back through the real arx core (full GetObject + staging + fsync).
        let dst = dir.path().join("download.bin");
        let down = S3TransferSpec::DownloadOne {
            source: object.clone(),
            local_destination: dst.clone(),
        };
        let got = s3_download::download_one(
            &provider,
            &down,
            Arc::new(AtomicBool::new(false)),
            crate::transfer_queue::PauseGate::disabled(),
            &mut |_| {},
        )
        .await
        .expect("download must succeed against live endpoint");
        assert_eq!(got as usize, payload.len());

        let back = std::fs::read(&dst).unwrap();
        assert_eq!(back, payload, "roundtrip bytes must match exactly");
    }

    fn uuid_like() -> String {
        // ponytail: monotonic-ish token for unique keys; no crate needed
        use std::time::{SystemTime, UNIX_EPOCH};
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("{n}")
    }
}
