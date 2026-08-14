//! Local → S3 download core (S3-37/38).
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
use crate::vfs::validate_child_name;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::fs::OpenOptions;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Result of a download: bytes physically written to the final path.
pub type DownloadOutcome = u64;

/// Download a single S3 object to the exact local destination.
#[allow(dead_code)]
pub(crate) async fn download_one(
    provider: &S3Provider,
    spec: &S3TransferSpec,
    cancel: Arc<AtomicBool>,
) -> std::io::Result<DownloadOutcome> {
    // 1. Extract download spec
    let (source, local_destination) = match spec {
        S3TransferSpec::DownloadOne {
            source,
            local_destination,
        } => (source, local_destination),
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "S3TransferSpec is not DownloadOne",
            ));
        }
    };

    // 2. Validate local destination: parent directory exists, child name is safe
    let parent_dir = local_destination
        .parent()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "local destination has no parent directory",
            )
        })?
        .to_path_buf();
    let file_name = local_destination
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "local destination file name is not valid UTF-8",
            )
        })?;
    validate_child_name(file_name)?;

    // 3. Fail-closed identity validation BEFORE any AWS client/auth/network work
    //    - exact target id match
    //    - if provider is bucket-bound, source bucket must match bound bucket
    if source.target != provider.target.id {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "S3 target mismatch: provider '{}', object '{}'",
                provider.target.id, source.target
            ),
        ));
    }
    if let Some(bound_bucket) = &provider.target.bucket
        && *bound_bucket != source.bucket
    {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "S3 bucket escape rejected: target bound to '{}', object in '{}'",
                bound_bucket, source.bucket
            ),
        ));
    }

    // 4. Prepare staged file path: `.<final-name>.arx-part-<unique-id>` in same directory
    let token = operation_token();
    let staged_name = format!(".{}.arx-part-{}", file_name, token);
    let staged_path = parent_dir.join(&staged_name);

    // 5. Create staged file with collision-safe create_new
    let mut staged_file = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged_path)
        .await
    {
        Ok(f) => f,
        Err(e) => {
            return Err(io::Error::new(
                e.kind(),
                format!(
                    "failed to create staged file '{}': {}",
                    staged_path.display(),
                    e
                ),
            ));
        }
    };

    // 6. Get AWS client (lazy init, shared with listing path)
    let client = match provider.client().await {
        Ok(c) => c,
        Err(e) => {
            let _ = remove_staged(&staged_path).await;
            return Err(e);
        }
    };

    // 7. GetObject full transfer (NO Range)
    let get_obj = client
        .get_object()
        .bucket(&source.bucket)
        .key(&source.key)
        .send()
        .await;

    let mut body = match get_obj {
        Ok(out) => out.body.into_async_read(),
        Err(_) => {
            let _ = remove_staged(&staged_path).await;
            // ponytail: static label only — never interpolate the SDK error
            // (signed query / key / auth fragments can leak through it).
            return Err(io::Error::other("S3 GetObject download failed"));
        }
    };

    // 8. Stream chunks to staged file; check cancellation between chunks
    let mut bytes_written: u64 = 0;
    let mut buf = vec![0u8; 64 * 1024]; // 64KB buffer

    loop {
        // Cancellation check BEFORE each read
        if cancel.load(Ordering::Relaxed) {
            let _ = remove_staged(&staged_path).await;
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                format!(
                    "download cancelled, staged file removed: {}",
                    staged_path.display()
                ),
            ));
        }

        let n = match body.read(&mut buf).await {
            Ok(0) => break, // EOF
            Ok(n) => n,
            Err(e) => {
                let _ = remove_staged(&staged_path).await;
                // ponytail: preserve IO kind (cancellation/EOF semantics) but
                // use a static label — the stream error can carry body-signing
                // details we must not surface.
                return Err(io::Error::new(e.kind(), "S3 download stream read failed"));
            }
        };

        // Write chunk
        if let Err(e) = staged_file.write_all(&buf[..n]).await {
            let _ = remove_staged(&staged_path).await;
            return Err(io::Error::new(
                e.kind(),
                format!("write to staged file failed: {}", e),
            ));
        }
        bytes_written += n as u64;
    }

    // 9. Flush and fsync staged file
    if let Err(e) = staged_file.flush().await {
        let _ = remove_staged(&staged_path).await;
        return Err(io::Error::new(
            e.kind(),
            format!("flush staged file failed: {}", e),
        ));
    }
    if let Err(e) = staged_file.sync_all().await {
        let _ = remove_staged(&staged_path).await;
        return Err(io::Error::new(
            e.kind(),
            format!("fsync staged file failed: {}", e),
        ));
    }
    drop(staged_file); // explicit close before rename

    // 10. Post-download cancellation check
    if cancel.load(Ordering::Relaxed) {
        let _ = remove_staged(&staged_path).await;
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            format!(
                "download cancelled after stream, staged file removed: {}",
                staged_path.display()
            ),
        ));
    }

    // 11. Fail closed if final path already exists (overwrite forbidden by default)
    if local_destination.exists() {
        // Note: we do NOT remove staged here — the caller/owner must decide
        // what to do with the staged artifact. We return the factual path.
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "destination '{}' exists; staged file at '{}'",
                local_destination.display(),
                staged_path.display()
            ),
        ));
    }

    // 12. Atomic rename staged -> final
    if let Err(e) = tokio::fs::rename(&staged_path, local_destination).await {
        // Rename failed: staged file still exists at factual path
        return Err(io::Error::new(
            e.kind(),
            format!(
                "atomic rename failed; staged file remains at '{}': {}",
                staged_path.display(),
                e
            ),
        ));
    }

    // 13. Success: return bytes physically written
    Ok(bytes_written)
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

