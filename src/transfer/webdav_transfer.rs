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

use crate::transfer::{WebDavTransferSpec, validate_webdav_local_component};
use crate::transfer_queue::TypedTransferProgress;
use crate::vfs::webdav::WebDavProvider;
use crate::vfs::{EntryIdentity, WebDavCollectionRef, WebDavObjectRef};
use std::collections::{HashSet, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::fs::File as TokioFile;
use tokio::io::AsyncWriteExt;

/// Result of an upload: bytes physically sent.
pub type UploadOutcome = u64;

/// Result of a download: bytes physically written to the final path.
pub type DownloadOutcome = u64;

pub(crate) const MAX_WEBDAV_TREE_DESCENDANTS: usize = 50_000;
pub(crate) const MAX_WEBDAV_TREE_DEPTH: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TreeDirectory {
    pub relative: PathBuf,
    pub source: WebDavCollectionRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TreeFile {
    pub relative: PathBuf,
    pub source: WebDavObjectRef,
    pub advertised_size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WebDavTreeManifest {
    pub directories: Vec<TreeDirectory>,
    pub files: Vec<TreeFile>,
    pub descendant_count: usize,
    pub total_bytes: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
#[error(
    "WebDAV tree failed and cleanup of partial Local root {root} also failed: original={original}; cleanup={cleanup}"
)]
pub(crate) struct TreeCleanupFailure {
    pub root: PathBuf,
    pub original: String,
    pub cleanup: String,
}

fn accept_unique_identity(
    seen: &mut HashSet<String>,
    identity: String,
    message: &'static str,
) -> io::Result<()> {
    if !seen.insert(identity) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, message));
    }
    Ok(())
}

fn accept_local_name(seen: &mut HashSet<String>, name: &str) -> io::Result<()> {
    validate_webdav_local_component(name).map_err(|message| {
        io::Error::new(io::ErrorKind::InvalidData, format!("{message}: {name}"))
    })?;
    if !seen.insert(name.to_string()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("duplicate local presentation name: {name}"),
        ));
    }
    Ok(())
}

fn accept_descendant(count: &mut usize) -> io::Result<()> {
    if *count == MAX_WEBDAV_TREE_DESCENDANTS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "WebDAV tree exceeds 50000 descendants",
        ));
    }
    *count += 1;
    Ok(())
}

fn descendant_depth(parent_depth: usize) -> io::Result<usize> {
    let depth = parent_depth
        .checked_add(1)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "WebDAV tree depth overflow"))?;
    validate_depth(depth)?;
    Ok(depth)
}

fn validate_depth(depth: usize) -> io::Result<()> {
    if depth > MAX_WEBDAV_TREE_DEPTH {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "WebDAV tree exceeds maximum depth 128",
        ));
    }
    Ok(())
}

