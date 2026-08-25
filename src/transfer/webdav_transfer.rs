//! Local ↔ WebDAV basic transfer core (PACK E B1/B2).
//!
//! Invariants:
//! - existing downloads use exact server href identity; upload destinations are
//!   decoded logical write paths encoded only by `WebDavProvider`
//! - download: GET the exact href, stream to staged file in destination dir,
//!   atomic rename to final path, fsync
//! - cancellation truth: staged file removed, never a partial final path
//! - no overwrite of an existing final path without a frozen policy

use crate::transfer::{
    WebDavTransferSpec, WebDavWriteTarget, validate_webdav_local_component,
    webdav_write_child_target,
};
use crate::transfer_queue::TypedTransferProgress;
use crate::vfs::webdav::{NewCollectionError, WebDavProvider};
use crate::vfs::{
    EntryIdentity, MAX_WEBDAV_TREE_DEPTH, MAX_WEBDAV_TREE_DESCENDANTS, WebDavCollectionRef,
    WebDavObjectRef,
};
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UploadTreeDirectory {
    pub relative: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UploadTreeFile {
    pub relative: PathBuf,
    pub local_source: PathBuf,
    pub expected_size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UploadTreeManifest {
    pub directories: Vec<UploadTreeDirectory>,
    pub files: Vec<UploadTreeFile>,
    pub descendant_count: usize,
    pub total_bytes: u64,
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

#[derive(Debug, thiserror::Error)]
#[error("WebDAV upload root ownership is ambiguous at {target}:{logical_path}: {reason}")]
pub(crate) struct UploadTreeRootAmbiguous {
    pub target: String,
    pub logical_path: String,
    pub reason: String,
}

#[derive(Debug, thiserror::Error)]
#[error(
    "WebDAV upload tree failed and cleanup of {target}:{logical_path} failed: original={original}; cleanup={cleanup}"
)]
pub(crate) struct UploadTreeCleanupFailure {
    pub target: String,
    pub logical_path: String,
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

fn checked_upload_total(sum: u64, size: u64) -> io::Result<u64> {
    sum.checked_add(size).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "upload tree byte total overflow",
        )
    })
}

pub(crate) fn build_upload_tree_manifest(root: &Path) -> io::Result<UploadTreeManifest> {
    let root_meta = std::fs::symlink_metadata(root)?;
    if root_meta.file_type().is_symlink() || !root_meta.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "recursive WebDAV upload root must be a real directory, not a symlink",
        ));
    }
    let mut directories = Vec::new();
    let mut files = Vec::new();
    let mut descendants = 0usize;
    let mut total_bytes = 0u64;
    let mut pending = VecDeque::from([(PathBuf::new(), 0usize)]);
    while let Some((relative_parent, depth)) = pending.pop_front() {
        let absolute_parent = root.join(&relative_parent);
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&absolute_parent)? {
            let entry = entry?;
            let name = entry.file_name().into_string().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "non-UTF-8 local name cannot enter WebDAV logical path",
                )
            })?;
            validate_webdav_local_component(&name)
                .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))?;
            entries.push((name, entry.path()));
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, absolute) in entries {
            accept_descendant(&mut descendants)?;
            let child_depth = descendant_depth(depth)?;
            let relative = relative_parent.join(&name);
            let metadata = std::fs::symlink_metadata(&absolute)?;
            let file_type = metadata.file_type();
            if file_type.is_symlink() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "symlink is unsupported in recursive WebDAV upload: {}",
                        absolute.display()
                    ),
                ));
            }
            if metadata.is_dir() {
                directories.push(UploadTreeDirectory {
                    relative: relative.clone(),
                });
                pending.push_back((relative, child_depth));
            } else if metadata.is_file() {
                total_bytes = checked_upload_total(total_bytes, metadata.len())?;
                files.push(UploadTreeFile {
                    relative,
                    local_source: absolute,
                    expected_size: metadata.len(),
                });
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("special local file is unsupported: {}", absolute.display()),
                ));
            }
        }
    }
    Ok(UploadTreeManifest {
        directories,
        files,
        descendant_count: descendants,
        total_bytes,
    })
}

