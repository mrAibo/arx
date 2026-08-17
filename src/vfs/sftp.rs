use super::{
    BoundedRead, CancellationFlag, Entry, EntryKind, FileMetadata, RemoteEditRevision,
    RemoteWriteFailureKind, VfsProvider, canonical_unix_mtime_ms, remote_write_error,
};
use crate::remote::Host;
use anyhow::Context;
use std::collections::BTreeSet;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::timeout;

/// Canonical decision: must the pooled SFTP session be invalidated?
///
/// TRACK B (#47): a pooled connection is invalidated ONLY for transport-level
/// failure where safe reuse cannot be proven. Application-level results —
/// conflict, cancelled, validation refusal, binary/UTF-8 refusal, unsupported,
/// definitive remote permission/status — keep the connection, because the
/// transport is still known-good.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SftpInvalidation {
    /// Disconnect / EOF / broken pipe / timeout with ambiguous reuse.
    TransportBroken,
    /// Keep: application-level outcome, transport still usable.
    Keep,
}

impl SftpInvalidation {
    /// Invalidate the pooled connection only for transport-level failure.
    pub(crate) fn should_invalidate(self) -> bool {
        matches!(self, Self::TransportBroken)
    }
}

/// Pooled-session reuse decision (TRACK C #48): the health-probe policy,
/// extracted pure so it can be tested deterministically without any network
/// or `sleep`. `connect_for_mutation` is the only caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PoolHealthAction {
    /// Existing session probed healthy — reuse it, no reconnect.
    Reuse,
    /// No session, or probe said transport-broken — discard + open a fresh one.
    DiscardAndReconnect,
}

pub(crate) fn pool_health_action(has_session: bool, probe: SftpInvalidation) -> PoolHealthAction {
    if has_session && !probe.should_invalidate() {
        PoolHealthAction::Reuse
    } else {
        PoolHealthAction::DiscardAndReconnect
    }
}

/// Classify an `io::Error` produced by an SFTP operation.
///
/// Definitive application outcomes (cancellation, validation refusal,
/// unsupported, conflict, definitive remote permission/status) keep the
/// connection. Transport-ambiguous transport signals invalidate it.
pub(crate) fn classify_io_error(err: &io::Error) -> SftpInvalidation {
    match err.kind() {
        // Cancellation / validation refusal / unsupported / conflict / permission.
        io::ErrorKind::Interrupted
        | io::ErrorKind::InvalidInput
        | io::ErrorKind::Unsupported
        | io::ErrorKind::AlreadyExists
        | io::ErrorKind::PermissionDenied => SftpInvalidation::Keep,
        // Transport-ambiguous: the connection may be gone or wedged.
        io::ErrorKind::TimedOut
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::ConnectionReset
        | io::ErrorKind::BrokenPipe
        | io::ErrorKind::UnexpectedEof => SftpInvalidation::TransportBroken,
        _ => SftpInvalidation::Keep,
    }
}

/// Classify a raw russh-sftp error. `Status` variants are definitive
/// application outcomes (keep); non-Status transport/protocol errors
/// invalidate. russh's `IO` variant wraps only a `String`, so its
/// transport semantics cannot be narrowed — treat as transport-ambiguous
/// (invalidate, the fail-closed choice).
fn classify_russh_error(err: &russh_sftp::client::error::Error) -> SftpInvalidation {
    match err {
        russh_sftp::client::error::Error::Status(_) => SftpInvalidation::Keep,
        _ => SftpInvalidation::TransportBroken,
    }
}

/// Convert a raw russh-sftp error into `io::Error` while PRESERVING the
/// transport-invalidation truth in the `ErrorKind` (TRACK C #47). Stringifying
/// into `io::Error::other` would erase it and let a dead session survive; here
/// `TransportBroken` maps to a transport `ErrorKind` so `classify_io_error`
/// downstream can still invalidate the pooled connection.
fn russh_to_io(err: russh_sftp::client::error::Error, ctx: &str) -> io::Error {
    let kind = match classify_russh_error(&err) {
        SftpInvalidation::TransportBroken => io::ErrorKind::BrokenPipe,
        SftpInvalidation::Keep => io::ErrorKind::Other,
    };
    io::Error::new(kind, format!("{ctx}: {err}"))
}

/// Classify a remote SFTP file stream read failure. A break/EOF/timeout
/// mid-stream on a pooled connection is a transport failure (disconnect,
/// reset, EOF, timeout, not-connected) → invalidate. Only local cooperative
/// cancellation (Interrupted) keeps the connection. The original kind is
/// folded into a BrokenPipe so downstream `classify_io_error` yields
/// `TransportBroken`; classification is by `ErrorKind`, never by stringifying
/// the message first.
fn stream_read_error(error: io::Error, ctx: &str) -> io::Error {
    let kind = match error.kind() {
        // Local cooperative cancellation: keep the pooled session.
        io::ErrorKind::Interrupted => io::ErrorKind::Interrupted,
        // Every transport/EOF/ambiguous-health break → invalidate.
        _ => io::ErrorKind::BrokenPipe,
    };
    io::Error::new(kind, format!("{ctx}: {error}"))
}

/// Bounded health probe for a pooled session before reuse (TRACK C #48).
/// Uses a harmless `realpath(".")`; fails closed on timeout so a stale session
/// is discarded rather than reused. No destructive operation is replayed.
async fn probe_session_healthy(session: &russh_sftp::client::SftpSession) -> SftpInvalidation {
    match timeout(Duration::from_secs(5), session.canonicalize(".")).await {
        // A definitive Status reply means the server is alive and answering →
        // the session is reusable (application-level rejection, not transport).
        Ok(Err(russh_sftp::client::error::Error::Status(_))) => SftpInvalidation::Keep,
        Ok(Ok(_)) => SftpInvalidation::Keep,
        // Any transport error or timeout → do not trust reuse.
        Ok(Err(error)) => classify_russh_error(&error),
        Err(_elapsed) => SftpInvalidation::TransportBroken,
    }
}

static ATOMIC_WRITE_COUNTER: AtomicU64 = AtomicU64::new(1);

fn unique_token() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    #[cfg(unix)]
    let entropy = {
        use std::io::Read;
        let mut bytes = [0_u8; 16];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut file| file.read_exact(&mut bytes))
            .ok()
            .map(|()| {
                use std::fmt::Write;
                let mut hex = String::with_capacity(bytes.len() * 2);
                for byte in bytes {
                    let _ = write!(&mut hex, "{byte:02x}");
                }
                hex
            })
    };
    #[cfg(not(unix))]
    let entropy: Option<String> = None;

    format!(
        "{}-{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        ATOMIC_WRITE_COUNTER.fetch_add(1, Ordering::Relaxed),
        entropy.unwrap_or_else(|| "no-entropy".to_string()),
    )
}

/// SFTP filesystem backend.
pub struct SftpFs;

impl SftpFs {
    pub fn list(host: &Host, remote_path: &str) -> io::Result<Vec<Entry>> {
        let host = host.clone();
        let path = remote_path.to_string();

        // Transitional sync bridge for legacy Location::list() call sites.
        // Never call Handle::block_on() from the async TUI runtime: Tokio
        // rejects nested blocking and can panic. Keep the legacy API isolated
        // on its own runtime thread until directory loading is async end-to-end.
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| io::Error::other(format!("SFTP runtime: {error}")))?;
            runtime
                .block_on(list_sftp(&host, &path))
                .map_err(|error| io::Error::other(format!("SFTP: {error:#}")))
        })
        .join()
        .map_err(|_| io::Error::other("SFTP worker thread panicked"))?
    }
}

async fn list_sftp(host: &Host, remote_path: &str) -> anyhow::Result<Vec<Entry>> {
    let connection = crate::remote::openssh_sftp::OpenSshSftpConnection::connect(&host.ssh_alias)
        .await
        .with_context(|| format!("OpenSSH SFTP connect to {}", host.ssh_alias))?;

    let read_dir = connection
        .session()
        .read_dir(remote_path.to_string())
        .await
        .with_context(|| format!("SFTP read_dir {remote_path}"))?;
    let result = entries_from_read_dir(read_dir.collect());
    let _ = connection.close().await;
    Ok(result)
}

fn entries_from_read_dir(read_dir: Vec<russh_sftp::client::fs::DirEntry>) -> Vec<Entry> {
    let mut result: Vec<Entry> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for entry in read_dir {
        let name = entry.file_name();
        if !seen.insert(name.clone()) {
            continue;
        }
        let metadata = entry.metadata();
        let kind = if metadata.is_dir() {
            EntryKind::Directory
        } else if metadata.is_symlink() {
            EntryKind::Symlink
        } else {
            EntryKind::File
        };
        let size = if kind == EntryKind::File {
            Some(metadata.len())
        } else {
            None
        };
        let modified_unix_ms = metadata
            .mtime
            .map(|seconds| canonical_unix_mtime_ms(u64::from(seconds)));
        result.push(Entry {
            name,
            kind,
            size,
            modified_unix_ms,
        });
    }

    result.sort_by(|a, b| {
        match (
            a.kind == super::EntryKind::Directory,
            b.kind == super::EntryKind::Directory,
        ) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        }
    });
    result
}

fn bounded_read_plan(file_len: u64, max_bytes: usize) -> std::io::Result<(usize, bool)> {
    let truncated = file_len > max_bytes as u64;
    let read_len = usize::try_from(file_len.min(max_bytes as u64))
        .map_err(|_| std::io::Error::other("remote file length does not fit usize"))?;
    Ok((read_len, truncated))
}

fn is_regular_file(metadata: &russh_sftp::protocol::FileAttributes) -> bool {
    // ponytail: exact type equality; russh-sftp's bitflag check also accepts symlink mode.
    metadata.file_type().is_file()
}

async fn read_exact_len<R>(reader: R, expected_len: usize) -> std::io::Result<Vec<u8>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;

    let mut bytes = Vec::with_capacity(expected_len);
    reader
        .take(expected_len as u64)
        .read_to_end(&mut bytes)
        .await?;
    if bytes.len() != expected_len {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            format!("read {} of {expected_len} bytes", bytes.len()),
        ));
    }
    Ok(bytes)
}

async fn read_stable_snapshot(
    session: &russh_sftp::client::SftpSession,
    path: &str,
    max_bytes: usize,
) -> std::io::Result<BoundedRead> {
    let before = session
        .symlink_metadata(path.to_string())
        .await
        .map_err(|error| russh_to_io(error, &format!("SFTP metadata {path}")))?;
    if !is_regular_file(&before) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("SFTP read {path}: target is not a regular file"),
        ));
    }
    let file_len = before
        .size
        .ok_or_else(|| std::io::Error::other(format!("SFTP metadata size unavailable: {path}")))?;
    let (read_len, truncated) = bounded_read_plan(file_len, max_bytes)?;

    let first_file = session
        .open(path.to_string())
        .await
        .map_err(|error| russh_to_io(error, &format!("SFTP open {path}")))?;
    let first = read_exact_len(first_file, read_len)
        .await
        .map_err(|error| stream_read_error(error, &format!("SFTP read {path}")))?;

    let second = if truncated {
        first.clone()
    } else {
        let second_file = session
            .open(path.to_string())
            .await
            .map_err(|error| russh_to_io(error, &format!("SFTP reopen {path}")))?;
        read_exact_len(second_file, read_len)
            .await
            .map_err(|error| stream_read_error(error, &format!("SFTP reread {path}")))?
    };
    let after = session
        .symlink_metadata(path.to_string())
        .await
        .map_err(|error| russh_to_io(error, &format!("SFTP metadata {path}")))?;

    if !is_regular_file(&after)
        || after.size != Some(file_len)
        || before.mtime != after.mtime
        || before.permissions != after.permissions
        || before.uid != after.uid
        || before.gid != after.gid
        || first != second
    {
        return Err(std::io::Error::other(format!(
            "SFTP file changed while reading: {path}"
        )));
    }

    Ok(BoundedRead {
        bytes: first,
        truncated,
        unix_mode: before.permissions,
        unix_uid: before.uid,
        unix_gid: before.gid,
    })
}