fn checked_total_bytes(files: &[TreeFile]) -> Option<u64> {
    files
        .iter()
        .try_fold(0u64, |sum, file| sum.checked_add(file.advertised_size?))
}

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
    on_progress: &mut impl FnMut(TypedTransferProgress),
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

    on_progress(TypedTransferProgress::Bytes {
        completed: total as u64,
        total: Some(total as u64),
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
    pause: crate::transfer_queue::PauseGate,
    on_progress: &mut impl FnMut(TypedTransferProgress),
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

    // 2. Stream GET into a secure temp file IN the destination directory.
    //    NamedTempFile is the RAII owner: on drop (any error/cancel/return) the
    //    stage is removed. We write via a tokio wrapper, then finalize.
    let stage = tempfile::NamedTempFile::new_in(dest_dir)
        .map_err(|e| io::Error::other(format!("stage create: {e}")))?;

    // cancel before GET
    if cancel.load(Ordering::Relaxed) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "download cancelled",
        ));
    }

    let written = {
        // Wrap the stage's std file for async streaming.
        let mut temp_file = TokioFile::from_std(
            stage
                .reopen()
                .map_err(|e| io::Error::other(format!("stage reopen: {e}")))?,
        );
        // 3. Stream body with cancellation check between chunks. The provider
        // returns the exact cumulative byte count written to the sink; preserve
        // that fact instead of reconstructing it later from best-effort metadata.
        let max_bytes: usize = 16 * 1024 * 1024 * 1024;
        let written = provider
            .get_stream(
                &source.href,
                max_bytes,
                &mut temp_file,
                Some(&cancel),
                Some(&pause),
                |completed, total| {
                    on_progress(stream_progress(completed, total));
                },
            )
            .await?;
        temp_file.flush().await?;
        temp_file.sync_all().await?;
        // tokio wrapper dropped here -> std file closed; NamedTempFile still owns path.
        written
    };

    // pre-persist cancellation: never finalize a staged download after cancel
    if cancel.load(Ordering::Acquire) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "download cancelled",
        ));
    }

    // 4. Finalize: real noclobber for Forbid (persist_noclobber), replace for Allow.
    if matches!(overwrite, WebDavOverwritePolicy::Forbid) {
        match stage.persist_noclobber(local_destination) {
            Ok(_final) => {}
            Err(e) if e.error.kind() == io::ErrorKind::AlreadyExists => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "refusing to overwrite existing destination (policy Forbid)",
                ));
            }
            Err(e) => return Err(e.error),
        }
    } else {
        // Allow: replace existing destination.
        stage.persist(local_destination).map_err(|e| e.error)?;
    }

    // 5. Verify the committed final file still matches the exact streamed-byte
    // outcome. A metadata/stat failure after persist is a factual post-commit
    // error, never a successful synthetic `0 bytes` result. Zero remains valid
    // only when both the stream and final file are actually zero bytes.
    let final_size = std::fs::metadata(local_destination)?.len();
    if final_size != written {
        return Err(io::Error::other(
            "WebDAV downloaded object verification failed: final size differs from streamed bytes",
        ));
    }

    Ok(written)
}

pub(crate) async fn build_tree_manifest(
    provider: &WebDavProvider,
    root: &WebDavCollectionRef,
    cancel: &AtomicBool,
    pause: &crate::transfer_queue::PauseGate,
) -> io::Result<WebDavTreeManifest> {
    let mut directories = Vec::new();
    let mut files = Vec::new();
    let mut descendants = 0usize;
    let root_identity = provider.canonical_exact_href_identity(&root.target, &root.href)?;
    let mut seen_remote_identities = HashSet::from([root_identity]);
    let mut pending = VecDeque::from([(root.clone(), PathBuf::new(), 0usize)]);

    while let Some((collection, relative_parent, depth)) = pending.pop_front() {
        validate_depth(depth)?;
        if cancel.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "tree download cancelled",
            ));
        }
        pause.checkpoint().await;
        if cancel.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "tree download cancelled",
            ));
        }
        let children = provider.list_collection_exact(&collection).await?;
        let mut local_names = HashSet::new();
        for child in children {
            accept_descendant(&mut descendants)?;
            accept_local_name(&mut local_names, &child.entry.name)?;
            let relative = relative_parent.join(&child.entry.name);
            let child_depth = descendant_depth(depth)?;
            match child.identity {
                EntryIdentity::WebDavCollection(source) => {
                    let identity =
                        provider.canonical_exact_href_identity(&source.target, &source.href)?;
                    accept_unique_identity(
                        &mut seen_remote_identities,
                        identity,
                        "duplicate exact WebDAV remote identity",
                    )?;
                    directories.push(TreeDirectory {
                        relative: relative.clone(),
                        source: source.clone(),
                    });
                    pending.push_back((source, relative, child_depth));
                }
                EntryIdentity::WebDavObject(source) => {
                    let identity =
                        provider.canonical_exact_href_identity(&source.target, &source.href)?;
                    accept_unique_identity(
                        &mut seen_remote_identities,
                        identity,
                        "duplicate exact WebDAV remote identity",
                    )?;
                    files.push(TreeFile {
                        relative,
                        source,
                        advertised_size: child.entry.size,
                    });
                }
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "exact collection listing returned a non-WebDAV identity",
                    ));
                }
            }
        }
    }
    let total_bytes = checked_total_bytes(&files);
    Ok(WebDavTreeManifest {
        directories,
        files,
        descendant_count: descendants,
        total_bytes,
    })
}