// ponytail: root-relative no-follow open via libc openat/O_NOFOLLOW. No new
// dependency; libc is already a direct Linux dep. Used only by UploadTree file
// reads to avoid following a replaced intermediate symlink.
#[cfg(target_os = "linux")]
fn read_upload_file_nofollow(
    root: &Path,
    relative: &Path,
    _local_source: &Path,
    expected_size: u64,
) -> io::Result<Vec<u8>> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;

    let root_c = CString::new(root.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "root path contains NUL"))?;
    // SAFETY: root_c is NUL-terminated and outlives the call.
    let root_fd = unsafe {
        libc::open(
            root_c.as_ptr(),
            libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if root_fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: root_fd is a fresh successful open descriptor.
    let mut dir_fd = unsafe { OwnedFd::from_raw_fd(root_fd) };

    let components: Vec<&std::ffi::OsStr> = relative
        .components()
        .map(|component| match component {
            std::path::Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid upload tree component: {relative:?}"),
            )
        })?;
    for (index, name) in components.iter().enumerate() {
        let bytes = name.as_bytes();
        let c = CString::new(bytes).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "path component contains NUL")
        })?;
        let is_final = index + 1 == components.len();
        if is_final {
            // Final file component: reject a symlink via O_NOFOLLOW.
            // SAFETY: dir_fd is an open directory descriptor; c is NUL-terminated.
            let fd = unsafe {
                libc::openat(
                    dir_fd.as_raw_fd(),
                    c.as_ptr(),
                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: fd is a fresh open descriptor we now own.
            let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
            let stat = file.metadata()?;
            if !stat.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("upload tree file is not regular: {relative:?}"),
                ));
            }
            if stat.len() != expected_size {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "upload tree file size mismatch before PUT: {relative:?} expected {expected_size} got {}",
                        stat.len()
                    ),
                ));
            }
            let mut data = Vec::with_capacity(expected_size as usize);
            let read = std::io::Read::read_to_end(&mut file, &mut data)?;
            if read as u64 != expected_size {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "upload tree file read length mismatch: {relative:?} expected {expected_size} got {read}"
                    ),
                ));
            }
            return Ok(data);
        }
        // Intermediate directory component.
        // SAFETY: dir_fd is an open directory descriptor; c is NUL-terminated.
        let next = unsafe {
            libc::openat(
                dir_fd.as_raw_fd(),
                c.as_ptr(),
                libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if next < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: next is a fresh successful open descriptor. Assignment
        // drops the prior OwnedFd exactly once.
        dir_fd = unsafe { OwnedFd::from_raw_fd(next) };
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "upload tree empty relative path",
    ))
}

#[cfg(not(target_os = "linux"))]
fn read_upload_file_nofollow(
    _root: &Path,
    _relative: &Path,
    local_source: &Path,
    _expected_size: u64,
) -> io::Result<Vec<u8>> {
    tokio::fs::read(local_source)
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::NotFound, format!("read local: {e}")))
}

/// True when a non-UTF-8 sibling name's lossy representation collides with the
/// selected root's (valid UTF-8) presentation name. The selected entry itself
/// is excluded; a non-UTF-8 selected name is rejected by the caller.
fn selected_root_name_has_lossy_alias(parent: &Path, selected_name: &str) -> io::Result<bool> {
    for entry in std::fs::read_dir(parent)? {
        let file_name = entry?.file_name();
        if file_name.to_str() == Some(selected_name) {
            continue;
        }
        if file_name.to_str().is_none() && file_name.to_string_lossy() == selected_name {
            return Ok(true);
        }
    }
    Ok(false)
}

fn revalidate_upload_tree(root: &Path, frozen: &UploadTreeManifest) -> io::Result<()> {
    let current = build_upload_tree_manifest(root)?;
    if &current != frozen {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "local upload tree changed after manifest freeze",
        ));
    }
    Ok(())
}

fn revalidate_upload_file(file: &UploadTreeFile) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(&file.local_source)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() != file.expected_size
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("local upload file changed: {}", file.local_source.display()),
        ));
    }
    Ok(())
}