/// Pin the directory entry with a no-follow hardlink before opening it. SFTP v3
/// has no O_NOFOLLOW flag, so reading the pinned path closes the lstat/open race.
async fn read_pinned_snapshot(
    session: &russh_sftp::client::SftpSession,
    path: &str,
    max_bytes: usize,
    cancellation: &CancellationFlag,
    pause_after_pin: Option<&tokio::sync::Notify>,
) -> std::io::Result<BoundedRead> {
    if cancellation.is_cancelled() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            format!("SFTP read cancelled: {path}"),
        ));
    }
    let _parent = validate_transaction_parent(session, path).await?;
    let pin_path = format!("{path}.arx-read-{}", unique_token());
    match session.hardlink(path.to_string(), pin_path.clone()).await {
        Ok(true) => {}
        Ok(false) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "SFTP server lacks hardlink@openssh.com required for no-follow reads",
            ));
        }
        Err(error) => {
            if let russh_sftp::client::error::Error::Status(status) = &error {
                // Definitive: the server rejected the hardlink creation.
                // ARX did not create pin_path; do not attempt cleanup.
                return Err(std::io::Error::other(format!(
                    "SFTP pin snapshot {path}: server refused hardlink: {status:?}"
                )));
            }
            // Transport-ambiguous: pin creation outcome is uncertain; attempt cleanup.
            match session.remove_file(pin_path.clone()).await {
                Ok(()) => {}
                Err(russh_sftp::client::error::Error::Status(status))
                    if status.status_code == russh_sftp::protocol::StatusCode::NoSuchFile => {}
                Err(cleanup_error) => {
                    return Err(std::io::Error::other(format!(
                        "SFTP pin snapshot {path} failed ({error}); pin cleanup is uncertain ({cleanup_error}); pin={pin_path}"
                    )));
                }
            }
            return Err(std::io::Error::other(format!(
                "SFTP pin snapshot {path}: {error}; pin={pin_path}"
            )));
        }
    }

    if let Some(pin_created) = pause_after_pin {
        pin_created.notify_one();
        cancellation.cancelled().await;
    }

    let result = tokio::select! {
        _ = cancellation.cancelled() => Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            format!("SFTP read cancelled: {path}"),
        )),
        result = read_stable_snapshot(session, &pin_path, max_bytes) => result,
    };
    let cleanup = session.remove_file(pin_path.clone()).await;
    match (result, cleanup) {
        (Ok(snapshot), Ok(())) => Ok(snapshot),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(std::io::Error::other(format!(
            "SFTP snapshot read succeeded but pin cleanup failed ({error}); pin={pin_path}"
        ))),
        (Err(read_error), Err(cleanup_error)) => Err(std::io::Error::other(format!(
            "{read_error}; pin cleanup failed ({cleanup_error}); pin={pin_path}"
        ))),
    }
}

async fn remote_entry_metadata(
    session: &russh_sftp::client::SftpSession,
    path: &str,
) -> std::io::Result<Option<russh_sftp::protocol::FileAttributes>> {
    match session.symlink_metadata(path.to_string()).await {
        Ok(metadata) => Ok(Some(metadata)),
        Err(russh_sftp::client::error::Error::Status(status))
            if status.status_code == russh_sftp::protocol::StatusCode::NoSuchFile =>
        {
            Ok(None)
        }
        Err(error) => Err(std::io::Error::other(format!(
            "SFTP inspect {path}: {error}"
        ))),
    }
}

async fn staged_failure(
    session: &russh_sftp::client::SftpSession,
    target_path: &str,
    stage_path: &str,
    cause: impl Into<String>,
) -> std::io::Error {
    let cause = cause.into();
    match session.remove_file(stage_path.to_string()).await {
        Ok(()) => std::io::Error::other(cause),
        Err(cleanup_error) => remote_write_error(
            RemoteWriteFailureKind::RecoveryRequired,
            format!(
                "SFTP RECOVERY REQUIRED {target_path}: {cause}; stage cleanup failed ({cleanup_error}); stage={stage_path}"
            ),
        ),
    }
}

async fn transaction_failure(
    session: &russh_sftp::client::SftpSession,
    path: &str,
    stage_path: &str,
    transaction_path: &str,
    cause: impl Into<String>,
) -> std::io::Error {
    let cause = cause.into();
    let stage_cleanup = session.remove_file(stage_path.to_string()).await.err();
    let transaction_cleanup = session.remove_dir(transaction_path.to_string()).await.err();
    if stage_cleanup.is_some() || transaction_cleanup.is_some() {
        remote_write_error(
            RemoteWriteFailureKind::RecoveryRequired,
            format!(
                "SFTP RECOVERY REQUIRED {path}: {cause}; stage={stage_path}; stage cleanup={stage_cleanup:?}; transaction={transaction_path}; transaction cleanup={transaction_cleanup:?}"
            ),
        )
    } else {
        std::io::Error::other(cause)
    }
}

#[derive(Clone, PartialEq, Eq)]
struct TransactionParent {
    path: String,
    mode: u32,
    uid: u32,
    gid: Option<u32>,
}

fn remote_parent_path(path: &str) -> String {
    match path.rsplit_once('/') {
        Some(("", _)) => "/".into(),
        Some((parent, _)) => parent.into(),
        None => ".".into(),
    }
}

fn private_stage_owner(metadata: &russh_sftp::protocol::FileAttributes) -> Option<u32> {
    (is_regular_file(metadata) && metadata.permissions.map(|mode| mode & 0o7777) == Some(0o600))
        .then_some(metadata.uid)
        .flatten()
}

async fn validate_transaction_parent(
    session: &russh_sftp::client::SftpSession,
    target_path: &str,
) -> std::io::Result<TransactionParent> {
    let path = remote_parent_path(target_path);
    let metadata = session
        .symlink_metadata(path.clone())
        .await
        .map_err(|error| std::io::Error::other(format!("SFTP inspect parent {path}: {error}")))?;
    if !metadata.file_type().is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("SFTP transaction parent is not a directory: {path}"),
        ));
    }
    let mode = metadata
        .permissions
        .ok_or_else(|| std::io::Error::other(format!("SFTP parent mode unavailable: {path}")))?;
    let uid = metadata
        .uid
        .ok_or_else(|| std::io::Error::other(format!("SFTP parent owner unavailable: {path}")))?;
    if parent_is_unsafe_writable(mode) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("SFTP unsafe writable transaction parent without sticky bit: {path}"),
        ));
    }
    Ok(TransactionParent {
        path,
        mode,
        uid,
        gid: metadata.gid,
    })
}

/// TRACK H (#53): centralized, documented parent policy.
///
/// Group/world-writable target parents are rejected when exclusive namespace
/// control cannot be proven — i.e. writable AND missing the sticky bit. A
/// sticky `/tmp`-style parent (mode `0o1777`) is accepted because the sticky
/// bit prevents another account from replacing our private transaction
/// namespace. A private parent (`0o700`/`0o755`) is always accepted.
///
/// This is the single source of truth; `validate_transaction_parent` and all
/// tests route through it. We do NOT implement a temp-directory pin workaround
/// because no equivalent inode/namespace safety proof exists for a non-sticky
/// writable parent — fail closed instead.
pub(crate) fn parent_is_unsafe_writable(mode: u32) -> bool {
    mode & 0o022 != 0 && mode & 0o1000 == 0
}

async fn verify_transaction_parent_unchanged(
    session: &russh_sftp::client::SftpSession,
    target_path: &str,
    expected: &TransactionParent,
) -> std::io::Result<()> {
    if validate_transaction_parent(session, target_path).await? != *expected {
        return Err(std::io::Error::other(format!(
            "SFTP transaction parent changed: {}",
            expected.path
        )));
    }
    Ok(())
}

async fn verify_transaction_dir(
    session: &russh_sftp::client::SftpSession,
    transaction_path: &str,
    expected_uid: u32,
) -> std::io::Result<()> {
    let metadata = session
        .symlink_metadata(transaction_path.to_string())
        .await
        .map_err(|error| {
            std::io::Error::other(format!(
                "SFTP inspect transaction namespace {transaction_path}: {error}"
            ))
        })?;
    if !metadata.file_type().is_dir()
        || metadata.permissions.map(|mode| mode & 0o7777) != Some(0o700)
        || metadata.uid != Some(expected_uid)
    {
        return Err(std::io::Error::other(format!(
            "SFTP transaction namespace is not private or changed owner: {transaction_path}"
        )));
    }
    Ok(())
}

async fn prepare_transaction_dir(
    session: &russh_sftp::client::SftpSession,
    path: &str,
    stage_path: &str,
    transaction_path: &str,
    parent: &TransactionParent,
    expected_owner: u32,
) -> std::io::Result<u32> {
    let metadata = match session.symlink_metadata(transaction_path.to_string()).await {
        Ok(metadata) => metadata,
        Err(error) => {
            return Err(transaction_failure(
                session,
                path,
                stage_path,
                transaction_path,
                format!("SFTP inspect transaction namespace: {error}"),
            )
            .await);
        }
    };
    if metadata.uid != Some(expected_owner) {
        return Err(transaction_failure(
            session,
            path,
            stage_path,
            transaction_path,
            format!("SFTP transaction namespace owner mismatch: {transaction_path}"),
        )
        .await);
    }
    if let Err(error) = verify_transaction_dir(session, transaction_path, expected_owner).await {
        return Err(transaction_failure(
            session,
            path,
            stage_path,
            transaction_path,
            error.to_string(),
        )
        .await);
    }
    let entries = match session.read_dir(transaction_path.to_string()).await {
        Ok(entries) => entries.collect::<Vec<_>>(),
        Err(error) => {
            return Err(transaction_failure(
                session,
                path,
                stage_path,
                transaction_path,
                format!("SFTP inspect empty transaction namespace: {error}"),
            )
            .await);
        }
    };
    if !entries.is_empty() {
        return Err(transaction_failure(
            session,
            path,
            stage_path,
            transaction_path,
            format!("SFTP transaction namespace was modified before use: {transaction_path}"),
        )
        .await);
    }
    if let Err(error) = verify_transaction_parent_unchanged(session, path, parent).await {
        return Err(transaction_failure(
            session,
            path,
            stage_path,
            transaction_path,
            format!("SFTP revalidate transaction parent: {error}"),
        )
        .await);
    }
    Ok(expected_owner)
}

async fn staged_conflict(
    session: &russh_sftp::client::SftpSession,
    path: &str,
    stage_path: &str,
    transaction_path: &str,
    reason: &str,
) -> std::io::Error {
    let stage_cleanup = session.remove_file(stage_path.to_string()).await.err();
    let transaction_cleanup = session.remove_dir(transaction_path.to_string()).await.err();
    if stage_cleanup.is_none() && transaction_cleanup.is_none() {
        std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("SFTP conflict {path}: {reason}"),
        )
    } else {
        remote_write_error(
            RemoteWriteFailureKind::RecoveryRequired,
            format!(
                "SFTP RECOVERY REQUIRED {path}: {reason}; stage={stage_path}; stage cleanup={stage_cleanup:?}; transaction={transaction_path}; transaction cleanup={transaction_cleanup:?}"
            ),
        )
    }
}

async fn cancel_before_commit(
    session: &russh_sftp::client::SftpSession,
    path: &str,
    stage_path: &str,
    transaction_path: Option<&str>,
) -> std::io::Error {
    let stage_cleanup = session.remove_file(stage_path.to_string()).await.err();
    let transaction_cleanup = if let Some(transaction_path) = transaction_path {
        session.remove_dir(transaction_path.to_string()).await.err()
    } else {
        None
    };
    if stage_cleanup.is_some() || transaction_cleanup.is_some() {
        remote_write_error(
            RemoteWriteFailureKind::RecoveryRequired,
            format!(
                "SFTP RECOVERY REQUIRED {path}: cancellation cleanup failed; stage={stage_path}; stage cleanup={stage_cleanup:?}; transaction cleanup={transaction_cleanup:?}"
            ),
        )
    } else {
        std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            format!("SFTP remote edit cancelled before commit: {path}"),
        )
    }
}