async fn cleanup_owned_root(root: &Path, original: io::Error) -> io::Error {
    match tokio::fs::remove_dir_all(root).await {
        Ok(()) => original,
        Err(cleanup) => io::Error::other(TreeCleanupFailure {
            root: root.to_path_buf(),
            original: original.to_string(),
            cleanup: cleanup.to_string(),
        }),
    }
}

/// Materialize one manifest into one newly-created Local root. Item semantics:
/// selected root + every manifest descendant; full completion only after all
/// directories and files succeed.
pub(crate) async fn download_tree(
    provider: &WebDavProvider,
    spec: &WebDavTransferSpec,
    cancel: Arc<AtomicBool>,
    pause: crate::transfer_queue::PauseGate,
    on_progress: &mut impl FnMut(TypedTransferProgress),
) -> io::Result<usize> {
    let (source, final_root) = match spec {
        WebDavTransferSpec::DownloadTree {
            source,
            local_destination,
        } => (source, local_destination),
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "download_tree requires WebDavTransferSpec::DownloadTree",
            ));
        }
    };

    // Manifest first: no Local mutation and no progress before it succeeds.
    let manifest = build_tree_manifest(provider, source, &cancel, &pause).await?;
    if final_root.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "refusing to merge into existing destination {}",
                final_root.display()
            ),
        ));
    }
    pause.checkpoint().await;
    if cancel.load(Ordering::Acquire) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "tree download cancelled",
        ));
    }
    tokio::fs::create_dir(final_root).await?;

    let materialize: io::Result<()> = async {
        for directory in &manifest.directories {
            pause.checkpoint().await;
            if cancel.load(Ordering::Acquire) {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "tree download cancelled",
                ));
            }
            tokio::fs::create_dir(final_root.join(&directory.relative)).await?;
        }

        let mut completed_before = 0u64;
        for file in &manifest.files {
            pause.checkpoint().await;
            if cancel.load(Ordering::Acquire) {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "tree download cancelled",
                ));
            }
            let one = WebDavTransferSpec::DownloadOne {
                source: file.source.clone(),
                local_destination: final_root.join(&file.relative),
            };
            let total = manifest.total_bytes;
            let base = completed_before;
            let mut progress_overflow = false;
            let actual = download_one(
                provider,
                &one,
                WebDavOverwritePolicy::Forbid,
                cancel.clone(),
                pause.clone(),
                &mut |progress| {
                    if let TypedTransferProgress::Bytes { completed, .. } = progress {
                        if let Some(progress) = tree_progress(base, completed, total) {
                            on_progress(progress);
                        } else {
                            progress_overflow = true;
                        }
                    }
                },
            )
            .await?;
            if progress_overflow {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "downloaded byte progress overflow",
                ));
            }
            completed_before = completed_before.checked_add(actual).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "downloaded byte count overflow")
            })?;
            // Truthful terminal sample even when a zero-byte GET produced no
            // body chunks/callbacks.
            on_progress(TypedTransferProgress::Bytes {
                completed: completed_before,
                total: manifest.total_bytes,
            });
        }
        if manifest.files.is_empty() {
            on_progress(TypedTransferProgress::Bytes {
                completed: 0,
                total: Some(0),
            });
        }
        Ok(())
    }
    .await;

    if let Err(error) = materialize {
        return Err(cleanup_owned_root(final_root, error).await);
    }
    Ok(1 + manifest.descendant_count)
}

fn tree_progress(
    base: u64,
    current_file_completed: u64,
    total: Option<u64>,
) -> Option<TypedTransferProgress> {
    base.checked_add(current_file_completed)
        .map(|completed| TypedTransferProgress::Bytes { completed, total })
}

fn stream_progress(completed: u64, total: Option<u64>) -> TypedTransferProgress {
    TypedTransferProgress::Bytes { completed, total }
}

#[cfg(test)]
mod progress_tests {
    use super::*;