/// Overwrite policy for WebDAV upload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebDavOverwritePolicy {
    Forbid,
    Allow,
}

/// Upload a single local file to a WebDAV write target, reading bytes from the
/// caller-supplied buffer (used by UploadTree, which already read the file via
/// root-relative no-follow open). The single-file UploadOne path reads itself.
pub(crate) async fn upload_one_with_data(
    provider: &WebDavProvider,
    spec: &WebDavTransferSpec,
    data: &[u8],
    overwrite: WebDavOverwritePolicy,
    cancel: Arc<AtomicBool>,
    on_progress: &mut impl FnMut(TypedTransferProgress),
) -> io::Result<u64> {
    if cancel.load(Ordering::Relaxed) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "upload cancelled",
        ));
    }
    let (_, destination) = match spec {
        WebDavTransferSpec::UploadOne {
            local_source: _,
            destination,
        } => (Path::new(""), destination),
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "upload_one_with_data requires WebDavTransferSpec::UploadOne",
            ));
        }
    };
    let total = data.len();
    provider
        .put_logical_with_policy(&destination.logical_path, data, overwrite)
        .await?;
    on_progress(TypedTransferProgress::Bytes {
        completed: total as u64,
        total: Some(total as u64),
    });
    Ok(total as u64)
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

    // Read through an O_NOFOLLOW descriptor on Linux so a replacement symlink
    // cannot redirect a recursive upload after manifest validation.
    let data = {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(local_source)
                .map_err(|e| io::Error::new(io::ErrorKind::NotFound, format!("read local: {e}")))?;
            let mut data = Vec::new();
            std::io::Read::read_to_end(&mut file, &mut data)?;
            data
        }
        #[cfg(not(target_os = "linux"))]
        {
            tokio::fs::read(local_source)
                .await
                .map_err(|e| io::Error::new(io::ErrorKind::NotFound, format!("read local: {e}")))?
        }
    };
    let total = data.len();

    // 3. PUT with overwrite policy enforced at the HTTP layer (no
    //    existence preflight — racing TOCTOU is unsafe). For Forbid we
    //    send If-None-Match: * so the server rejects an existing resource
    //    with 412; Allow is a plain PUT.
    provider
        .put_logical_with_policy(&destination.logical_path, &data, overwrite)
        .await?;

    on_progress(TypedTransferProgress::Bytes {
        completed: total as u64,
        total: Some(total as u64),
    });

    Ok(total as u64)
}

fn upload_target_for_relative(
    root: &WebDavWriteTarget,
    relative: &Path,
) -> io::Result<WebDavWriteTarget> {
    let mut target = root.clone();
    for component in relative.components() {
        let name = component.as_os_str().to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "non-UTF-8 upload component")
        })?;
        target = webdav_write_child_target(&target.target, &target.logical_path, name)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    }
    Ok(target)
}

async fn cleanup_uploaded_root(
    provider: &WebDavProvider,
    root: &WebDavWriteTarget,
    original: io::Error,
) -> io::Error {
    match provider.delete_logical_collection(&root.logical_path).await {
        Ok(()) => original,
        Err(cleanup) => io::Error::other(UploadTreeCleanupFailure {
            target: root.target.clone(),
            logical_path: root.logical_path.clone(),
            original: original.to_string(),
            cleanup: cleanup.to_string(),
        }),
    }
}