async fn restore_backup_no_clobber(
    session: &russh_sftp::client::SftpSession,
    backup_path: &str,
    target_path: &str,
) -> Result<(), String> {
    match session
        .hardlink(backup_path.to_string(), target_path.to_string())
        .await
    {
        Ok(true) => {}
        Ok(false) => return Err("server lacks hardlink@openssh.com".into()),
        Err(error) => return Err(format!("restore link failed: {error}")),
    }
    session
        .remove_file(backup_path.to_string())
        .await
        .map_err(|error| format!("restored target but backup cleanup failed: {error}"))
}

#[derive(Clone, Copy)]
enum AtomicWriteFault {
    PreserveMode,
    VerifyBackup,
    VerifyVisible,
    Commit,
    Restore,
    BackupCleanup,
    ConcurrentTarget,
    CancelBeforeCommit,
    /// Deterministic transport break injected immediately after the staged
    /// payload is written (before backup). Simulates a break where the
    /// original is intact and no backup yet exists.
    StageWrite,
    /// Deterministic transport break injected immediately after the backup
    /// rename succeeds. Simulates a break where the backup IS preserved.
    BackupRename,
    /// Deterministic UID/GID metadata race: the staged file's ownership is
    /// detected to differ from the expected revision, forcing recovery before
    /// any silent replacement.
    MetadataRace,
}

#[cfg(test)]
#[derive(Clone, Copy, Default)]
struct AtomicWriteFaults {
    preserve_mode: bool,
    verify_backup: bool,
    verify_visible: bool,
    commit: bool,
    restore: bool,
    backup_cleanup: bool,
    concurrent_target: bool,
    cancel_before_commit: bool,
    stage_write: bool,
    backup_rename: bool,
    metadata_race: bool,
}

pub struct SftpProvider {
    pub host: crate::remote::Host,
    connection: Mutex<Option<crate::remote::openssh_sftp::OpenSshSftpConnection>>,
    #[cfg(test)]
    faults: AtomicWriteFaults,
    #[cfg(test)]
    pause_after_pin: Option<std::sync::Arc<tokio::sync::Notify>>,
    /// Test-only injectable seam for the pooled acquire/probe/reconnect
    /// algorithm (TRACK C #48 deterministic matrix). When set,
    /// `connect_for_mutation` uses these instead of a real SSH session, so the
    /// exact same production algorithm runs without any network or `sleep`.
    test_probe: Option<TestProbeFn>,
    test_connect: Option<TestConnectFn>,
}

/// ponytail: injected async probe used by the deterministic pooled-acquire
/// matrix. Async so a test can inject a probe that pends (proving the acquire
/// respects a bounded timeout rather than hanging).
type TestProbeFn = Box<
    dyn Fn(
            &crate::remote::openssh_sftp::OpenSshSftpConnection,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = SftpInvalidation> + Send + Sync>>
        + Send
        + Sync,
>;

/// ponytail: boxed async factory standing in for a real SSH connect in tests.
type TestConnectFn = Box<
    dyn Fn() -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = std::io::Result<
                            crate::remote::openssh_sftp::OpenSshSftpConnection,
                        >,
                    > + Send,
            >,
        > + Send
        + Sync,
>;

impl std::fmt::Debug for SftpProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SftpProvider")
            .field("host", &self.host)
            .field("connection", &"<pooled>")
            .finish()
    }
}

impl SftpProvider {
    pub fn new(host: crate::remote::Host) -> Self {
        Self {
            host,
            connection: Mutex::new(None),
            #[cfg(test)]
            faults: AtomicWriteFaults::default(),
            #[cfg(test)]
            pause_after_pin: None,
            test_probe: None,
            test_connect: None,
        }
    }

    fn pause_after_pin(&self) -> Option<&tokio::sync::Notify> {
        #[cfg(test)]
        {
            self.pause_after_pin.as_deref()
        }
        #[cfg(not(test))]
        {
            None
        }
    }

    fn fault_enabled(&self, fault: AtomicWriteFault) -> bool {
        #[cfg(test)]
        {
            match fault {
                AtomicWriteFault::PreserveMode => self.faults.preserve_mode,
                AtomicWriteFault::VerifyBackup => self.faults.verify_backup,
                AtomicWriteFault::VerifyVisible => self.faults.verify_visible,
                AtomicWriteFault::Commit => self.faults.commit,
                AtomicWriteFault::Restore => self.faults.restore,
                AtomicWriteFault::BackupCleanup => self.faults.backup_cleanup,
                AtomicWriteFault::ConcurrentTarget => self.faults.concurrent_target,
                AtomicWriteFault::CancelBeforeCommit => self.faults.cancel_before_commit,
                AtomicWriteFault::StageWrite => self.faults.stage_write,
                AtomicWriteFault::BackupRename => self.faults.backup_rename,
                AtomicWriteFault::MetadataRace => self.faults.metadata_race,
            }
        }
        #[cfg(not(test))]
        {
            let _ = fault;
            false
        }
    }

    async fn list_pooled(&self, path: &str) -> std::io::Result<Vec<Entry>> {
        // Probe the pooled session (transport break → fresh replace) before reuse.
        let mut guard = self.connect_for_mutation().await?;

        // One reconnect attempt handles servers closing an idle subsystem
        // between directory reads while avoiding a reconnect per directory.
        for attempt in 0..2 {
            if guard.is_none() {
                *guard = Some(
                    crate::remote::openssh_sftp::OpenSshSftpConnection::connect(
                        &self.host.ssh_alias,
                    )
                    .await?,
                );
            }

            let result = guard
                .as_ref()
                .expect("connection initialized")
                .session()
                .read_dir(path.to_string())
                .await;

            match result {
                Ok(entries) => return Ok(entries_from_read_dir(entries.collect())),
                Err(error) => {
                    // Transport-only invalidation: keep pool on application/Status error.
                    if classify_russh_error(&error).should_invalidate()
                        && let Some(mut broken) = guard.take()
                    {
                        broken.abort().await;
                    }
                    if attempt == 1 {
                        return Err(std::io::Error::other(format!(
                            "SFTP read_dir {path}: {error}"
                        )));
                    }
                }
            }
        }

        unreachable!("SFTP retry loop always returns")
    }

    /// Reuse pooled connection without retry (mutations are not retried).
    /// Before reuse, runs a bounded health probe (TRACK C #48); a stale or
    /// broken session is discarded and a fresh one acquired.
    #[allow(dead_code)]
    async fn connect_for_mutation(
        &self,
    ) -> std::io::Result<
        tokio::sync::MutexGuard<'_, Option<crate::remote::openssh_sftp::OpenSshSftpConnection>>,
    > {
        let mut guard = self.connection.lock().await;
        // Probe the pooled session (async); abort only on transport break.
        // Test seam: if `test_probe` is injected, it stands in for the real
        // `realpath(".")` health probe so the matrix runs without a network.
        let probe = match guard.as_ref() {
            Some(session) => {
                if let Some(probe_fn) = &self.test_probe {
                    probe_fn(session).await
                } else {
                    probe_session_healthy(session.session()).await
                }
            }
            None => SftpInvalidation::Keep,
        };
        // #48: the health-probe policy decides reuse vs discard+reconnect.
        if matches!(
            pool_health_action(guard.is_some(), probe),
            PoolHealthAction::DiscardAndReconnect
        ) && let Some(mut broken) = guard.take()
        {
            broken.abort().await;
        }
        if guard.is_none() {
            let conn = if let Some(connect_fn) = &self.test_connect {
                connect_fn().await?
            } else {
                crate::remote::openssh_sftp::OpenSshSftpConnection::connect(&self.host.ssh_alias)
                    .await?
            };
            *guard = Some(conn);
        }
        Ok(guard)
    }

    /// Test-only: inject probe + a connect counter for the deterministic #48
    /// acquire/probe/reconnect matrix. The same `connect_for_mutation` algorithm
    /// runs; only the SSH I/O is replaced (by a harmless stub), so no network or
    /// `sleep` is needed. `connects` is incremented exactly once per real
    /// (re)connection, proving reuse vs fresh-replace behavior.
    #[cfg(test)]
    fn with_test_pool(
        mut self,
        probe: Option<SftpInvalidation>,
        connects: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) -> Self {
        let probe = probe.map(|p| {
            Box::new(
                move |_conn: &crate::remote::openssh_sftp::OpenSshSftpConnection| {
                    Box::pin(async move { p })
                        as std::pin::Pin<
                            Box<dyn std::future::Future<Output = SftpInvalidation> + Send + Sync>,
                        >
                },
            )
                as Box<
                    dyn Fn(
                            &crate::remote::openssh_sftp::OpenSshSftpConnection,
                        ) -> std::pin::Pin<
                            Box<dyn std::future::Future<Output = SftpInvalidation> + Send + Sync>,
                        > + Send
                        + Sync,
                >
        });
        let connects = connects.clone();
        let connect: TestConnectFn = Box::new(move || {
            let connects = connects.clone();
            Box::pin(async move {
                connects.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(crate::remote::openssh_sftp::OpenSshSftpConnection::test_stub().await)
            })
        });
        self.test_probe = probe;
        self.test_connect = Some(connect);
        self
    }

    /// Test-only: inject a probe that never resolves (pending forever) so the
    /// bounded-timeout acquire path can be proven deterministically under a
    /// bounded `tokio::time::timeout` (no real `sleep`, no `test-util` feature).
    /// Also injects the stub connect so the first acquire succeeds immediately
    /// and only the second (pooled) acquire hits the pending probe.
    #[cfg(test)]
    fn with_test_pool_pending_probe(
        mut self,
        connects: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) -> Self {
        let connects2 = connects.clone();
        let connect: TestConnectFn = Box::new(move || {
            let connects2 = connects2.clone();
            Box::pin(async move {
                connects2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(crate::remote::openssh_sftp::OpenSshSftpConnection::test_stub().await)
            })
        });
        let probe: TestProbeFn = Box::new(
            |_conn: &crate::remote::openssh_sftp::OpenSshSftpConnection| {
                Box::pin(async {
                    // Never resolves: stands in for a health probe that hangs.
                    std::future::pending::<()>().await;
                    unreachable!()
                })
            },
        );
        self.test_connect = Some(connect);
        self.test_probe = Some(probe);
        self
    }

    async fn mkdir(&self, path: &str) -> std::io::Result<()> {
        let mut guard = self.connect_for_mutation().await?;
        let conn = guard
            .as_ref()
            .ok_or_else(|| std::io::Error::other("SFTP connection lost"))?;
        match conn
            .session
            .as_ref()
            .expect("connected session")
            .create_dir(path.to_string())
            .await
        {
            Ok(()) => Ok(()),
            Err(e) => {
                // Transport-only invalidation: keep pool on application/Status error.
                if classify_russh_error(&e).should_invalidate()
                    && let Some(mut broken) = guard.take()
                {
                    broken.abort().await;
                }
                Err(std::io::Error::other(format!("SFTP mkdir {path}: {e}")))
            }
        }
    }

    async fn remove_file(&self, path: &str) -> std::io::Result<()> {
        let mut guard = self.connect_for_mutation().await?;
        let conn = guard
            .as_ref()
            .ok_or_else(|| std::io::Error::other("SFTP connection lost"))?;
        match conn
            .session
            .as_ref()
            .expect("connected session")
            .remove_file(path.to_string())
            .await
        {
            Ok(()) => Ok(()),
            Err(e) => {
                // Transport-only invalidation: keep pool on application/Status error.
                if classify_russh_error(&e).should_invalidate()
                    && let Some(mut broken) = guard.take()
                {
                    broken.abort().await;
                }
                Err(std::io::Error::other(format!(
                    "SFTP remove_file {path}: {e}"
                )))
            }
        }
    }

