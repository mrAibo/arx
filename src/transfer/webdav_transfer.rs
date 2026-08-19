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
use tokio::fs::{self, OpenOptions};
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
    _overwrite: WebDavOverwritePolicy,
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

    // 3. PUT to the exact href (the frozen identity's raw href)
    provider.put(&destination.href, &data).await?;

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
    cancel: Arc<AtomicBool>,
) -> io::Result<DownloadOutcome> {
    if cancel.load(Ordering::Relaxed) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "download cancelled",
        ));
    }
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

    // 2. GET the exact raw href from the identity
    let br = provider.get_bounded(&source.href, usize::MAX).await?;

    // 3. Stream to staged file, then atomic rename
    let dest_dir = local_destination
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "destination has no parent"))?;
    let dest_name = local_destination.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "destination has no filename")
    })?;

    let staged = dest_dir.join(format!(
        ".arx-download-{}-{}",
        std::process::id(),
        dest_name.to_string_lossy()
    ));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&staged)
        .await?;

    file.write_all(&br.bytes).await?;
    file.flush().await?;
    // fsync
    file.sync_all().await?;
    drop(file);

    // 4. Atomic rename to final path
    fs::rename(&staged, local_destination).await?;

    Ok(br.bytes.len() as u64)
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
