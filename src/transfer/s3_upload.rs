//! S3 → Local upload core (S3-34/35). Typed internal API only at this seam
//! stage; the executor body is implemented by the S3-34/35 card.
//!
//! Invariants (enforced by the implementer, not here):
//! - exact `S3ObjectRef` bucket/key authority; no name-based reconstruction
//! - single PutObject, no multipart, no hidden retry (SDK retries stay disabled)
//! - `BASIC_TRANSFER_MAX_BYTES` guard before any PutObject
//! - explicit overwrite policy via HeadObject preflight, fail closed
//! - cancellation truth: no fake-abort semantics for a single PutObject

use crate::transfer::S3TransferSpec;
use crate::vfs::s3::S3Provider;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Temporary single-put ceiling for the basic transfer pack. NOT an S3 service
/// limit; S3-43 may later replace/redefine threshold logic.
pub const BASIC_TRANSFER_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// Explicit frozen overwrite policy. Basic user-facing transfer defaults to
/// `Forbid`; `Confirmed` is only reachable through a separately wired explicit
/// confirmation flow (not in this pack's UI surface).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum S3OverwritePolicy {
    Forbid,
    Confirmed,
}

/// Result of an upload: bytes physically written.
pub type UploadOutcome = u64;

/// Upload a single regular local file to the exact S3 destination ref.
///
/// Seam signature only — implemented by S3-34/35. The default body is a
/// placeholder so the module compiles before the core lands.
#[allow(dead_code)]
pub(crate) async fn upload_one(
    _provider: &S3Provider,
    _spec: &S3TransferSpec,
    _policy: S3OverwritePolicy,
    _cancel: Arc<AtomicBool>,
) -> std::io::Result<UploadOutcome> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "S3 upload core not implemented (S3-34/35)",
    ))
}

/// Convenience: resolve the upload destination path for a `UploadOne` spec.
#[allow(dead_code)]
pub(crate) fn upload_local_source(spec: &S3TransferSpec) -> Option<PathBuf> {
    match spec {
        S3TransferSpec::UploadOne { local_source, .. } => Some(local_source.clone()),
        _ => None,
    }
}