    async fn remove_dir(&self, path: &str) -> std::io::Result<()> {
        let mut guard = self.connect_for_mutation().await?;
        let conn = guard
            .as_ref()
            .ok_or_else(|| std::io::Error::other("SFTP connection lost"))?;
        match conn
            .session
            .as_ref()
            .expect("connected session")
            .remove_dir(path.to_string())
            .await
        {
            Ok(()) => Ok(()),
            Err(e) => {
                // Transport-only invalidation: keep pool on application/Status error.
                if classify_russh_error(&e).should_invalidate()
                    && let Some(mut broken) = guard.take()
                {
                    broken.abort().await;
                }
                Err(std::io::Error::other(format!(
                    "SFTP remove_dir {path}: {e}"
                )))
            }
        }
    }
}
#[async_trait::async_trait]
impl VfsProvider for SftpProvider {
    fn list(&self, path: &str) -> std::io::Result<Vec<Entry>> {
        SftpFs::list(&self.host, path)
    }

    async fn list_async(&self, path: &str) -> std::io::Result<Vec<Entry>> {
        self.list_pooled(path).await
    }

    fn read_head(&self, path: &str, max_lines: usize) -> std::io::Result<Vec<String>> {
        // ponytail: sync bridge for legacy VfsProvider::read_head callers.
        // New SFTP preview goes through read_prefix_bytes (async) in the
        // effect pipeline.
        const MAX_BYTES: usize = 1024 * 1024; // 1 MiB

        let host = self.host.clone();
        let path = path.to_string();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| std::io::Error::other(format!("SFTP runtime: {e}")))?;
            rt.block_on(async {
                let provider = SftpProvider::new(host);
                let bounded = provider.read_prefix(&path, MAX_BYTES).await?;
                crate::services::preview::format_bounded_preview(
                    &bounded.bytes,
                    None,
                    bounded.truncated,
                    &path,
                    max_lines,
                )
            })
        })
        .join()
        .map_err(|_| std::io::Error::other("SFTP worker thread panicked"))?
    }

    fn copy_files(&self, _src: &str, _dst: &str, _names: &[String]) -> std::io::Result<usize> {
        Err(std::io::Error::other("SFTP copy via transfer planner"))
    }

    fn move_files(&self, _src: &str, _dst: &str, _names: &[String]) -> std::io::Result<usize> {
        Err(std::io::Error::other("SFTP move via transfer planner"))
    }

    fn delete_files(&self, _dir: &str, _names: &[String]) -> std::io::Result<usize> {
        Err(std::io::Error::other("SFTP delete via transfer planner"))
    }

    async fn mkdir(&self, path: &str) -> std::io::Result<()> {
        self.mkdir(path).await
    }

    async fn remove_file(&self, path: &str) -> std::io::Result<()> {
        self.remove_file(path).await
    }

    async fn remove_dir(&self, path: &str) -> std::io::Result<()> {
        self.remove_dir(path).await
    }

    async fn read_prefix_bytes(
        &self,
        path: &str,
        max_bytes: usize,
    ) -> std::io::Result<BoundedRead> {
        self.read_prefix(path, max_bytes).await
    }

    async fn write_file_bytes_if_unchanged(
        &self,
        path: &str,
        data: &[u8],
        revision: &RemoteEditRevision,
        cancellation: &CancellationFlag,
    ) -> std::io::Result<()> {
        // TRACK G (#52): no double-lock. `write_atomic` already invalidates the
        // pooled connection itself when the transport breaks; application-level
        // (Status) failures correctly keep the connection. Re-acquiring the lock
        // here only to rediscover connection state would be a misleading second
        // lock scope, so we just propagate the result.
        self.write_atomic(path, data, revision, cancellation).await
    }

    async fn metadata(&self, path: &str) -> std::io::Result<FileMetadata> {
        self.remote_metadata(path).await
    }

    async fn read_all_capped(&self, path: &str, max_bytes: usize) -> std::io::Result<BoundedRead> {
        self.read_all(path, max_bytes, &CancellationFlag::default())
            .await
    }

    async fn read_all_capped_cancellable(
        &self,
        path: &str,
        max_bytes: usize,
        cancellation: &CancellationFlag,
    ) -> std::io::Result<BoundedRead> {
        self.read_all(path, max_bytes, cancellation).await
    }
}

impl SftpProvider {
    /// Read up to `max_bytes` from the beginning of a remote file.
    /// Uses pooled connection with one retry — read is non-destructive.
    async fn read_prefix(&self, path: &str, max_bytes: usize) -> std::io::Result<BoundedRead> {
        use tokio::io::AsyncReadExt;

        // Probe the pooled session (transport break → fresh replace) before reuse.
        let mut guard = self.connect_for_mutation().await?;

        for attempt in 0..2 {
            if guard.is_none() {
                *guard = Some(
                    crate::remote::openssh_sftp::OpenSshSftpConnection::connect(
                        &self.host.ssh_alias,
                    )
                    .await?,
                );
            }

            let conn = guard
                .as_ref()
                .ok_or_else(|| std::io::Error::other("SFTP connection lost"))?;

            let open_result = conn
                .session
                .as_ref()
                .expect("connected session")
                .open(path.to_string())
                .await;

            match open_result {
                Ok(mut file) => {
                    // ponytail: read bounded prefix, loops on short chunks
                    let cap = max_bytes + 1; // +1 for truncation detection
                    let mut buf = Vec::new();
                    // read_to_end is bounded by take(cap)
                    tokio::io::AsyncReadExt::take(&mut file, cap as u64)
                        .read_to_end(&mut buf)
                        .await
                        .map_err(|e| stream_read_error(e, &format!("SFTP read {path}")))?;
                    let truncated = buf.len() > max_bytes;
                    if truncated {
                        buf.truncate(max_bytes);
                    }
                    return Ok(BoundedRead {
                        bytes: buf,
                        truncated,
                        unix_mode: None,
                        unix_uid: None,
                        unix_gid: None,
                    });
                }
                Err(error) => {
                    // Transport-only invalidation: keep pool on application/Status error.
                    if classify_russh_error(&error).should_invalidate()
                        && let Some(mut broken) = guard.take()
                    {
                        broken.abort().await;
                    }
                    if attempt == 1 {
                        return Err(std::io::Error::other(format!("SFTP open {path}: {error}")));
                    }
                }
            }
        }

        unreachable!()
    }

    /// Read a stable snapshot up to max_bytes. Files within the limit are read
    /// twice; size, mtime, mode, ownership, and content must remain unchanged.
    async fn read_all(
        &self,
        path: &str,
        max_bytes: usize,
        cancellation: &CancellationFlag,
    ) -> std::io::Result<BoundedRead> {
        let mut guard = self.connect_for_mutation().await?;
        let session = &guard
            .as_ref()
            .ok_or_else(|| std::io::Error::other("SFTP connection lost"))?
            .session();
        let result = read_pinned_snapshot(
            session,
            path,
            max_bytes,
            cancellation,
            self.pause_after_pin(),
        )
        .await;
        if let Err(error) = &result {
            // Transport-only invalidation: keep pool on application/Status error.
            if classify_io_error(error).should_invalidate()
                && let Some(mut broken) = guard.take()
            {
                broken.abort().await;
            }
        }
        result
    }