    #[test]
    fn tree_cycle_duplicate_and_name_guards_r9_r11() {
        let mut identities = HashSet::new();
        accept_unique_identity(&mut identities, "http://x/dav/root".into(), "cycle").unwrap();
        assert!(
            accept_unique_identity(&mut identities, "http://x/dav/root".into(), "cycle").is_err()
        );
        let mut names = HashSet::new();
        accept_local_name(&mut names, "safe name").unwrap();
        assert!(accept_local_name(&mut names, "safe name").is_err());
        for unsafe_name in ["..", ".", "a/b", "/absolute", ""] {
            assert!(accept_local_name(&mut HashSet::new(), unsafe_name).is_err());
        }
    }

    #[test]
    fn tree_budget_boundaries_r12_r13() {
        let mut count = 0usize;
        for _ in 0..MAX_WEBDAV_TREE_DESCENDANTS {
            accept_descendant(&mut count).expect("exactly 50000 accepted");
        }
        assert_eq!(count, 50_000);
        assert!(accept_descendant(&mut count).is_err());
        assert!(validate_depth(128).is_ok());
        assert!(validate_depth(129).is_err());
        assert_eq!(descendant_depth(127).unwrap(), 128);
        assert!(descendant_depth(128).is_err());
    }

    #[test]
    fn tree_total_and_progress_r14_r16() {
        let object = |name: &str, size| TreeFile {
            relative: PathBuf::from(name),
            source: WebDavObjectRef {
                target: "t".into(),
                href: format!("/dav/{name}"),
            },
            advertised_size: size,
        };
        assert_eq!(
            checked_total_bytes(&[object("a", Some(3)), object("b", Some(5))]),
            Some(8)
        );
        assert_eq!(
            checked_total_bytes(&[object("a", Some(3)), object("b", None)]),
            None
        );
        assert_eq!(
            checked_total_bytes(&[object("a", Some(u64::MAX)), object("b", Some(1))]),
            None
        );
        assert_eq!(
            tree_progress(3, 2, Some(8)),
            Some(TypedTransferProgress::Bytes {
                completed: 5,
                total: Some(8)
            })
        );
        assert_eq!(
            tree_progress(5, 1, Some(8)),
            Some(TypedTransferProgress::Bytes {
                completed: 6,
                total: Some(8)
            })
        );
        assert_eq!(tree_progress(u64::MAX, 1, None), None);
    }

    #[tokio::test]
    async fn cleanup_owned_root_truth_r18_r20() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("owned");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("partial"), b"x").unwrap();
        let original = io::Error::other("download failed");
        let returned = cleanup_owned_root(&root, original).await;
        assert_eq!(returned.to_string(), "download failed");
        assert!(!root.exists(), "attempt-owned root removed");

        let cancelled_root = temp.path().join("cancelled-owned");
        std::fs::create_dir(&cancelled_root).unwrap();
        let cancelled = cleanup_owned_root(
            &cancelled_root,
            io::Error::new(io::ErrorKind::Interrupted, "cancelled"),
        )
        .await;
        assert_eq!(cancelled.kind(), io::ErrorKind::Interrupted);
        assert!(
            !cancelled_root.exists(),
            "cancel cleanup removed owned root"
        );

        let nonexistent = temp.path().join("not-owned");
        let recovery = cleanup_owned_root(&nonexistent, io::Error::other("cancelled")).await;
        assert!(
            recovery
                .get_ref()
                .is_some_and(|e| e.downcast_ref::<TreeCleanupFailure>().is_some())
        );
        assert!(
            recovery
                .to_string()
                .contains(nonexistent.to_string_lossy().as_ref())
        );
    }

    #[test]
    fn missing_content_length_stays_unknown() {
        assert_eq!(
            stream_progress(7, None),
            TypedTransferProgress::Bytes {
                completed: 7,
                total: None,
            }
        );
    }

    #[test]
    fn streamed_terminal_bytes_are_exact() {
        assert_eq!(
            stream_progress(19, Some(19)),
            TypedTransferProgress::Bytes {
                completed: 19,
                total: Some(19),
            }
        );
    }
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
        assert_eq!(
            webdav_download_local_name(&obj, "unicodé spáces.txt").unwrap(),
            "unicodé spáces.txt"
        );
        for bad in [
            "../",
            "/",
            "",
            ".",
            "..",
            "a/b",
            "a\\b",
            "\\absolute",
            "C:escape",
            "C:\\escape",
            "nul\0name",
        ] {
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
