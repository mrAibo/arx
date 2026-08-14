//! Local → S3 download core (S3-37/38). Typed internal API only at this seam
//! stage; the executor body is implemented by the S3-37/38 card.
//!
//! Invariants (enforced by the implementer, not here):
//! - exact `S3ObjectRef` bucket/key authority; no name-based reconstruction
//! - GetObject full transfer (no Range), streamed in chunks; never collect the
//!   whole object into RAM
//! - staged file in the destination directory, atomic rename to final
//! - fsync + collision-safe staging; leftover artifacts reported truthfully
//! - cancellation truth: staged file removed, never a partial final path
//! - no overwrite of an existing final path without a frozen policy

use crate::transfer::S3TransferSpec;
use crate::vfs::s3::S3Provider;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Result of a download: bytes physically written to the final path.
pub type DownloadOutcome = u64;

/// Download a single S3 object to the exact local destination.
///
/// Seam signature only — implemented by S3-37/38. The default body is a
/// placeholder so the module compiles before the core lands.
#[allow(dead_code)]
pub(crate) async fn download_one(
    _provider: &S3Provider,
    _spec: &S3TransferSpec,
    _cancel: Arc<AtomicBool>,
) -> std::io::Result<DownloadOutcome> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "S3 download core not implemented (S3-37/38)",
    ))
}

/// Convenience: resolve the download local destination for a `DownloadOne` spec.
#[allow(dead_code)]
pub(crate) fn download_local_destination(spec: &S3TransferSpec) -> Option<PathBuf> {
    match spec {
        S3TransferSpec::DownloadOne {
            local_destination, ..
        } => Some(local_destination.clone()),
        _ => None,
    }
}