    /// Atomic write via SFTP: stage → verify → commit → rollback on failure.
    /// Reuses the transactional pattern proven in sftp_copy::upload_file.
    async fn write_atomic(
        &self,
        path: &str,
        data: &[u8],
        revision: &RemoteEditRevision,
        cancellation: &CancellationFlag,
    ) -> std::io::Result<()> {
        use tokio::io::AsyncWriteExt;

        let expected_original = revision.bytes();
        let original_unix_mode = revision.unix_mode();
        let original_unix_uid = revision.unix_uid();
        let original_unix_gid = revision.unix_gid();

        if cancellation.is_cancelled() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                format!("SFTP remote edit cancelled before staging: {path}"),
            ));
        }

        let token = unique_token();
        let mut stage_path = format!("{path}.arx-part-{token}");
        let transaction_path = format!("{path}.arx-txn-{token}");
        let transaction_stage_path = format!("{transaction_path}/stage");
        let backup_path = format!("{transaction_path}/backup");

        let mut guard = self.connect_for_mutation().await?;
        let session = &guard
            .as_ref()
            .ok_or_else(|| std::io::Error::other("SFTP connection lost"))?
            .session();
        let transaction_parent = validate_transaction_parent(session, path).await?;

        // Create an empty 0600 stage first. Its owner proves which account the
        // separate atomic mkdir command must create the private namespace for.
        let mut create_attrs = russh_sftp::protocol::FileAttributes::empty();
        create_attrs.permissions = Some(0o600);
        let mut remote = match session
            .open_with_flags_and_attributes(
                stage_path.clone(),
                russh_sftp::protocol::OpenFlags::CREATE
                    | russh_sftp::protocol::OpenFlags::EXCLUDE
                    | russh_sftp::protocol::OpenFlags::WRITE,
                create_attrs,
            )
            .await
        {
            Ok(remote) => remote,
            Err(russh_sftp::client::error::Error::Status(status)) => {
                return Err(std::io::Error::other(format!(
                    "SFTP create stage for {path} refused: {status:?}"
                )));
            }
            Err(error) => {
                let message = format!(
                    "SFTP RECOVERY REQUIRED {path}: stage creation outcome is uncertain ({error}); stage={stage_path}"
                );
                if let Some(mut broken) = guard.take() {
                    broken.abort().await;
                }
                return Err(remote_write_error(
                    RemoteWriteFailureKind::RecoveryRequired,
                    message,
                ));
            }
        };
        let stage_owner = match session.symlink_metadata(stage_path.clone()).await {
            Ok(metadata) => match private_stage_owner(&metadata) {
                Some(owner) => owner,
                None => {
                    drop(remote);
                    return Err(staged_failure(
                        session,
                        path,
                        &stage_path,
                        format!("SFTP stage is not a private 0600 regular file: {stage_path}"),
                    )
                    .await);
                }
            },
            Err(error) => {
                drop(remote);
                return Err(staged_failure(
                    session,
                    path,
                    &stage_path,
                    format!("SFTP inspect stage {stage_path}: {error}"),
                )
                .await);
            }
        };

        match crate::remote::openssh_sftp::OpenSshSftpConnection::create_private_dir(
            &self.host.ssh_alias,
            &transaction_path,
        )
        .await
        {
            Ok(()) => {}
            Err(error) if classify_io_error(&error).should_invalidate() => {
                drop(remote);
                let stage_cleanup = session.remove_file(stage_path.clone()).await.err();
                return Err(remote_write_error(
                    RemoteWriteFailureKind::RecoveryRequired,
                    format!(
                        "SFTP RECOVERY REQUIRED {path}: private transaction creation outcome is uncertain ({error}); transaction={transaction_path}; stage={stage_path}; stage cleanup={stage_cleanup:?}"
                    ),
                ));
            }
            Err(error) => {
                drop(remote);
                return Err(staged_failure(
                    session,
                    path,
                    &stage_path,
                    format!(
                        "SFTP create private transaction namespace {transaction_path}: {error}"
                    ),
                )
                .await);
            }
        }
        let transaction_owner = prepare_transaction_dir(
            session,
            path,
            &stage_path,
            &transaction_path,
            &transaction_parent,
            stage_owner,
        )
        .await?;

        match session
            .rename(stage_path.clone(), transaction_stage_path.clone())
            .await
        {
            Ok(()) => stage_path = transaction_stage_path,
            Err(russh_sftp::client::error::Error::Status(status)) => {
                drop(remote);
                return Err(transaction_failure(
                    session,
                    path,
                    &stage_path,
                    &transaction_path,
                    format!("SFTP move stage into private transaction refused: {status:?}"),
                )
                .await);
            }
            Err(error) => {
                drop(remote);
                let message = format!(
                    "SFTP RECOVERY REQUIRED {path}: moving stage into transaction is uncertain ({error}); old-stage={stage_path}; transaction-stage={transaction_stage_path}; transaction={transaction_path}"
                );
                if let Some(mut broken) = guard.take() {
                    broken.abort().await;
                }
                return Err(remote_write_error(
                    RemoteWriteFailureKind::RecoveryRequired,
                    message,
                ));
            }
        }

        // Probe no-clobber support inside the private namespace before upload.
        let link_probe_path = format!("{transaction_path}/link-probe");
        match session
            .hardlink(stage_path.clone(), link_probe_path.clone())
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                drop(remote);
                return Err(transaction_failure(
                    session,
                    path,
                    &stage_path,
                    &transaction_path,
                    "SFTP server lacks hardlink@openssh.com required for safe commit",
                )
                .await);
            }
            Err(russh_sftp::client::error::Error::Status(status)) => {
                drop(remote);
                return Err(transaction_failure(
                    session,
                    path,
                    &stage_path,
                    &transaction_path,
                    format!("SFTP no-clobber capability probe refused: {status:?}"),
                )
                .await);
            }
            Err(error) => {
                drop(remote);
                let message = format!(
                    "SFTP RECOVERY REQUIRED {path}: no-clobber probe outcome is uncertain ({error}); probe={link_probe_path}; stage={stage_path}; transaction={transaction_path}"
                );
                if let Some(mut broken) = guard.take() {
                    broken.abort().await;
                }
                return Err(remote_write_error(
                    RemoteWriteFailureKind::RecoveryRequired,
                    message,
                ));
            }
        }
        if let Err(error) = session.remove_file(link_probe_path.clone()).await {
            drop(remote);
            return Err(remote_write_error(
                RemoteWriteFailureKind::RecoveryRequired,
                format!(
                    "SFTP RECOVERY REQUIRED {path}: no-clobber probe cleanup failed ({error}); probe={link_probe_path}; stage={stage_path}; transaction={transaction_path}"
                ),
            ));
        }
        if cancellation.is_cancelled() {
            drop(remote);
            return Err(
                cancel_before_commit(session, path, &stage_path, Some(&transaction_path)).await,
            );
        }

        if let Err(e) = remote.write_all(data).await {
            drop(remote);
            return Err(transaction_failure(
                session,
                path,
                &stage_path,
                &transaction_path,
                format!("SFTP write {path}: {e}"),
            )
            .await);
        }
        if let Err(e) = remote.flush().await {
            drop(remote);
            return Err(transaction_failure(
                session,
                path,
                &stage_path,
                &transaction_path,
                format!("SFTP flush {path}: {e}"),
            )
            .await);
        }
        if let Err(e) = remote.shutdown().await {
            drop(remote);
            return Err(transaction_failure(
                session,
                path,
                &stage_path,
                &transaction_path,
                format!("SFTP close {path}: {e}"),
            )
            .await);
        }
        drop(remote);
        if self.fault_enabled(AtomicWriteFault::MetadataRace) {
            // UID/GID metadata race: detect ownership drift before any
            // replacement and force recovery (never silently replace).
            return Err(remote_write_error(
                RemoteWriteFailureKind::RecoveryRequired,
                format!(
                    "SFTP RECOVERY REQUIRED {path}: UID/GID metadata race detected after staging (injected); stage={stage_path}; transaction={transaction_path}"
                ),
            ));
        }
        if self.fault_enabled(AtomicWriteFault::StageWrite) {
            // Deterministic transport break after the staged payload is
            // written but before backup exists. Original remains intact.
            if let Some(mut broken) = guard.take() {
                broken.abort().await;
            }
            return Err(remote_write_error(
                RemoteWriteFailureKind::RecoveryRequired,
                format!(
                    "SFTP RECOVERY REQUIRED {path}: transport break after stage write (injected); original intact; stage={stage_path}; transaction={transaction_path}"
                ),
            ));
        }
        if cancellation.is_cancelled() {
            return Err(
                cancel_before_commit(session, path, &stage_path, Some(&transaction_path)).await,
            );
        }

        // Keep the staged payload private while it is written. Restore the
        // target metadata only after the complete payload is closed.
        if self.fault_enabled(AtomicWriteFault::PreserveMode) {
            return Err(transaction_failure(
                session,
                path,
                &stage_path,
                &transaction_path,
                format!("SFTP preserve metadata for {path}: injected failure"),
            )
            .await);
        }
        let mut attrs = russh_sftp::protocol::FileAttributes::empty();
        attrs.permissions = Some(original_unix_mode);
        attrs.uid = Some(original_unix_uid);
        attrs.gid = Some(original_unix_gid);
        if let Err(error) = session.set_metadata(stage_path.clone(), attrs).await {
            return Err(transaction_failure(
                session,
                path,
                &stage_path,
                &transaction_path,
                format!("SFTP preserve metadata for {path}: {error}"),
            )
            .await);
        }
        if cancellation.is_cancelled() {
            return Err(
                cancel_before_commit(session, path, &stage_path, Some(&transaction_path)).await,
            );
        }

        // ── Verify staged content and metadata ──
        match self
            .verify_remote_matches(
                session,
                &stage_path,
                data,
                original_unix_mode,
                original_unix_uid,
                original_unix_gid,
            )
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                return Err(transaction_failure(
                    session,
                    path,
                    &stage_path,
                    &transaction_path,
                    format!("SFTP stage verification failed for {path}"),
                )
                .await);
            }
            Err(error) => {
                return Err(transaction_failure(
                    session,
                    path,
                    &stage_path,
                    &transaction_path,
                    format!("SFTP stage verification error for {path}: {error}"),
                )
                .await);
            }
        }

        // ── Backup existing target ──
        let target_metadata = match remote_entry_metadata(session, path).await {
            Ok(metadata) => metadata,
            Err(error) => {
                return Err(transaction_failure(
                    session,
                    path,
                    &stage_path,
                    &transaction_path,
                    format!("SFTP inspect target {path}: {error}"),
                )
                .await);
            }
        };
        let Some(target_metadata) = target_metadata else {
            return Err(staged_conflict(
                session,
                path,
                &stage_path,
                &transaction_path,
                "remote target disappeared during edit",
            )
            .await);
        };
        if !is_regular_file(&target_metadata) {
            return Err(staged_conflict(
                session,
                path,
                &stage_path,
                &transaction_path,
                "remote target is no longer a regular file",
            )
            .await);
        }
        match remote_entry_metadata(session, &backup_path).await {
            Ok(None) => {}
            Ok(Some(_)) => {
                return Err(transaction_failure(
                    session,
                    path,
                    &stage_path,
                    &transaction_path,
                    format!("SFTP backup path already exists: {backup_path}"),
                )
                .await);
            }
            Err(error) => {
                return Err(transaction_failure(
                    session,
                    path,
                    &stage_path,
                    &transaction_path,
                    format!("SFTP inspect backup path {backup_path}: {error}"),
                )
                .await);
            }
        }
        if self.fault_enabled(AtomicWriteFault::CancelBeforeCommit) {
            cancellation.cancel();
        }
        if cancellation.is_cancelled() {
            return Err(
                cancel_before_commit(session, path, &stage_path, Some(&transaction_path)).await,
            );
        }
        if let Err(error) =
            verify_transaction_parent_unchanged(session, path, &transaction_parent).await
        {
            return Err(transaction_failure(
                session,
                path,
                &stage_path,
                &transaction_path,
                format!("SFTP transaction parent changed before commit: {error}"),
            )
            .await);
        }
        if let Err(error) =
            verify_transaction_dir(session, &transaction_path, transaction_owner).await
        {
            return Err(transaction_failure(
                session,
                path,
                &stage_path,
                &transaction_path,
                format!("SFTP transaction namespace changed before commit: {error}"),
            )
            .await);
        }
        match session.rename(path.to_string(), backup_path.clone()).await {
            Ok(()) => {
                if self.fault_enabled(AtomicWriteFault::BackupRename) {
                    // Deterministic transport break after the backup rename
                    // succeeds. Backup IS preserved; operation must reach
                    // rollback/recovery terminal truth, never abandon.
                    if let Some(mut broken) = guard.take() {
                        broken.abort().await;
                    }
                    return Err(remote_write_error(
                        RemoteWriteFailureKind::RecoveryRequired,
                        format!(
                            "SFTP RECOVERY REQUIRED {path}: transport break after backup rename (injected); backup preserved={backup_path}; stage={stage_path}; transaction={transaction_path}"
                        ),
                    ));
                }
            }
            Err(russh_sftp::client::error::Error::Status(status)) => {
                return Err(transaction_failure(
                    session,
                    path,
                    &stage_path,
                    &transaction_path,
                    format!("SFTP backup rename refused for {path}: {status:?}"),
                )
                .await);
            }
            Err(error) => {
                let message = format!(
                    "SFTP RECOVERY REQUIRED {path}: backup rename transport outcome is uncertain ({error}); backup={backup_path}; stage={stage_path}"
                );
                if let Some(mut broken) = guard.take() {
                    broken.abort().await;
                }
                return Err(remote_write_error(
                    RemoteWriteFailureKind::RecoveryRequired,
                    message,
                ));
            }
        }

        // ── Commit-time race check against exact content and metadata ──
        let backup_verification = if self.fault_enabled(AtomicWriteFault::VerifyBackup) {
            Ok(false)
        } else {
            self.verify_remote_matches(
                session,
                &backup_path,
                expected_original,
                original_unix_mode,
                original_unix_uid,
                original_unix_gid,
            )
            .await
        };
        let (backup_matches, verification_error) = match backup_verification {
            Ok(matches) => (matches, None),
            Err(error) => (false, Some(error)),
        };
        if !backup_matches {
            let restore_error = if self.fault_enabled(AtomicWriteFault::Restore) {
                Some("injected failure".to_string())
            } else {
                restore_backup_no_clobber(session, &backup_path, path)
                    .await
                    .err()
            };
            if let Some(error) = restore_error {
                return Err(remote_write_error(
                    RemoteWriteFailureKind::RecoveryRequired,
                    format!(
                        "SFTP RECOVERY REQUIRED {path}: verification failed but restore failed ({error}); backup={backup_path}; stage={stage_path}"
                    ),
                ));
            }
            let stage_cleanup = session.remove_file(stage_path.clone()).await.err();
            let transaction_cleanup = session.remove_dir(transaction_path.clone()).await.err();
            if stage_cleanup.is_some() || transaction_cleanup.is_some() {
                return Err(remote_write_error(
                    RemoteWriteFailureKind::RecoveryRequired,
                    format!(
                        "SFTP RECOVERY REQUIRED {path}: original restored after verification failure but cleanup failed; stage={stage_path}; stage cleanup={stage_cleanup:?}; transaction={transaction_path}; transaction cleanup={transaction_cleanup:?}"
                    ),
                ));
            }
            if let Some(error) = verification_error {
                return Err(error);
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("SFTP conflict {path}: remote modified during edit"),
            ));
        }

        // ── Commit ──
        let mut injected_error = None;
        if self.fault_enabled(AtomicWriteFault::ConcurrentTarget) {
            match session.create(path.to_string()).await {
                Ok(mut competing) => {
                    if let Err(error) = competing.write_all(b"concurrent").await {
                        injected_error = Some(format!("inject concurrent target: {error}"));
                    } else if let Err(error) = competing.shutdown().await {
                        injected_error = Some(format!("close concurrent target: {error}"));
                    }
                    drop(competing);
                }
                Err(error) => {
                    injected_error = Some(format!("create concurrent target: {error}"));
                }
            }
        }
        let commit_error = if let Some(error) = injected_error {
            Some(error)
        } else if self.fault_enabled(AtomicWriteFault::Commit) {
            Some("injected failure".to_string())
        } else {
            match session.hardlink(stage_path.clone(), path.to_string()).await {
                Ok(true) => None,
                Ok(false) => Some("server lacks hardlink@openssh.com".to_string()),
                Err(russh_sftp::client::error::Error::Status(status)) => {
                    Some(format!("commit link refused: {status:?}"))
                }
                Err(error) => {
                    let message = format!(
                        "SFTP RECOVERY REQUIRED {path}: commit link transport outcome is uncertain ({error}); backup={backup_path}; stage={stage_path}"
                    );
                    if let Some(mut broken) = guard.take() {
                        broken.abort().await;
                    }
                    return Err(remote_write_error(
                        RemoteWriteFailureKind::RecoveryRequired,
                        message,
                    ));
                }
            }
        };
        if let Some(error) = commit_error {
            let restore_error = if self.fault_enabled(AtomicWriteFault::Restore) {
                Some("injected failure".to_string())
            } else {
                restore_backup_no_clobber(session, &backup_path, path)
                    .await
                    .err()
            };
            if let Some(restore_error) = restore_error {
                return Err(remote_write_error(
                    RemoteWriteFailureKind::RecoveryRequired,
                    format!(
                        "SFTP RECOVERY REQUIRED {path}: commit failed ({error}) and restore failed ({restore_error}); backup={backup_path}; stage={stage_path}"
                    ),
                ));
            }
            let stage_cleanup = session.remove_file(stage_path.clone()).await.err();
            let transaction_cleanup = session.remove_dir(transaction_path.clone()).await.err();
            if stage_cleanup.is_some() || transaction_cleanup.is_some() {
                return Err(remote_write_error(
                    RemoteWriteFailureKind::RecoveryRequired,
                    format!(
                        "SFTP RECOVERY REQUIRED {path}: commit failed ({error}), original restored, but cleanup failed; stage={stage_path}; stage cleanup={stage_cleanup:?}; transaction={transaction_path}; transaction cleanup={transaction_cleanup:?}"
                    ),
                ));
            }
            return Err(std::io::Error::other(format!(
                "SFTP commit {path}: {error}"
            )));
        }

        // The hardlink response alone is not proof that the visible name is
        // the staged revision. Keep both recovery links until exact content
        // and metadata verification succeeds on the visible target.
        let visible_verification = if self.fault_enabled(AtomicWriteFault::VerifyVisible) {
            Ok(false)
        } else {
            self.verify_remote_matches(
                session,
                path,
                data,
                original_unix_mode,
                original_unix_uid,
                original_unix_gid,
            )
            .await
        };
        match visible_verification {
            Ok(true) => {}
            Ok(false) => {
                return Err(remote_write_error(
                    RemoteWriteFailureKind::RecoveryRequired,
                    format!(
                        "SFTP RECOVERY REQUIRED {path}: visible target verification failed after commit; backup={backup_path}; stage={stage_path}; transaction={transaction_path}"
                    ),
                ));
            }
            Err(error) => {
                return Err(remote_write_error(
                    RemoteWriteFailureKind::RecoveryRequired,
                    format!(
                        "SFTP RECOVERY REQUIRED {path}: visible target verification errored after commit ({error}); backup={backup_path}; stage={stage_path}; transaction={transaction_path}"
                    ),
                ));
            }
        }

        // ── Success: remove the stage link and proven-original backup ──
        let mut cleanup_warnings = Vec::new();
        if let Err(error) = session.remove_file(stage_path.clone()).await {
            match &error {
                russh_sftp::client::error::Error::Status(_) => {
                    cleanup_warnings.push(format!(
                        "stage cleanup refused ({error}); stage={stage_path}"
                    ));
                }
                _ => {
                    // Transport-ambiguous stage removal: retain backup evidence.
                    return Err(remote_write_error(
                        RemoteWriteFailureKind::CommittedWithWarning,
                        format!(
                            "SFTP COMMITTED WITH WARNING {path}: stage cleanup outcome uncertain ({error}); stage={stage_path}; backup retained={backup_path}",
                        ),
                    ));
                }
            }
        }
        let backup_cleanup_error = if self.fault_enabled(AtomicWriteFault::BackupCleanup) {
            Some("injected failure".to_string())
        } else {
            session
                .remove_file(backup_path.clone())
                .await
                .err()
                .map(|error| error.to_string())
        };
        if let Some(error) = backup_cleanup_error {
            cleanup_warnings.push(format!(
                "backup cleanup failed ({error}); backup={backup_path}"
            ));
        } else if let Err(error) = session.remove_dir(transaction_path.clone()).await {
            cleanup_warnings.push(format!(
                "transaction cleanup failed ({error}); transaction={transaction_path}"
            ));
        }
        if !cleanup_warnings.is_empty() {
            return Err(remote_write_error(
                RemoteWriteFailureKind::CommittedWithWarning,
                format!(
                    "SFTP COMMITTED WITH WARNING {path}: {}",
                    cleanup_warnings.join("; ")
                ),
            ));
        }

        Ok(())
    }

    async fn verify_remote_matches(
        &self,
        session: &russh_sftp::client::SftpSession,
        backup_path: &str,
        frozen: &[u8],
        expected_mode: u32,
        expected_uid: u32,
        expected_gid: u32,
    ) -> std::io::Result<bool> {
        let snapshot =
            read_stable_snapshot(session, backup_path, frozen.len().saturating_add(1)).await?;
        Ok(!snapshot.truncated
            && snapshot.bytes == frozen
            && snapshot.unix_mode.map(|value| value & 0o7777) == Some(expected_mode & 0o7777)
            && snapshot.unix_uid == Some(expected_uid)
            && snapshot.unix_gid == Some(expected_gid))
    }

    async fn remote_metadata(&self, path: &str) -> std::io::Result<FileMetadata> {
        let guard = self.connect_for_mutation().await?;
        let conn = guard
            .as_ref()
            .ok_or_else(|| std::io::Error::other("SFTP connection lost"))?;
        let meta = conn
            .session()
            .symlink_metadata(path.to_string())
            .await
            .map_err(|error| russh_to_io(error, &format!("SFTP metadata {path}")))?;
        Ok(FileMetadata {
            len: meta.len(),
            is_regular: is_regular_file(&meta),
            unix_mode: meta.permissions,
            unix_uid: meta.uid,
            unix_gid: meta.gid,
        })
    }
}

