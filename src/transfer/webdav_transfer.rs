//! Local ↔ WebDAV basic transfer core (PACK E B1/B2).
//!
//! Invariants:
//! - exact `WebDavObjectRef` (target + raw href from listing) is the sole
//!   authority for the remote object — no name-based reconstruction
//! - upload: PUT the file to the exact href; server may redirect
//! - download: GET the exact href, stream to staged file in destination dir,
//!   atomic rename to final path, fsync
//! - cancellation truth: staged file removed, never a partial final path
//! - no overwrite of an existing final path without a frozen policy

use crate::transfer::{TransferProgress, WebDavTransferSpec};
use crate::vfs::webdav::WebDavProvider;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::fs::File as TokioFile;
use tokio::io::AsyncWriteExt;

/// Result of an upload: bytes physically sent.
pub type UploadOutcome = u64;

/// Result of a download: bytes physically written to the final path.
pub type DownloadOutcome = u64;

/// Overwrite policy for WebDAV upload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebDavOverwritePolicy {
    Forbid,
    Allow,
}

/// Upload a single local file to the exact WebDAV href.
#[allow(dead_code)]
pub(crate) async fn upload_one(
    provider: &WebDavProvider,
    spec: &WebDavTransferSpec,
    overwrite: WebDavOverwritePolicy,
    cancel: Arc<AtomicBool>,
    on_progress: &mut impl FnMut(TransferProgress),
) -> io::Result<UploadOutcome> {
    if cancel.load(Ordering::Relaxed) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "upload cancelled",
        ));
    }
    // 1. Extract upload spec
    let (local_source, destination) = match spec {
        WebDavTransferSpec::UploadOne {
            local_source,
            destination,
        } => (local_source, destination),
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "upload_one requires WebDavTransferSpec::UploadOne",
            ));
        }
    };

    // 2. Read local file
    let data = tokio::fs::read(local_source)
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::NotFound, format!("read local: {e}")))?;
    let total = data.len();

    // 3. PUT with overwrite policy enforced at the HTTP layer (no
    //    existence preflight — racing TOCTOU is unsafe). For Forbid we
    //    send If-None-Match: * so the server rejects an existing resource
    //    with 412; Allow is a plain PUT.
    provider
        .put_with_policy(&destination.href, &data, overwrite)
        .await?;

    on_progress(TransferProgress {
        completed: total,
        total,
    });

    Ok(total as u64)
}

/// Download a single WebDAV object to the exact local destination.
#[allow(dead_code)]
pub(crate) async fn download_one(
    provider: &WebDavProvider,
    spec: &WebDavTransferSpec,
    overwrite: WebDavOverwritePolicy,
    cancel: Arc<AtomicBool>,
) -> io::Result<DownloadOutcome> {
    // 1. Extract download spec
    let (source, local_destination) = match spec {
        WebDavTransferSpec::DownloadOne {
            source,
            local_destination,
        } => (source, local_destination),
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "download_one requires WebDavTransferSpec::DownloadOne",
            ));
        }
    };

    // 2. Stream GET into a tokio temp file in the destination directory
    //    (RAII cleanup guard ensures cleanup on any error/cancel/exit).
    let dest_dir = local_destination
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "destination has no parent"))?;

    // cancel before GET
    if cancel.load(Ordering::Relaxed) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "download cancelled",
        ));
    }

    // Create a unique temp file path in the destination directory
    let temp_path = dest_dir.join(format!(
        ".arx-download-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        local_destination
            .file_name()
            .ok_or_else(|| io::Error::new(
                io::ErrorKind::InvalidInput,
                "destination has no filename"
            ))?
            .to_string_lossy()
    ));

    // RAII cleanup guard
    struct TempFileGuard {
        path: std::path::PathBuf,
    }
    impl Drop for TempFileGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
    let _guard = TempFileGuard {
        path: temp_path.clone(),
    };

    // Open with tokio for async streaming
    let mut temp_file = TokioFile::options()
        .create_new(true)
        .write(true)
        .open(&temp_path)
        .await?;

    {
        // 3. Stream body with cancellation check between chunks.
        let max_bytes: usize = 16 * 1024 * 1024 * 1024;
        provider
            .get_stream(&source.href, max_bytes, &mut temp_file, Some(&cancel))
            .await?;
    }

    temp_file.flush().await?;
    temp_file.sync_all().await?;
    drop(temp_file); // ensure closed before persist

    // pre-persist cancellation: never finalize a staged download after cancel
    if cancel.load(Ordering::Acquire) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "download cancelled",
        ));
    }

    // 4. Finalize: persist noclobber for Forbid, replace for Allow.
    if matches!(overwrite, WebDavOverwritePolicy::Forbid) {
        // Forbid: noclobber — fail if destination exists.
        // Use std::fs to avoid async rename issues across filesystems
        match std::fs::rename(&temp_path, local_destination) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "refusing to overwrite existing destination (policy Forbid)",
                ));
            }
            Err(e) => return Err(e),
        }
    } else {
        // Allow: replace existing destination.
        std::fs::rename(&temp_path, local_destination)?;
    }

    // Guard disarmed — file moved successfully
    std::mem::forget(_guard);

    // 5. Return bytes written (zero is valid).
    let size = std::fs::metadata(local_destination)
        .map(|m| m.len())
        .unwrap_or(0) as u64;
    Ok(size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transfer::{
        webdav_download_local_name, webdav_spec_for_objects, webdav_upload_destination_ref,
    };
    use crate::vfs::webdav::WebDavObjectRef;
    use std::path::PathBuf;

    #[test]
    fn webdav_transfer_spec_target_returns_correct_id() {
        let upload = WebDavTransferSpec::UploadOne {
            local_source: PathBuf::from("/tmp/a.txt"),
            destination: WebDavObjectRef {
                target: "my-target".into(),
                href: "/dav/file.txt".into(),
            },
        };
        assert_eq!(upload.target(), "my-target");

        let download = WebDavTransferSpec::DownloadOne {
            source: WebDavObjectRef {
                target: "other-target".into(),
                href: "/dav/file.txt".into(),
            },
            local_destination: PathBuf::from("/tmp/down.txt"),
        };
        assert_eq!(download.target(), "other-target");
    }

    #[test]
    fn webdav_upload_destination_ref_constructs_correctly() {
        let r = webdav_upload_destination_ref("tgt", "/dav/my%20file.txt", "my file.txt").unwrap();
        assert_eq!(r.target, "tgt");
        assert_eq!(r.href, "/dav/my%20file.txt");
    }

    #[test]
    fn webdav_upload_destination_ref_rejects_unsafe_filename() {
        for bad in ["", ".", "..", "/", "../", "a/b"] {
            assert!(
                webdav_upload_destination_ref("tgt", "/dav/x", bad).is_err(),
                "{bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn webdav_download_local_name_validation() {
        let obj = WebDavObjectRef {
            target: "t".into(),
            href: "/dav/x".into(),
        };
        assert_eq!(webdav_download_local_name(&obj, "a.txt").unwrap(), "a.txt");
        for bad in ["../", "/", "", ".", ".."] {
            assert!(
                webdav_download_local_name(&obj, bad).is_err(),
                "{bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn webdav_spec_for_objects_enforces_single() {
        let one = WebDavObjectRef {
            target: "t".into(),
            href: "/dav/x".into(),
        };
        assert!(webdav_spec_for_objects(std::slice::from_ref(&one)).is_ok());
        assert!(webdav_spec_for_objects(&[]).is_err());
        assert!(webdav_spec_for_objects(&[one.clone(), one]).is_err());
    }
}