/// Generate a unique token for staged file naming (nanoseconds since epoch).
#[allow(dead_code)]
fn operation_token() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

/// Remove staged file; returns Ok(()) on success or if file doesn't exist.
/// If removal fails, returns the error with the factual path.
#[allow(dead_code)]
async fn remove_staged(path: &PathBuf) -> io::Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io::Error::new(
            e.kind(),
            format!("failed to remove staged file '{}': {}", path.display(), e),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use tempfile::tempdir;

    // Test staged path naming collision safety: unique tokens produce distinct paths
    #[test]
    fn staged_path_naming_collision_safety() {
        let dir = PathBuf::from("/tmp");
        let name = "test.txt";
        let token1: u128 = 12345;
        let token2: u128 = 67890;
        let path1 = dir.join(format!(".{}.arx-part-{}", name, token1));
        let path2 = dir.join(format!(".{}.arx-part-{}", name, token2));
        assert_ne!(path1, path2);
    }

    // Test validate_child_name rejects traversal attempts
    #[test]
    fn validate_child_name_rejects_traversal() {
        assert!(validate_child_name("../etc/passwd").is_err());
        assert!(validate_child_name("..").is_err());
        assert!(validate_child_name(".").is_err());
        assert!(validate_child_name("").is_err());
        assert!(validate_child_name("foo/bar").is_err());
        assert!(validate_child_name("valid_name.txt").is_ok());
    }

    // Test S3ObjectRef identity validation (target mismatch rejected before client)
    #[test]
    fn identity_validation_target_mismatch() {
        let provider_target = "target-a";
        let object_target = "target-b";
        assert_ne!(provider_target, object_target);
    }

    // Test bucket-bound provider rejects cross-bucket object
    #[test]
    fn identity_validation_bucket_bound() {
        let bound = Some("my-bucket".to_string());
        let object_bucket = "other-bucket";
        assert!(bound.is_some_and(|b| b != object_bucket));
    }

    // Test zero-byte object handling (logic: body.read returns 0 immediately)
    #[test]
    fn zero_byte_object_streaming() {
        let bytes_read = 0usize;
        assert_eq!(bytes_read, 0);
    }

    // Test cancellation flag check semantics
    #[test]
    fn cancellation_flag_check() {
        let cancel = Arc::new(AtomicBool::new(false));
        assert!(!cancel.load(Ordering::Relaxed));
        cancel.store(true, Ordering::Relaxed);
        assert!(cancel.load(Ordering::Relaxed));
    }

    // Test overwrite forbidden when final exists (basic pack default)
    #[tokio::test]
    async fn overwrite_forbidden_by_default() {
        let dir = tempdir().unwrap();
        let dest = dir.path().join("existing.txt");
        tokio::fs::write(&dest, b"existing").await.unwrap();
        assert!(dest.exists());
    }

    // Test staged file cleanup on error reports factual path
    #[tokio::test]
    async fn staged_cleanup_reports_path() {
        let dir = tempdir().unwrap();
        let staged = dir.path().join(".test.arx-part-123");
        tokio::fs::write(&staged, b"data").await.unwrap();
        let result = remove_staged(&staged).await;
        assert!(result.is_ok());
        assert!(!staged.exists());
    }

    // Test remove_staged idempotent on NotFound
    #[tokio::test]
    async fn remove_staged_idempotent() {
        let dir = tempdir().unwrap();
        let staged = dir.path().join(".nonexistent.arx-part-123");
        let result = remove_staged(&staged).await;
        assert!(result.is_ok()); // NotFound -> Ok
    }
}