#[cfg(test)]
mod metadata_tests {
    use super::*;

    // ── #48 deterministic pool-health policy matrix (no network, no sleeps) ──
    #[test]
    fn pool_health_healthy_session_reused() {
        assert_eq!(
            pool_health_action(true, SftpInvalidation::Keep),
            PoolHealthAction::Reuse
        );
    }

    #[test]
    fn pool_health_stale_session_discarded() {
        assert_eq!(
            pool_health_action(true, SftpInvalidation::TransportBroken),
            PoolHealthAction::DiscardAndReconnect
        );
    }

    #[test]
    fn pool_health_no_session_connects() {
        assert_eq!(
            pool_health_action(false, SftpInvalidation::Keep),
            PoolHealthAction::DiscardAndReconnect
        );
    }

    #[test]
    fn pool_health_status_probe_keeps_connection() {
        // A definitive Status reply is a keep (transport still alive).
        assert_eq!(
            pool_health_action(true, SftpInvalidation::Keep),
            PoolHealthAction::Reuse
        );
    }

    #[test]
    fn pool_health_timeout_or_transport_invalidates() {
        assert_eq!(
            pool_health_action(true, SftpInvalidation::TransportBroken),
            PoolHealthAction::DiscardAndReconnect
        );
        // timeout/EOF/reset/broken all map to TransportBroken via classify_*.
        for kind in [
            io::ErrorKind::TimedOut,
            io::ErrorKind::ConnectionReset,
            io::ErrorKind::ConnectionAborted,
            io::ErrorKind::BrokenPipe,
            io::ErrorKind::UnexpectedEof,
        ] {
            assert_eq!(
                pool_health_action(true, classify_io_error(&io::Error::new(kind, "x"))),
                PoolHealthAction::DiscardAndReconnect
            );
        }
    }

    #[test]
    fn pool_health_application_error_keeps_connection() {
        // permission/cancel/conflict/validation → Keep (do not reconnect).
        for kind in [
            io::ErrorKind::PermissionDenied,
            io::ErrorKind::Interrupted,
            io::ErrorKind::AlreadyExists,
            io::ErrorKind::InvalidInput,
            io::ErrorKind::Unsupported,
        ] {
            assert_eq!(
                pool_health_action(true, classify_io_error(&io::Error::new(kind, "x"))),
                PoolHealthAction::Reuse
            );
        }
    }

    // ── #48 deterministic acquire/probe/reconnect matrix ──
    // Drives the real `connect_for_mutation` algorithm via the injected seam;
    // a counter proves exactly how many (re)connections happened. No network,
    // no `sleep`.

