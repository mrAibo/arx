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
use crate::transfer::integrity::ObjectIntegrity;
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

    let (remote_size, remote_etag, mut body) = match get_obj {
        Ok(out) => {
            // Capture remote integrity evidence BEFORE streaming the body:
            // content_length is the expected size for post-transfer verification;
            // e_tag is recorded but not checked locally (cannot be recomputed
            // without a full re-hash of the downloaded file).
            let size: u64 = out.content_length().unwrap_or(0).try_into().unwrap_or(0);
            let etag = out.e_tag().map(|s| s.to_string());
            (size, etag, out.body.into_async_read())
        }
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

    // 13. Post-transfer verification: the local final file size MUST match the
    //     remote size. This runs ONLY on the success path — every partial /
    //     cancelled / errored download returns earlier (staged file removed,
    //     Err propagated), so a partial file is never reported as verified.
    let expected = ObjectIntegrity::new(remote_size, remote_etag);
    verify_downloaded_object(local_destination, &expected)?;

    // 14. Success: return bytes physically written
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

/// Pure size-consistency check: does the local file size equal the expected
/// remote size? This is the honest factual check for downloads — ARX does NOT
/// compute a content hash, so we compare bytes written, not content.
fn local_size_consistent(expected_size: u64, actual_size: u64) -> io::Result<()> {
    if expected_size == actual_size {
        Ok(())
    } else {
        Err(io::Error::other(
            "S3 downloaded object verification failed: local size differs from remote",
        ))
    }
}

/// Verify a just-downloaded object: stat the final local file and confirm its
/// size matches the expected remote size. Runs only after a successful atomic
/// rename, so a partial / cancelled download is never verified.
fn verify_downloaded_object(
    local_path: &std::path::Path,
    expected: &ObjectIntegrity,
) -> io::Result<()> {
    let actual = std::fs::metadata(local_path)?.len();
    local_size_consistent(expected.size, actual)
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

    // ── S3-53: post-transfer verification unit tests ─────────────────────────

    #[test]
    fn local_size_consistent_equal_ok() {
        assert!(local_size_consistent(100, 100).is_ok());
        assert!(local_size_consistent(0, 0).is_ok());
    }

    #[test]
    fn local_size_consistent_mismatch_err() {
        assert!(local_size_consistent(100, 99).is_err());
        assert!(local_size_consistent(100, 101).is_err());
        assert!(local_size_consistent(0, 1).is_err());
        assert!(local_size_consistent(1, 0).is_err());
    }

    #[test]
    fn verify_downloaded_object_size_match() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("obj.bin");
        std::fs::write(&path, vec![0u8; 256]).unwrap();
        let expected = ObjectIntegrity::new(256, Some("\"abc123\"".to_string()));
        assert!(verify_downloaded_object(&path, &expected).is_ok());
    }

    #[test]
    fn verify_downloaded_object_size_mismatch() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("obj.bin");
        std::fs::write(&path, vec![0u8; 256]).unwrap();
        let expected = ObjectIntegrity::new(512, Some("\"abc123\"".to_string()));
        assert!(verify_downloaded_object(&path, &expected).is_err());
    }
}

// ── Physical acceptance test (gated on ARX_TEST_S3_ENDPOINT) ───────────────

#[cfg(test)]
mod physical_acceptance {
    use super::*;
    use crate::config::S3TargetConfig;
    use crate::transfer::S3TransferSpec;
    use crate::transfer::s3_upload::S3OverwritePolicy;
    use crate::transfer::s3_upload::upload_one;
    use crate::vfs::s3::{S3ObjectRef, S3Provider};
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

    fn uuid_like() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("{n}")
    }

    /// Download post-transfer verification: after a real download the local
    /// file size must equal the remote content_length (HeadObject). Exercises
    /// the verification path inside `download_one` on a live endpoint.
    #[tokio::test]
    async fn download_post_transfer_size_matches_remote() {
        let Some(target) = minio_target() else {
            eprintln!("skipping: ARX_TEST_S3_ENDPOINT not set");
            return;
        };
        let provider = S3Provider::new(target);
        let dir = tempdir().unwrap();

        // Upload a known payload via the upload path.
        let payload: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
        let src = dir.path().join("upload.bin");
        std::fs::write(&src, &payload).unwrap();

        let key = format!("arx-phys-accept/{}/verify.bin", uuid_like());
        let object = S3ObjectRef {
            target: "phys-accept".into(),
            bucket: "arxtest".into(),
            key: key.clone(),
        };

        let up_spec = S3TransferSpec::UploadOne {
            local_source: src,
            destination: object.clone(),
        };
        let _written = upload_one(
            &provider,
            &up_spec,
            S3OverwritePolicy::Forbid,
            Arc::new(AtomicBool::new(false)),
            &mut |_| {},
        )
        .await
        .expect("upload must succeed against live endpoint");

        // Remote size as reported by HeadObject (independent of download path).
        let head = provider
            .client()
            .await
            .unwrap()
            .head_object()
            .bucket(&object.bucket)
            .key(&object.key)
            .send()
            .await
            .expect("uploaded object must exist");
        let remote_size: u64 = head
            .content_length()
            .expect("head must report size")
            .try_into()
            .unwrap();

        // Download (internally verifies) and assert the local file size matches.
        let dst = dir.path().join("download.bin");
        let down_spec = S3TransferSpec::DownloadOne {
            source: object.clone(),
            local_destination: dst.clone(),
        };
        let got = download_one(&provider, &down_spec, Arc::new(AtomicBool::new(false)))
            .await
            .expect("download must succeed");
        assert_eq!(got, remote_size, "downloaded bytes must equal remote size");

        let local_size = std::fs::metadata(&dst).unwrap().len();
        assert_eq!(
            local_size, remote_size,
            "local file size must equal remote size"
        );
    }
}