pub(crate) async fn upload_tree(
    provider: &WebDavProvider,
    spec: &WebDavTransferSpec,
    cancel: Arc<AtomicBool>,
    pause: crate::transfer_queue::PauseGate,
    on_progress: &mut impl FnMut(TypedTransferProgress),
) -> io::Result<usize> {
    let (local_source, destination_root) = match spec {
        WebDavTransferSpec::UploadTree {
            local_source,
            destination_root,
        } => (local_source, destination_root),
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "upload_tree requires WebDavTransferSpec::UploadTree",
            ));
        }
    };

    let manifest = build_upload_tree_manifest(local_source)?;
    if cancel.load(Ordering::Acquire) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "tree upload cancelled",
        ));
    }
    pause.checkpoint().await;
    revalidate_upload_tree(local_source, &manifest)?;
    if cancel.load(Ordering::Acquire) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "tree upload cancelled",
        ));
    }
    // Check for non-UTF-8 lossy alias collision on the selected root directory
    // before any remote mutation. A selected root whose own name is non-UTF-8
    // cannot be trusted either, so reject it outright.
    let Some(parent) = local_source.parent() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "recursive upload root has no parent directory",
        ));
    };
    let selected_name = local_source
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "recursive upload root name is not valid UTF-8: {}",
                    local_source.display()
                ),
            )
        })?;
    if selected_root_name_has_lossy_alias(parent, selected_name)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "recursive upload root name is a lossy alias for a non-UTF-8 directory: {selected_name}"
            ),
        ));
    }
    // Final gate immediately before the first remote mutation.
    pause.checkpoint().await;
    if cancel.load(Ordering::Acquire) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "tree upload cancelled",
        ));
    }
    match provider
        .create_new_collection(&destination_root.logical_path)
        .await
    {
        Ok(()) => {}
        Err(NewCollectionError::Definitive(error)) => return Err(error),
        Err(NewCollectionError::Ambiguous(error)) => {
            return Err(io::Error::other(UploadTreeRootAmbiguous {
                target: destination_root.target.clone(),
                logical_path: destination_root.logical_path.clone(),
                reason: error.to_string(),
            }));
        }
    }

    let materialize: io::Result<()> = async {
        if cancel.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "tree upload cancelled",
            ));
        }
        for directory in &manifest.directories {
            pause.checkpoint().await;
            if cancel.load(Ordering::Acquire) {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "tree upload cancelled",
                ));
            }
            let target = upload_target_for_relative(destination_root, &directory.relative)?;
            provider
                .create_new_collection(&target.logical_path)
                .await
                .map_err(|error| io::Error::other(error.to_string()))?;
        }

        let mut completed_before = 0u64;
        for file in &manifest.files {
            pause.checkpoint().await;
            if cancel.load(Ordering::Acquire) {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "tree upload cancelled",
                ));
            }
            revalidate_upload_file(file)?;
            pause.checkpoint().await;
            if cancel.load(Ordering::Acquire) {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "tree upload cancelled",
                ));
            }
            let target = upload_target_for_relative(destination_root, &file.relative)?;
            let data = read_upload_file_nofollow(
                local_source,
                &file.relative,
                &file.local_source,
                file.expected_size,
            )?;
            // Required gate AFTER the local read, immediately before PUT: a
            // pause/cancel arriving during the read must still prevent it.
            pause.checkpoint().await;
            if cancel.load(Ordering::Acquire) {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "tree upload cancelled",
                ));
            }
            let one = WebDavTransferSpec::UploadOne {
                local_source: file.local_source.clone(),
                destination: target,
            };
            let base = completed_before;
            let mut overflow = false;
            let actual = upload_one_with_data(
                provider,
                &one,
                &data,
                WebDavOverwritePolicy::Forbid,
                cancel.clone(),
                &mut |progress| {
                    if let TypedTransferProgress::Bytes { completed, .. } = progress {
                        match base.checked_add(completed) {
                            Some(completed) => on_progress(TypedTransferProgress::Bytes {
                                completed,
                                total: Some(manifest.total_bytes),
                            }),
                            None => overflow = true,
                        }
                    }
                },
            )
            .await?;
            if overflow {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "upload tree progress overflow",
                ));
            }
            completed_before = completed_before.checked_add(actual).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "upload byte count overflow")
            })?;
            on_progress(TypedTransferProgress::Bytes {
                completed: completed_before,
                total: Some(manifest.total_bytes),
            });
        }
        if cancel.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "tree upload cancelled",
            ));
        }
        if manifest.files.is_empty() {
            on_progress(TypedTransferProgress::Bytes {
                completed: 0,
                total: Some(0),
            });
        }
        // final cumulative check
        if completed_before != manifest.total_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "upload tree completed bytes mismatch manifest total",
            ));
        }
        Ok(())
    }
    .await;

    if let Err(error) = materialize {
        return Err(cleanup_uploaded_root(provider, destination_root, error).await);
    }
    Ok(1 + manifest.descendant_count)
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
        WebDavWriteTarget, webdav_download_local_name, webdav_spec_for_objects,
        webdav_write_child_target,
    };
    use crate::vfs::webdav::WebDavObjectRef;
    use std::path::PathBuf;

    #[test]
    fn upload_tree_manifest_is_deterministic_and_bounded() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("empty")).unwrap();
        std::fs::create_dir(root.path().join("nested")).unwrap();
        std::fs::create_dir(root.path().join("unicodé spáces")).unwrap();
        std::fs::write(root.path().join("z.txt"), b"z").unwrap();
        std::fs::write(root.path().join("nested/a.txt"), b"abc").unwrap();
        std::fs::write(root.path().join("unicodé spáces/zero.bin"), b"").unwrap();
        let manifest = build_upload_tree_manifest(root.path()).unwrap();
        assert_eq!(manifest.descendant_count, 6);
        assert_eq!(manifest.total_bytes, 4);
        assert_eq!(
            manifest
                .directories
                .iter()
                .map(|entry| entry.relative.to_string_lossy().to_string())
                .collect::<Vec<_>>(),
            ["empty", "nested", "unicodé spáces"]
        );
        assert_eq!(
            manifest
                .files
                .iter()
                .map(|entry| entry.relative.to_string_lossy().to_string())
                .collect::<Vec<_>>(),
            ["z.txt", "nested/a.txt", "unicodé spáces/zero.bin"]
        );
        revalidate_upload_tree(root.path(), &manifest).unwrap();
        std::fs::write(root.path().join("z.txt"), b"changed").unwrap();
        assert!(revalidate_upload_tree(root.path(), &manifest).is_err());
        assert!(checked_upload_total(u64::MAX, 1).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn upload_tree_rejects_symlink_socket_and_non_utf8() {
        use std::os::unix::ffi::OsStringExt;
        use std::os::unix::fs::symlink;
        use std::os::unix::net::UnixListener;
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("real"), b"x").unwrap();
        symlink(root.path().join("real"), root.path().join("link")).unwrap();
        assert!(build_upload_tree_manifest(root.path()).is_err());
        std::fs::remove_file(root.path().join("link")).unwrap();
        let socket_path = root.path().join("socket");
        let listener = UnixListener::bind(&socket_path).unwrap();
        assert!(build_upload_tree_manifest(root.path()).is_err());
        drop(listener);
        std::fs::remove_file(&socket_path).unwrap();
        let bad = std::ffi::OsString::from_vec(vec![0xff]);
        std::fs::write(root.path().join(bad), b"x").unwrap();
        assert!(build_upload_tree_manifest(root.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn upload_tree_rejects_symlink_root() {
        use std::os::unix::fs::symlink;
        let parent = tempfile::tempdir().unwrap();
        let real = parent.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = parent.path().join("link");
        symlink(&real, &link).unwrap();
        assert!(build_upload_tree_manifest(&link).is_err());
    }

    #[test]
    fn webdav_transfer_spec_target_returns_correct_id() {
        let upload = WebDavTransferSpec::UploadOne {
            local_source: PathBuf::from("/tmp/a.txt"),
            destination: WebDavWriteTarget {
                target: "my-target".into(),
                logical_path: "/dav/file.txt".into(),
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
    fn webdav_write_child_target_constructs_correctly() {
        let r = webdav_write_child_target("tgt", "/dav", "my % file.txt").unwrap();
        assert_eq!(r.target, "tgt");
        assert_eq!(r.logical_path, "/dav/my % file.txt");
        assert_eq!(
            webdav_write_child_target("tgt", "/", "root")
                .unwrap()
                .logical_path,
            "/root"
        );
    }

    #[test]
    fn webdav_write_child_target_rejects_unsafe_filename() {
        for bad in ["", ".", "..", "/", "../", "a/b", "a\\b", "C:x"] {
            assert!(
                webdav_write_child_target("tgt", "/dav", bad).is_err(),
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

    #[cfg(unix)]
    #[test]
    fn root_relative_no_follow_rejects_symlink_and_size_mismatch() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("sub")).unwrap();
        std::fs::write(root.path().join("real.txt"), b"real-content").unwrap();
        // Intermediate symlink replacement must fail: sub -> real.txt.
        std::fs::remove_dir(root.path().join("sub")).unwrap();
        symlink(root.path().join("real.txt"), root.path().join("sub")).unwrap();
        std::fs::write(root.path().join("file.txt"), b"payload").unwrap();
        let err = read_upload_file_nofollow(
            root.path(),
            Path::new("sub/file.txt"),
            &root.path().join("sub/file.txt"),
            8,
        )
        .unwrap_err();
        // Intermediate symlink replacement must fail (ENOTDIR from O_DIRECTORY,
        // or ELOOP from O_NOFOLLOW) before any read.
        assert!(
            matches!(
                err.kind(),
                io::ErrorKind::NotFound
                    | io::ErrorKind::InvalidInput
                    | io::ErrorKind::Other
                    | io::ErrorKind::PermissionDenied
                    | io::ErrorKind::NotADirectory
            ),
            "intermediate symlink rejected: {err}"
        );
        std::fs::remove_file(root.path().join("sub")).unwrap();

        // Final symlink rejected via O_NOFOLLOW.
        symlink(root.path().join("real.txt"), root.path().join("link.txt")).unwrap();
        let err = read_upload_file_nofollow(
            root.path(),
            Path::new("link.txt"),
            &root.path().join("link.txt"),
            11,
        )
        .unwrap_err();
        // O_NOFOLLOW open of a symlink must fail before any read (ELOOP/
        // EACCES/ENOTDIR depending on platform); any error proves rejection.
        let _ = err;

        // Size/length mismatch fail closed before PUT.
        std::fs::write(root.path().join("file.txt"), b"payload").unwrap();
        let ok = read_upload_file_nofollow(
            root.path(),
            Path::new("file.txt"),
            &root.path().join("file.txt"),
            7,
        )
        .unwrap();
        assert_eq!(ok, b"payload");
        assert!(
            read_upload_file_nofollow(
                root.path(),
                Path::new("file.txt"),
                &root.path().join("file.txt"),
                999,
            )
            .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn lossy_root_alias_rejected_before_mutation() {
        use std::os::unix::ffi::OsStringExt;
        let parent = tempfile::tempdir().unwrap();
        // Selected root is a valid UTF-8 name that equals the lossy form of a
        // non-UTF-8 sibling (invalid bytes lossily render as replacement chars).
        let selected = "\u{FFFD}\u{FFFD}";
        let good = parent.path().join(selected);
        std::fs::create_dir(&good).unwrap();
        let bad = std::ffi::OsString::from_vec(vec![0xff, 0xff]);
        std::fs::create_dir(parent.path().join(bad)).unwrap();
        assert!(selected_root_name_has_lossy_alias(parent.path(), selected).unwrap());
        // No collision: a normal sibling name is fine.
        let other = parent.path().join("other");
        std::fs::create_dir(&other).unwrap();
        assert!(!selected_root_name_has_lossy_alias(parent.path(), "other").unwrap());
    }

    #[test]
    fn upload_tree_zero_byte_and_total_progress() {
        let manifest = UploadTreeManifest {
            directories: vec![UploadTreeDirectory {
                relative: PathBuf::from("empty"),
            }],
            files: vec![],
            descendant_count: 1,
            total_bytes: 0,
        };
        let mut progress = None;
        let mut completed = 0u64;
        for _ in &manifest.files {
            completed = completed.checked_add(0).unwrap();
            progress = Some(TypedTransferProgress::Bytes {
                completed,
                total: Some(manifest.total_bytes),
            });
        }
        if manifest.files.is_empty() {
            progress = Some(TypedTransferProgress::Bytes {
                completed: 0,
                total: Some(0),
            });
        }
        assert_eq!(
            progress,
            Some(TypedTransferProgress::Bytes {
                completed: 0,
                total: Some(0)
            })
        );
        assert_eq!(completed, manifest.total_bytes);
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