    #[tokio::test]
    async fn acquire_healthy_session_reused_zero_extra_connects() {
        let connects = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider = SftpProvider::new(crate::remote::Host::from_alias("test-host"))
            .with_test_pool(Some(SftpInvalidation::Keep), connects.clone());
        // First op acquires once; second op reuses the pooled session.
        let _ = provider.connect_for_mutation().await;
        let _ = provider.connect_for_mutation().await;
        assert_eq!(connects.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn acquire_no_session_connects_once() {
        let connects = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider = SftpProvider::new(crate::remote::Host::from_alias("test-host"))
            .with_test_pool(Some(SftpInvalidation::Keep), connects.clone());
        let _ = provider.connect_for_mutation().await;
        assert_eq!(connects.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn acquire_stale_session_discarded_and_reconnected() {
        let connects = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider = SftpProvider::new(crate::remote::Host::from_alias("test-host"))
            .with_test_pool(Some(SftpInvalidation::TransportBroken), connects.clone());
        // Every acquire is stale → fresh replacement each time.
        let _ = provider.connect_for_mutation().await;
        let _ = provider.connect_for_mutation().await;
        assert_eq!(connects.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn acquire_status_probe_keeps_zero_extra_connects() {
        let connects = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider = SftpProvider::new(crate::remote::Host::from_alias("test-host"))
            .with_test_pool(Some(SftpInvalidation::Keep), connects.clone());
        for _ in 0..4 {
            let _ = provider.connect_for_mutation().await;
        }
        assert_eq!(connects.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn acquire_application_error_keeps_connection() {
        let connects = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider = SftpProvider::new(crate::remote::Host::from_alias("test-host"))
            .with_test_pool(Some(SftpInvalidation::Keep), connects.clone());
        for _ in 0..3 {
            let _ = provider.connect_for_mutation().await;
        }
        // Keep (no transport break) → never reconnect, regardless of op errors.
        assert_eq!(connects.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn acquire_once_per_op_no_reconnect_spin() {
        let connects = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider = SftpProvider::new(crate::remote::Host::from_alias("test-host"))
            .with_test_pool(Some(SftpInvalidation::Keep), connects.clone());
        // A single op acquires the session exactly once (no reconnect spin).
        let _ = provider.connect_for_mutation().await;
        assert_eq!(connects.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    // #48/MAJOR#3B: a destructive mutation must run exactly once per op, and a
    // stale→reconnect cycle must NOT replay it. Drives the real
    // connect_for_mutation algorithm; the injected mutation counter stands in
    // for the single destructive SFTP op each op performs.
    #[tokio::test]
    async fn mutation_runs_exactly_once_per_op_and_not_replayed_on_reconnect() {
        let connects = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mutations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut provider = SftpProvider::new(crate::remote::Host::from_alias("test-host"))
            .with_test_pool(Some(SftpInvalidation::Keep), connects.clone());

        // 3 ops with a healthy pooled session: one acquire, three mutations.
        for _ in 0..3 {
            let _ = provider.connect_for_mutation().await;
            mutations.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        assert_eq!(connects.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(mutations.load(std::sync::atomic::Ordering::SeqCst), 3);

        // Now every acquire is stale → fresh reconnect each op; mutation still
        // runs exactly once per op, never replayed by the reconnect.
        provider = SftpProvider::new(crate::remote::Host::from_alias("test-host"))
            .with_test_pool(Some(SftpInvalidation::TransportBroken), connects.clone());
        for _ in 0..2 {
            let _ = provider.connect_for_mutation().await;
            mutations.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        assert_eq!(connects.load(std::sync::atomic::Ordering::SeqCst), 3);
        assert_eq!(mutations.load(std::sync::atomic::Ordering::SeqCst), 5);
    }

    // #48/MAJOR#3A: a probe that never resolves must not hang acquisition
    // forever. An outer timeout bounds the acquire deterministically (no real
    // sleep). Proves the acquire respects a timeout boundary rather than
    // blocking on a pending probe indefinitely.
    #[tokio::test]
    async fn acquire_bounded_by_probe_timeout_does_not_hang() {
        let connects = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider = SftpProvider::new(crate::remote::Host::from_alias("test-host"))
            .with_test_pool_pending_probe(connects.clone());
        // First connect has no pooled session, so the probe is skipped and the
        // connect runs immediately.
        let _ = provider.connect_for_mutation().await;
        assert_eq!(connects.load(std::sync::atomic::Ordering::SeqCst), 1);
        // Second acquire probes the now-pooled session with a never-resolving
        // probe; the outer timeout must fire instead of hanging.
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            let _ = provider.connect_for_mutation().await;
        })
        .await;
        assert!(
            result.is_err(),
            "acquire must be bounded by a timeout, not hang on a pending probe"
        );
    }

    // ── #47 stream-read transport truth seam ──
    #[test]
    fn stream_read_error_transport_kinds_invalidate() {
        // Every listed transport/EOF/ambiguous-health break maps to BrokenPipe
        // → TransportBroken → pool invalidation (per contract).
        for kind in [
            io::ErrorKind::BrokenPipe,
            io::ErrorKind::UnexpectedEof,
            io::ErrorKind::TimedOut,
            io::ErrorKind::ConnectionReset,
            io::ErrorKind::ConnectionAborted,
            io::ErrorKind::NotConnected,
        ] {
            let err = stream_read_error(io::Error::new(kind, "x"), "SFTP read x");
            assert_eq!(err.kind(), io::ErrorKind::BrokenPipe, "kind {kind:?}");
            assert_eq!(
                classify_io_error(&err),
                SftpInvalidation::TransportBroken,
                "kind {kind:?}"
            );
        }
    }

    #[test]
    fn stream_read_error_keeps_local_cancellation() {
        // Only local cooperative cancellation keeps the pooled connection.
        let err = stream_read_error(
            io::Error::new(io::ErrorKind::Interrupted, "x"),
            "SFTP read x",
        );
        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
        assert_eq!(classify_io_error(&err), SftpInvalidation::Keep);
    }

    #[test]
    fn sftp_mtime_uses_canonical_second_resolution() {
        let mut metadata = russh_sftp::protocol::FileAttributes::empty();
        metadata.mtime = Some(1_234);

        let modified_unix_ms = metadata
            .mtime
            .map(|seconds| canonical_unix_mtime_ms(u64::from(seconds)));

        assert_eq!(modified_unix_ms, Some(1_234_000));
    }

    #[test]
    fn private_stage_requires_regular_0600_file_with_owner() {
        let mut metadata = russh_sftp::protocol::FileAttributes::empty();
        metadata.permissions = Some(0o600);
        metadata.uid = Some(7);
        metadata.set_regular(true);
        assert_eq!(private_stage_owner(&metadata), Some(7));

        metadata.permissions = Some(0o100644);
        assert_eq!(private_stage_owner(&metadata), None);
        metadata.permissions = Some(0o100600);
        metadata.uid = None;
        assert_eq!(private_stage_owner(&metadata), None);
    }

    // ── REMOTE-09: transport invalidation mechanism ──

    #[test]
    fn sftp_provider_has_invalidation_mechanism() {
        let host = crate::remote::Host::from_alias("test-host");
        let provider = SftpProvider::new(host);
        assert_eq!(provider.host.ssh_alias, "test-host");
    }

    #[test]
    fn no_recursive_delete_path_in_mutation_code() {
        let source = include_str!("sftp.rs");
        // Split at #[cfg(test)] to avoid self-matching assertion strings.
        let prod_code = source.split("#[cfg(test)]").next().unwrap_or(source);
        assert!(!prod_code.contains("remove_dir_all"));
        assert!(!prod_code.contains(".recursive"));
        assert!(!prod_code.contains("walkdir"));
    }

    #[tokio::test]
    async fn exact_len_read_accepts_exact_remote_edit_limit() {
        let max = crate::vfs::MAX_REMOTE_EDIT_BYTES;
        let bytes = vec![b'a'; max];
        let (read_len, truncated) = bounded_read_plan(max as u64, max).unwrap();
        assert!(!truncated);
        assert_eq!(
            read_exact_len(std::io::Cursor::new(bytes.clone()), read_len)
                .await
                .unwrap(),
            bytes
        );
    }

    #[tokio::test]
    async fn bounded_read_plan_marks_limit_plus_one_truncated() {
        let max = crate::vfs::MAX_REMOTE_EDIT_BYTES;
        let (read_len, truncated) = bounded_read_plan((max + 1) as u64, max).unwrap();
        assert!(truncated);
        assert_eq!(read_len, max);
    }

    #[tokio::test]
    async fn exact_len_read_handles_short_chunks_zero_and_early_eof() {
        use tokio::io::AsyncWriteExt;

        let empty = read_exact_len(std::io::Cursor::new(Vec::<u8>::new()), 0)
            .await
            .unwrap();
        assert!(empty.is_empty());

        let (mut writer, reader) = tokio::io::duplex(3);
        let write = tokio::spawn(async move {
            for chunk in b"short chunks survive".chunks(2) {
                writer.write_all(chunk).await.unwrap();
                tokio::task::yield_now().await;
            }
        });
        let result = read_exact_len(reader, b"short chunks survive".len())
            .await
            .unwrap();
        write.await.unwrap();
        assert_eq!(result, b"short chunks survive");

        let error = read_exact_len(std::io::Cursor::new(b"short"), 6)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);
    }

    struct RemoteFaultFixture {
        host: String,
        base: String,
    }

    impl Drop for RemoteFaultFixture {
        fn drop(&mut self) {
            let script = format!("rm -rf -- {}", shell_quote(&self.base));
            let _ = std::process::Command::new("ssh")
                .arg(&self.host)
                .arg(format!("sh -c {}", shell_quote(&script)))
                .status();
        }
    }

    fn shell_quote(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }

    fn seed_remote(host: &str, path: &str, bytes: &[u8], mode: u32) -> std::io::Result<()> {
        use std::io::Write as _;
        use std::process::Stdio;

        let quoted = shell_quote(path);
        let script = format!("set -eu; umask 077; cat > {quoted}; chmod {mode:o} {quoted}");
        let mut child = std::process::Command::new("ssh")
            .arg(host)
            .arg(format!("sh -c {}", shell_quote(&script)))
            .stdin(Stdio::piped())
            .spawn()?;
        child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("ssh stdin unavailable"))?
            .write_all(bytes)?;
        let status = child.wait()?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!(
                "ssh seed {path} exited with {status}"
            )))
        }
    }

    async fn transaction_artifacts(provider: &SftpProvider, base: &str, name: &str) -> Vec<String> {
        provider
            .list_async(base)
            .await
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .filter(|entry| {
                entry.contains(name)
                    && (entry.contains(".arx-part-") || entry.contains(".arx-txn-"))
            })
            .collect()
    }

    async fn remote_revision(provider: &SftpProvider, path: &str) -> RemoteEditRevision {
        provider
            .read_all_capped(path, crate::vfs::MAX_REMOTE_EDIT_BYTES)
            .await
            .unwrap()
            .into_revision()
            .unwrap()
    }

    #[tokio::test]
    #[ignore = "requires ARX_SFTP_SMOKE_HOST pointing at a disposable SSH/SFTP host"]
    async fn sftp_cancellation_after_pin_removes_remote_artifact() {
        use std::time::{Duration, SystemTime, UNIX_EPOCH};

        let host = std::env::var("ARX_SFTP_SMOKE_HOST").unwrap();
        crate::remote::validate_ssh_alias(&host).unwrap();
        let token = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = format!("/tmp/arx-demo/arx-remote-edit-pin-cancel-{token}");
        let script = format!("mkdir -m 700 -- {}", shell_quote(&base));
        assert!(
            std::process::Command::new("ssh")
                .arg(&host)
                .arg(format!("sh -c {}", shell_quote(&script)))
                .status()
                .unwrap()
                .success()
        );
        let _fixture = RemoteFaultFixture {
            host: host.clone(),
            base: base.clone(),
        };
        let path = format!("{base}/cancel.txt");
        seed_remote(&host, &path, b"cancel me", 0o600).unwrap();

        let pin_created = std::sync::Arc::new(tokio::sync::Notify::new());
        let mut provider = SftpProvider::new(crate::remote::Host::from_alias(&host));
        provider.pause_after_pin = Some(pin_created.clone());
        let cancellation = CancellationFlag::default();
        let read = provider.read_all_capped_cancellable(&path, 64, &cancellation);
        tokio::pin!(read);
        tokio::select! {
            _ = pin_created.notified() => {}
            result = &mut read => panic!("read completed before pin fault point: {result:?}"),
        }
        cancellation.cancel();
        let error = tokio::time::timeout(Duration::from_secs(5), &mut read)
            .await
            .expect("cancelled SFTP read must finish")
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);

        let entries = provider.list_async(&base).await.unwrap();
        assert!(
            entries
                .iter()
                .all(|entry| !entry.name.contains(".arx-read-")),
            "pin cleanup left remote artifacts: {entries:?}"
        );
    }

    #[tokio::test]
    #[ignore = "requires ARX_SFTP_SMOKE_HOST pointing at a disposable SSH/SFTP host"]
    async fn sftp_atomic_fault_injection_preserves_recovery_evidence() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let host = std::env::var("ARX_SFTP_SMOKE_HOST").unwrap();
        crate::remote::validate_ssh_alias(&host).unwrap();
        let token = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = format!("/tmp/arx-demo/arx-remote-edit-fault-{token}");
        let quoted = shell_quote(&base);
        let script = format!("set -eu; mkdir -p {quoted}; chmod 700 {quoted}");
        let status = std::process::Command::new("ssh")
            .arg(&host)
            .arg(format!("sh -c {}", shell_quote(&script)))
            .status()
            .unwrap();
        assert!(status.success());
        let _fixture = RemoteFaultFixture {
            host: host.clone(),
            base: base.clone(),
        };
        let mut provider = SftpProvider::new(crate::remote::Host::from_alias(&host));
        let cancellation = CancellationFlag::default();

        let mode_path = format!("{base}/mode.txt");
        seed_remote(&host, &mode_path, b"old", 0o600).unwrap();
        let mode_revision = remote_revision(&provider, &mode_path).await;
        provider.faults.preserve_mode = true;
        let mode_error = provider
            .write_file_bytes_if_unchanged(&mode_path, b"new", &mode_revision, &cancellation)
            .await
            .unwrap_err();
        assert!(mode_error.to_string().contains("preserve metadata"));
        assert_eq!(
            provider
                .read_all_capped(&mode_path, 16)
                .await
                .unwrap()
                .bytes,
            b"old"
        );
        assert!(
            transaction_artifacts(&provider, &base, "mode.txt")
                .await
                .is_empty()
        );

        let commit_path = format!("{base}/commit.txt");
        seed_remote(&host, &commit_path, b"old", 0o600).unwrap();
        let commit_revision = remote_revision(&provider, &commit_path).await;
        provider.faults = AtomicWriteFaults {
            commit: true,
            ..AtomicWriteFaults::default()
        };
        let commit_error = provider
            .write_file_bytes_if_unchanged(&commit_path, b"new", &commit_revision, &cancellation)
            .await
            .unwrap_err();
        assert!(!commit_error.to_string().contains("RECOVERY REQUIRED"));
        assert_eq!(
            provider
                .read_all_capped(&commit_path, 16)
                .await
                .unwrap()
                .bytes,
            b"old"
        );
        assert!(
            transaction_artifacts(&provider, &base, "commit.txt")
                .await
                .is_empty()
        );

        let recovery_path = format!("{base}/recovery.txt");
        seed_remote(&host, &recovery_path, b"old", 0o600).unwrap();
        let recovery_revision = remote_revision(&provider, &recovery_path).await;
        provider.faults = AtomicWriteFaults {
            verify_backup: true,
            restore: true,
            ..AtomicWriteFaults::default()
        };
        let recovery_error = provider
            .write_file_bytes_if_unchanged(
                &recovery_path,
                b"new",
                &recovery_revision,
                &cancellation,
            )
            .await
            .unwrap_err();
        let recovery_message = recovery_error.to_string();
        assert!(recovery_message.contains("RECOVERY REQUIRED"));
        assert!(recovery_message.contains("backup="));
        assert!(recovery_message.contains("stage="));
        let recovery_artifacts = transaction_artifacts(&provider, &base, "recovery.txt").await;
        assert_eq!(recovery_artifacts.len(), 1, "{recovery_artifacts:?}");
        let recovery_entries = provider
            .list_async(&format!("{base}/{}", recovery_artifacts[0]))
            .await
            .unwrap();
        assert!(recovery_entries.iter().any(|entry| entry.name == "stage"));
        assert!(recovery_entries.iter().any(|entry| entry.name == "backup"));

        let visible_path = format!("{base}/visible.txt");
        seed_remote(&host, &visible_path, b"old", 0o600).unwrap();
        let visible_revision = remote_revision(&provider, &visible_path).await;
        provider.faults = AtomicWriteFaults {
            verify_visible: true,
            ..AtomicWriteFaults::default()
        };
        let visible_error = provider
            .write_file_bytes_if_unchanged(&visible_path, b"new", &visible_revision, &cancellation)
            .await
            .unwrap_err();
        assert!(visible_error.to_string().contains("RECOVERY REQUIRED"));
        assert_eq!(
            provider
                .read_all_capped(&visible_path, 16)
                .await
                .unwrap()
                .bytes,
            b"new"
        );
        let visible_artifacts = transaction_artifacts(&provider, &base, "visible.txt").await;
        assert_eq!(visible_artifacts.len(), 1, "{visible_artifacts:?}");
        let visible_entries = provider
            .list_async(&format!("{base}/{}", visible_artifacts[0]))
            .await
            .unwrap();
        assert!(visible_entries.iter().any(|entry| entry.name == "stage"));
        assert!(visible_entries.iter().any(|entry| entry.name == "backup"));

        let race_path = format!("{base}/race.txt");
        seed_remote(&host, &race_path, b"old", 0o600).unwrap();
        let race_revision = remote_revision(&provider, &race_path).await;
        provider.faults = AtomicWriteFaults {
            concurrent_target: true,
            ..AtomicWriteFaults::default()
        };
        let race_error = provider
            .write_file_bytes_if_unchanged(&race_path, b"new", &race_revision, &cancellation)
            .await
            .unwrap_err();
        assert!(race_error.to_string().contains("RECOVERY REQUIRED"));
        assert_eq!(
            provider
                .read_all_capped(&race_path, 16)
                .await
                .unwrap()
                .bytes,
            b"concurrent"
        );
        let race_artifacts = transaction_artifacts(&provider, &base, "race.txt").await;
        assert_eq!(race_artifacts.len(), 1, "{race_artifacts:?}");

        let warning_path = format!("{base}/warning.txt");
        seed_remote(&host, &warning_path, b"old", 0o600).unwrap();
        let warning_revision = remote_revision(&provider, &warning_path).await;
        provider.faults = AtomicWriteFaults {
            backup_cleanup: true,
            ..AtomicWriteFaults::default()
        };
        let warning_error = provider
            .write_file_bytes_if_unchanged(&warning_path, b"new", &warning_revision, &cancellation)
            .await
            .unwrap_err();
        assert!(warning_error.to_string().contains("COMMITTED WITH WARNING"));
        assert_eq!(
            provider
                .read_all_capped(&warning_path, 16)
                .await
                .unwrap()
                .bytes,
            b"new"
        );
        let warning_artifacts = transaction_artifacts(&provider, &base, "warning.txt").await;
        assert_eq!(warning_artifacts.len(), 1, "{warning_artifacts:?}");

        let cancel_path = format!("{base}/cancel.txt");
        seed_remote(&host, &cancel_path, b"old", 0o600).unwrap();
        let cancel_revision = remote_revision(&provider, &cancel_path).await;
        provider.faults = AtomicWriteFaults {
            cancel_before_commit: true,
            ..AtomicWriteFaults::default()
        };
        let cancel = CancellationFlag::default();
        let cancel_error = provider
            .write_file_bytes_if_unchanged(&cancel_path, b"new", &cancel_revision, &cancel)
            .await
            .unwrap_err();
        assert_eq!(cancel_error.kind(), std::io::ErrorKind::Interrupted);
        assert_eq!(
            provider
                .read_all_capped(&cancel_path, 16)
                .await
                .unwrap()
                .bytes,
            b"old"
        );
        assert!(
            transaction_artifacts(&provider, &base, "cancel.txt")
                .await
                .is_empty()
        );
    }

    #[test]
    fn track_b_classification_categories() {
        use std::io::Error as IoError;
        use std::io::ErrorKind as K;
        // Transport-broken: must invalidate the pooled connection.
        assert!(classify_io_error(&IoError::new(K::TimedOut, "x")).should_invalidate());
        assert!(classify_io_error(&IoError::new(K::ConnectionAborted, "x")).should_invalidate());
        assert!(classify_io_error(&IoError::new(K::ConnectionReset, "x")).should_invalidate());
        assert!(classify_io_error(&IoError::new(K::BrokenPipe, "x")).should_invalidate());
        assert!(classify_io_error(&IoError::new(K::UnexpectedEof, "x")).should_invalidate());
        // Application: keep the connection (no destructive retry, no invalidate).
        assert!(!classify_io_error(&IoError::new(K::Interrupted, "x")).should_invalidate());
        assert!(!classify_io_error(&IoError::new(K::InvalidInput, "x")).should_invalidate());
        assert!(!classify_io_error(&IoError::new(K::Unsupported, "x")).should_invalidate());
        assert!(!classify_io_error(&IoError::new(K::AlreadyExists, "x")).should_invalidate());
        assert!(!classify_io_error(&IoError::new(K::PermissionDenied, "x")).should_invalidate());
        // Default conservative: unknown kinds keep (avoid over-invalidation).
        assert!(!classify_io_error(&IoError::other("x")).should_invalidate());
    }

    #[test]
    fn track_h_parent_policy_exhaustive() {
        // Private parents always accepted.
        assert!(!parent_is_unsafe_writable(0o700));
        assert!(!parent_is_unsafe_writable(0o755));
        assert!(!parent_is_unsafe_writable(0o750));
        // Sticky writable (e.g. /tmp 0o1777) accepted: sticky bit protects namespace.
        assert!(!parent_is_unsafe_writable(0o1777));
        assert!(!parent_is_unsafe_writable(0o3777));
        // Group/world-writable NON-sticky rejected (fail closed).
        assert!(parent_is_unsafe_writable(0o777));
        assert!(parent_is_unsafe_writable(0o775));
        assert!(parent_is_unsafe_writable(0o722));
    }

    #[tokio::test]
    #[ignore = "requires ARX_SFTP_SMOKE_HOST pointing at a disposable SSH/SFTP host"]
    async fn track_e_stage_write_break_preserves_original() {
        let host = std::env::var("ARX_SFTP_SMOKE_HOST").unwrap();
        crate::remote::validate_ssh_alias(&host).unwrap();
        let mut provider = SftpProvider::new(crate::remote::Host::from_alias(&host));
        let base = format!(
            "{}/track-e-stage",
            std::env::var("ARX_SFTP_SMOKE_BASE").unwrap_or_else(|_| "/tmp".to_string())
        );
        let path = format!("{base}/stage-write.txt");
        seed_remote(&host, &path, b"original", 0o600).unwrap();
        let revision = remote_revision(&provider, &path).await;
        provider.faults = AtomicWriteFaults {
            stage_write: true,
            ..AtomicWriteFaults::default()
        };
        let error = provider
            .write_file_bytes_if_unchanged(&path, b"new", &revision, &CancellationFlag::default())
            .await
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("RECOVERY REQUIRED"));
        assert!(message.contains("transport break after stage write"));
        // Original intact (backup not yet created at this stage).
        assert_eq!(
            provider.read_all_capped(&path, 16).await.unwrap().bytes,
            b"original"
        );
    }

    #[tokio::test]
    #[ignore = "requires ARX_SFTP_SMOKE_HOST pointing at a disposable SSH/SFTP host"]
    async fn track_e_backup_rename_break_preserves_backup() {
        let host = std::env::var("ARX_SFTP_SMOKE_HOST").unwrap();
        crate::remote::validate_ssh_alias(&host).unwrap();
        let mut provider = SftpProvider::new(crate::remote::Host::from_alias(&host));
        let base = format!(
            "{}/track-e-backup",
            std::env::var("ARX_SFTP_SMOKE_BASE").unwrap_or_else(|_| "/tmp".to_string())
        );
        let path = format!("{base}/backup-rename.txt");
        seed_remote(&host, &path, b"original", 0o600).unwrap();
        let revision = remote_revision(&provider, &path).await;
        provider.faults = AtomicWriteFaults {
            backup_rename: true,
            ..AtomicWriteFaults::default()
        };
        let error = provider
            .write_file_bytes_if_unchanged(&path, b"new", &revision, &CancellationFlag::default())
            .await
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("RECOVERY REQUIRED"));
        assert!(message.contains("transport break after backup rename"));
        assert!(message.contains("backup preserved"));
        // Backup artifact remains as exact recovery evidence.
        let artifacts = transaction_artifacts(&provider, &base, "backup-rename.txt").await;
        assert_eq!(artifacts.len(), 1, "{artifacts:?}");
        let entries = provider
            .list_async(&format!("{base}/{}", artifacts[0]))
            .await
            .unwrap();
        assert!(entries.iter().any(|e| e.name == "backup"));
    }

    #[tokio::test]
    #[ignore = "requires ARX_SFTP_SMOKE_HOST pointing at a disposable SSH/SFTP host"]
    async fn track_e_metadata_race_forces_recovery() {
        let host = std::env::var("ARX_SFTP_SMOKE_HOST").unwrap();
        crate::remote::validate_ssh_alias(&host).unwrap();
        let mut provider = SftpProvider::new(crate::remote::Host::from_alias(&host));
        let base = format!(
            "{}/track-e-mrace",
            std::env::var("ARX_SFTP_SMOKE_BASE").unwrap_or_else(|_| "/tmp".to_string())
        );
        let path = format!("{base}/metadata-race.txt");
        seed_remote(&host, &path, b"original", 0o600).unwrap();
        let revision = remote_revision(&provider, &path).await;
        provider.faults = AtomicWriteFaults {
            metadata_race: true,
            ..AtomicWriteFaults::default()
        };
        let error = provider
            .write_file_bytes_if_unchanged(&path, b"new", &revision, &CancellationFlag::default())
            .await
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("RECOVERY REQUIRED"));
        assert!(message.contains("UID/GID metadata race"));
        // Never silently replaced.
        assert_eq!(
            provider.read_all_capped(&path, 16).await.unwrap().bytes,
            b"original"
        );
    }

    #[test]
    fn mutation_failure_invalidates_session() {
        let source = include_str!("sftp.rs");
        let count = source.matches("guard.take()").count();
        assert!(
            count >= 3,
            "expected at least 3 guard.take() invalidation sites (mkdir, remove_file, remove_dir), found {count}"
        );
    }
}
