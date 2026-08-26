use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use russh_sftp::client::error::Error as SftpError;
use russh_sftp::protocol::StatusCode;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::remote::openssh_sftp::OpenSshSftpConnection;
use crate::vfs::Location;

use crate::transfer::executor::{TransferExecutionError, TransferOutcome};
use crate::transfer::{TransferIntent, TransferMethod, TransferPlan};
use crate::transfer_queue::{PauseGate, RetryDisposition, TypedTransferProgress};

const COPY_BUFFER_SIZE: usize = 64 * 1024;

pub(crate) async fn execute_sftp_copy(
    plan: &TransferPlan,
    names: &[String],
    cancel: Arc<AtomicBool>,
    pause: PauseGate,
    on_progress: &mut impl FnMut(TypedTransferProgress),
) -> Result<TransferOutcome, TransferExecutionError> {
    if plan.intent != TransferIntent::Copy {
        return Err(invalid(
            "SFTP Move requires a separate copy/verify/delete-source transaction",
        ));
    }

    if let (
        Location::Sftp {
            host: source_host,
            path: source_dir,
        },
        Location::Sftp {
            host: destination_host,
            path: destination_dir,
        },
    ) = (&plan.source, &plan.destination)
    {
        return execute_sftp_remote_copy(
            source_host,
            source_dir,
            destination_host,
            destination_dir,
            names,
            cancel,
            pause,
            on_progress,
        )
        .await;
    }

    let (host, direction) = match (&plan.source, &plan.destination) {
        (Location::Local(src), Location::Sftp { host, path }) => (
            host.as_str(),
            Direction::Upload {
                src,
                remote_dir: path,
            },
        ),
        (Location::Sftp { host, path }, Location::Local(dst)) => (
            host.as_str(),
            Direction::Download {
                remote_dir: path,
                dst,
            },
        ),
        _ => {
            return Err(invalid(
                "SFTP copy requires exactly one local and one SFTP endpoint",
            ));
        }
    };

    let connection = OpenSshSftpConnection::connect(host).await?;
    let total = names.len();
    let mut completed = 0;
    let mut cumulative_written = 0;
    let mut total_bytes = Some(0_u64);

    for name in names {
        validate_name(name)?;
        let size = match direction {
            Direction::Upload { src, .. } => tokio::fs::metadata(src.join(name))
                .await
                .map_err(TransferExecutionError::safe_to_retry)?
                .len(),
            Direction::Download { remote_dir, .. } => connection
                .session()
                .metadata(remote_join(remote_dir, name))
                .await
                .map_err(|error| phase_sftp(name, error, RetryDisposition::SafeToRetry))?
                .len(),
        };
        total_bytes = total_bytes.and_then(|total| total.checked_add(size));
    }

    let result = async {
        for name in names {
            validate_name(name)?;
            check_cancelled(&cancel, completed)?;
            match direction {
                Direction::Upload { src, remote_dir } => {
                    upload_file(
                        &connection,
                        src,
                        remote_dir,
                        name,
                        &cancel,
                        &pause,
                        completed,
                        &mut cumulative_written,
                        total_bytes,
                        on_progress,
                    )
                    .await?;
                }
                Direction::Download { remote_dir, dst } => {
                    download_file(
                        &connection,
                        remote_dir,
                        dst,
                        name,
                        &cancel,
                        &pause,
                        completed,
                        &mut cumulative_written,
                        total_bytes,
                        on_progress,
                    )
                    .await?;
                }
            }

            completed += 1;
        }
        Ok(TransferOutcome { completed, total })
    }
    .await;

    // Always attempt protocol shutdown, including error/cancel paths.
    let _ = connection.close().await;
    result
}

#[derive(Clone, Copy)]
enum Direction<'a> {
    Upload { src: &'a Path, remote_dir: &'a str },
    Download { remote_dir: &'a str, dst: &'a Path },
}

#[allow(clippy::too_many_arguments)]
async fn execute_sftp_remote_copy(
    source_host: &str,
    source_dir: &str,
    destination_host: &str,
    destination_dir: &str,
    names: &[String],
    cancel: Arc<AtomicBool>,
    pause: PauseGate,
    on_progress: &mut impl FnMut(TypedTransferProgress),
) -> Result<TransferOutcome, TransferExecutionError> {
    let source = OpenSshSftpConnection::connect(source_host)
        .await
        .map_err(TransferExecutionError::safe_to_retry)?;
    let destination = match OpenSshSftpConnection::connect(destination_host).await {
        Ok(connection) => connection,
        Err(error) => {
            let _ = source.close().await;
            return Err(TransferExecutionError::safe_to_retry(error));
        }
    };

    let total = names.len();
    let mut completed = 0usize;
    let mut cumulative_written = 0u64;
    let mut total_bytes = Some(0u64);

    let result = async {
        for name in names {
            validate_name(name)?;
            let path = remote_join(source_dir, name);
            let metadata = source
                .session()
                .symlink_metadata(path)
                .await
                .map_err(|error| phase_sftp(name, error, RetryDisposition::SafeToRetry))?;
            if !metadata.is_regular() {
                return Err(invalid(
                    "SFTP remote-to-remote copy supports regular files only",
                ));
            }
            total_bytes = total_bytes.and_then(|total| total.checked_add(metadata.len()));
        }

        for name in names {
            validate_name(name)?;
            check_cancelled(&cancel, completed)?;
            copy_remote_file(
                &source,
                source_dir,
                &destination,
                destination_dir,
                name,
                &cancel,
                &pause,
                completed,
                &mut cumulative_written,
                total_bytes,
                on_progress,
            )
            .await?;
            completed += 1;
        }

        Ok(TransferOutcome { completed, total })
    }
    .await;

    let _ = source.close().await;
    let _ = destination.close().await;
    result
}

#[allow(clippy::too_many_arguments)]
async fn copy_remote_file(
    source_connection: &OpenSshSftpConnection,
    source_dir: &str,
    destination_connection: &OpenSshSftpConnection,
    destination_dir: &str,
    name: &str,
    cancel: &AtomicBool,
    pause: &PauseGate,
    completed: usize,
    cumulative_written: &mut u64,
    total_bytes: Option<u64>,
    on_progress: &mut impl FnMut(TypedTransferProgress),
) -> Result<(), TransferExecutionError> {
    let source_path = remote_join(source_dir, name);
    let source_before = source_connection
        .session()
        .symlink_metadata(source_path.clone())
        .await
        .map_err(|error| phase_sftp(name, error, RetryDisposition::SafeToRetry))?;
    if !source_before.is_regular() {
        return Err(invalid(
            "SFTP remote-to-remote copy supports regular files only",
        ));
    }

    let target = remote_join(destination_dir, name);
    let token = operation_token();
    let temp = format!("{target}.arx-part-{token}");
    let backup = format!("{target}.arx-bak-{token}");

    let mut remote_source = source_connection
        .session()
        .open(source_path.clone())
        .await
        .map_err(|error| phase_sftp(name, error, RetryDisposition::SafeToRetry))?;
    let mut remote_destination = match destination_connection.session().create(temp.clone()).await {
        Ok(file) => file,
        Err(error) => {
            return Err(clean_remote_stage(
                destination_connection,
                &temp,
                phase_sftp(name, error, RetryDisposition::SafeToRetry),
            )
            .await);
        }
    };

    if let Err(error) = copy_stream(
        &mut remote_source,
        &mut remote_destination,
        cancel,
        pause,
        completed,
        cumulative_written,
        total_bytes,
        on_progress,
    )
    .await
    {
        return Err(clean_remote_stage(destination_connection, &temp, error).await);
    }
    if let Err(error) = remote_destination.flush().await {
        return Err(clean_remote_stage(
            destination_connection,
            &temp,
            phase_err(error, RetryDisposition::SafeToRetry),
        )
        .await);
    }
    if let Err(error) = remote_destination.shutdown().await {
        return Err(clean_remote_stage(
            destination_connection,
            &temp,
            phase_err(error, RetryDisposition::SafeToRetry),
        )
        .await);
    }

    let source_after = match source_connection
        .session()
        .symlink_metadata(source_path)
        .await
    {
        Ok(metadata) => metadata,
        Err(error) => {
            return Err(clean_remote_stage(
                destination_connection,
                &temp,
                phase_sftp(name, error, RetryDisposition::SafeToRetry),
            )
            .await);
        }
    };
    if !source_after.is_regular()
        || source_after.size != source_before.size
        || source_after.mtime != source_before.mtime
    {
        return Err(clean_remote_stage(
            destination_connection,
            &temp,
            phase_err(
                io::Error::other("SFTP source changed during remote-to-remote copy"),
                RetryDisposition::SafeToRetry,
            ),
        )
        .await);
    }

    let staged = match destination_connection
        .session()
        .metadata(temp.clone())
        .await
    {
        Ok(metadata) => metadata,
        Err(error) => {
            return Err(clean_remote_stage(
                destination_connection,
                &temp,
                phase_sftp(name, error, RetryDisposition::SafeToRetry),
            )
            .await);
        }
    };
    if staged.len() != source_before.len() {
        return Err(clean_remote_stage(
            destination_connection,
            &temp,
            phase_err(
                io::Error::other("SFTP remote-to-remote staged size differs"),
                RetryDisposition::SafeToRetry,
            ),
        )
        .await);
    }
    if let Err(cancelled) = check_cancelled(cancel, completed) {
        return Err(clean_remote_stage(destination_connection, &temp, cancelled).await);
    }

    let had_target = match destination_connection
        .session()
        .symlink_metadata(target.clone())
        .await
    {
        Ok(metadata) if metadata.is_regular() => true,
        Ok(_) => {
            return Err(clean_remote_stage(
                destination_connection,
                &temp,
                invalid("SFTP remote-to-remote destination is not a regular file"),
            )
            .await);
        }
        Err(SftpError::Status(status)) if status.status_code == StatusCode::NoSuchFile => false,
        Err(error) => {
            return Err(clean_remote_stage(
                destination_connection,
                &temp,
                phase_sftp(name, error, RetryDisposition::SafeToRetry),
            )
            .await);
        }
    };

    if had_target
        && let Err(error) = destination_connection
            .session()
            .rename(target.clone(), backup.clone())
            .await
    {
        return Err(clean_remote_stage(
            destination_connection,
            &temp,
            phase_sftp(name, error, RetryDisposition::SafeToRetry),
        )
        .await);
    }

    if let Err(cancelled) = check_cancelled(cancel, completed) {
        if let Err(cleanup) = destination_connection
            .session()
            .remove_file(temp.clone())
            .await
        {
            return Err(phase_sftp(
                &temp,
                cleanup,
                RetryDisposition::RecoveryRequired,
            ));
        }
        if had_target
            && destination_connection
                .session()
                .rename(backup.clone(), target.clone())
                .await
                .is_err()
        {
            return Err(phase_err(
                io::Error::other("SFTP remote-to-remote cancellation rollback failed"),
                RetryDisposition::RecoveryRequired,
            ));
        }
        return Err(cancelled);
    }

    if let Err(error) = destination_connection
        .session()
        .rename(temp.clone(), target.clone())
        .await
    {
        let _ = destination_connection.session().remove_file(temp).await;
        if had_target
            && destination_connection
                .session()
                .rename(backup, target)
                .await
                .is_err()
        {
            return Err(phase_err(
                io::Error::other("SFTP remote-to-remote commit failed and rollback failed"),
                RetryDisposition::RecoveryRequired,
            ));
        }
        return Err(phase_sftp(name, error, RetryDisposition::AmbiguousMutation));
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn upload_file(
    connection: &OpenSshSftpConnection,
    src_dir: &Path,
    remote_dir: &str,
    name: &str,
    cancel: &AtomicBool,
    pause: &PauseGate,
    completed: usize,
    cumulative_written: &mut u64,
    total_bytes: Option<u64>,
    on_progress: &mut impl FnMut(TypedTransferProgress),
) -> Result<(), TransferExecutionError> {
    let local_path = src_dir.join(name);
    let local_meta = tokio::fs::metadata(&local_path)
        .await
        .map_err(|e| phase_err(e, RetryDisposition::SafeToRetry))?;
    if !local_meta.is_file() {
        return Err(invalid(
            "SFTP fallback currently supports regular files only",
        ));
    }

    let target = remote_join(remote_dir, name);
    let token = operation_token();
    let temp = format!("{target}.arx-part-{token}");
    let backup = format!("{target}.arx-bak-{token}");

    let mut local = tokio::fs::File::open(&local_path)
        .await
        .map_err(|e| phase_err(e, RetryDisposition::SafeToRetry))?;
    let mut remote = match connection.session().create(temp.clone()).await {
        Ok(remote) => remote,
        Err(error) => {
            return Err(clean_remote_stage(
                connection,
                &temp,
                phase_sftp(name, error, RetryDisposition::SafeToRetry),
            )
            .await);
        }
    };

    if let Err(error) = copy_stream(
        &mut local,
        &mut remote,
        cancel,
        pause,
        completed,
        cumulative_written,
        total_bytes,
        on_progress,
    )
    .await
    {
        return Err(clean_remote_stage(connection, &temp, error).await);
    }
    if let Err(error) = remote.flush().await {
        return Err(clean_remote_stage(
            connection,
            &temp,
            phase_err(error, RetryDisposition::SafeToRetry),
        )
        .await);
    }
    if let Err(error) = remote.shutdown().await {
        return Err(clean_remote_stage(
            connection,
            &temp,
            phase_err(error, RetryDisposition::SafeToRetry),
        )
        .await);
    }

    let staged = match connection.session().metadata(temp.clone()).await {
        Ok(staged) => staged,
        Err(error) => {
            return Err(clean_remote_stage(
                connection,
                &temp,
                phase_sftp(name, error, RetryDisposition::SafeToRetry),
            )
            .await);
        }
    };
    if staged.len() != local_meta.len() {
        return Err(clean_remote_stage(
            connection,
            &temp,
            phase_err(
                io::Error::other("SFTP upload verification failed: staged size differs"),
                RetryDisposition::SafeToRetry,
            ),
        )
        .await);
    }
    if let Err(cancelled) = check_cancelled(cancel, completed) {
        return Err(clean_remote_stage(connection, &temp, cancelled).await);
    }
    let had_target = match remote_exists(connection.session(), &target).await {
        Ok(exists) => exists,
        Err(error) => {
            return Err(clean_remote_stage(connection, &temp, error).await);
        }
    };
    if had_target
        && let Err(error) = connection
            .session()
            .rename(target.clone(), backup.clone())
            .await
    {
        return Err(clean_remote_stage(
            connection,
            &temp,
            phase_sftp(name, error, RetryDisposition::SafeToRetry),
        )
        .await);
    }
    // Cancellation after target→backup must roll the original target back
    // instead of committing the staged replacement.
    if let Err(cancelled) = check_cancelled(cancel, completed) {
        if let Err(cleanup) = connection.session().remove_file(temp.clone()).await {
            return Err(phase_sftp(
                &temp,
                cleanup,
                RetryDisposition::RecoveryRequired,
            ));
        }
        if had_target
            && connection
                .session()
                .rename(backup.clone(), target.clone())
                .await
                .is_err()
        {
            return Err(phase_err(
                io::Error::other("SFTP cancellation rollback failed"),
                RetryDisposition::RecoveryRequired,
            ));
        }
        return Err(cancelled);
    }
    if let Err(error) = connection
        .session()
        .rename(temp.clone(), target.clone())
        .await
    {
        let _ = connection.session().remove_file(temp).await;
        if had_target && connection.session().rename(backup, target).await.is_err() {
            return Err(phase_err(
                io::Error::other("SFTP commit failed and rollback failed"),
                RetryDisposition::RecoveryRequired,
            ));
        }
        return Err(phase_sftp(name, error, RetryDisposition::AmbiguousMutation));
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn download_file(
    connection: &OpenSshSftpConnection,
    remote_dir: &str,
    dst_dir: &Path,
    name: &str,
    cancel: &AtomicBool,
    pause: &PauseGate,
    completed: usize,
    cumulative_written: &mut u64,
    total_bytes: Option<u64>,
    on_progress: &mut impl FnMut(TypedTransferProgress),
) -> Result<(), TransferExecutionError> {
    let source = remote_join(remote_dir, name);
    let remote_meta = connection
        .session()
        .metadata(source.clone())
        .await
        .map_err(|error| phase_sftp(name, error, RetryDisposition::SafeToRetry))?;
    if !remote_meta.is_regular() {
        return Err(invalid(
            "SFTP fallback currently supports regular files only",
        ));
    }

    let token = operation_token();
    let target = dst_dir.join(name);
    let temp = dst_dir.join(format!(".arx-part-{token}"));
    let backup = path_with_suffix(&target, &format!(".arx-bak-{token}"));

    let mut remote = connection
        .session()
        .open(source)
        .await
        .map_err(|error| phase_sftp(name, error, RetryDisposition::SafeToRetry))?;
    let mut local = match tokio::fs::File::create(&temp).await {
        Ok(local) => local,
        Err(error) => {
            return Err(
                clean_local_stage(&temp, phase_err(error, RetryDisposition::SafeToRetry)).await,
            );
        }
    };

    if let Err(error) = copy_stream(
        &mut remote,
        &mut local,
        cancel,
        pause,
        completed,
        cumulative_written,
        total_bytes,
        on_progress,
    )
    .await
    {
        return Err(clean_local_stage(&temp, error).await);
    }
    if let Err(error) = local.flush().await {
        return Err(
            clean_local_stage(&temp, phase_err(error, RetryDisposition::SafeToRetry)).await,
        );
    }
    if let Err(error) = local.sync_all().await {
        return Err(
            clean_local_stage(&temp, phase_err(error, RetryDisposition::SafeToRetry)).await,
        );
    }

    let staged = match tokio::fs::metadata(&temp).await {
        Ok(staged) => staged,
        Err(error) => {
            return Err(
                clean_local_stage(&temp, phase_err(error, RetryDisposition::SafeToRetry)).await,
            );
        }
    };
    if staged.len() != remote_meta.len() {
        return Err(clean_local_stage(
            &temp,
            phase_err(
                io::Error::other("SFTP download verification failed: staged size differs"),
                RetryDisposition::SafeToRetry,
            ),
        )
        .await);
    }
    if let Err(cancelled) = check_cancelled(cancel, completed) {
        return Err(clean_local_stage(&temp, cancelled).await);
    }
    let had_target = match local_exists(&target).await {
        Ok(exists) => exists,
        Err(error) => {
            return Err(clean_local_stage(&temp, error).await);
        }
    };
    if had_target && let Err(error) = tokio::fs::rename(&target, &backup).await {
        return Err(
            clean_local_stage(&temp, phase_err(error, RetryDisposition::SafeToRetry)).await,
        );
    }
    if let Err(cancelled) = check_cancelled(cancel, completed) {
        if let Err(error) = tokio::fs::remove_file(&temp).await
            && error.kind() != io::ErrorKind::NotFound
        {
            return Err(phase_err(error, RetryDisposition::RecoveryRequired));
        }
        if had_target && tokio::fs::rename(&backup, &target).await.is_err() {
            return Err(phase_err(
                io::Error::other("SFTP cancellation rollback failed"),
                RetryDisposition::RecoveryRequired,
            ));
        }
        return Err(cancelled);
    }
    if let Err(error) = tokio::fs::rename(&temp, &target).await {
        let _ = tokio::fs::remove_file(&temp).await;
        if had_target && tokio::fs::rename(&backup, &target).await.is_err() {
            return Err(phase_err(
                io::Error::other("SFTP commit failed and rollback failed"),
                RetryDisposition::RecoveryRequired,
            ));
        }
        return Err(phase_err(error, RetryDisposition::AmbiguousMutation));
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn copy_stream<R, W>(
    reader: &mut R,
    writer: &mut W,
    cancel: &AtomicBool,
    pause: &PauseGate,
    completed: usize,
    cumulative_written: &mut u64,
    total_bytes: Option<u64>,
    on_progress: &mut impl FnMut(TypedTransferProgress),
) -> Result<(), TransferExecutionError>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut buffer = vec![0_u8; COPY_BUFFER_SIZE];
    loop {
        pause.checkpoint().await;
        check_cancelled(cancel, completed)?;
        let read = reader
            .read(&mut buffer)
            .await
            .map_err(|e| phase_err(e, RetryDisposition::SafeToRetry))?;
        if read == 0 {
            return Ok(());
        }
        writer
            .write_all(&buffer[..read])
            .await
            .map_err(|e| phase_err(e, RetryDisposition::SafeToRetry))?;
        *cumulative_written = cumulative_written.saturating_add(read as u64);
        on_progress(TypedTransferProgress::Bytes {
            completed: *cumulative_written,
            total: total_bytes,
        });
        check_cancelled(cancel, completed)?;
    }
}

async fn remote_exists(
    session: &russh_sftp::client::SftpSession,
    path: &str,
) -> Result<bool, TransferExecutionError> {
    match session.metadata(path.to_string()).await {
        Ok(_) => Ok(true),
        Err(SftpError::Status(status)) if status.status_code == StatusCode::NoSuchFile => Ok(false),
        Err(error) => Err(phase_sftp(path, error, RetryDisposition::SafeToRetry)),
    }
}

async fn local_exists(path: &Path) -> Result<bool, TransferExecutionError> {
    match tokio::fs::metadata(path).await {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(phase_err(error, RetryDisposition::SafeToRetry)),
    }
}

fn validate_name(name: &str) -> Result<(), TransferExecutionError> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\0') {
        return Err(invalid(
            "transfer item must be a single safe path component",
        ));
    }
    Ok(())
}

fn remote_join(dir: &str, name: &str) -> String {
    if dir == "/" {
        format!("/{name}")
    } else {
        let dir = dir.trim_end_matches('/');
        if dir.is_empty() {
            name.to_string()
        } else {
            format!("{dir}/{name}")
        }
    }
}

fn path_with_suffix(path: &Path, suffix: &str) -> std::path::PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    value.into()
}

fn operation_token() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn check_cancelled(cancel: &AtomicBool, completed: usize) -> Result<(), TransferExecutionError> {
    if cancel.load(Ordering::Relaxed) {
        Err(TransferExecutionError::Cancelled { completed })
    } else {
        Ok(())
    }
}

fn invalid(reason: impl Into<String>) -> TransferExecutionError {
    TransferExecutionError::InvalidPlan {
        method: TransferMethod::Sftp,
        reason: reason.into(),
    }
}

fn sftp_failure(item: &str, error: SftpError) -> TransferExecutionError {
    TransferExecutionError::Worker(format!("SFTP {item}: {error}"))
}

fn phase_sftp(
    item: &str,
    error: SftpError,
    disposition: RetryDisposition,
) -> TransferExecutionError {
    phase_err(
        io::Error::other(format!("SFTP {item}: {error}")),
        disposition,
    )
}

async fn clean_local_stage(
    path: &Path,
    original: TransferExecutionError,
) -> TransferExecutionError {
    match tokio::fs::remove_file(path).await {
        Ok(()) => original,
        Err(e) if e.kind() == io::ErrorKind::NotFound => original,
        Err(e) => phase_err(e, RetryDisposition::RecoveryRequired),
    }
}

async fn clean_remote_stage(
    connection: &OpenSshSftpConnection,
    path: &str,
    original: TransferExecutionError,
) -> TransferExecutionError {
    match connection.session().remove_file(path.to_string()).await {
        Ok(()) => original,
        Err(SftpError::Status(status)) if status.status_code == StatusCode::NoSuchFile => original,
        Err(error) => phase_sftp(path, error, RetryDisposition::RecoveryRequired),
    }
}

fn phase_err(error: io::Error, disposition: RetryDisposition) -> TransferExecutionError {
    TransferExecutionError::Io {
        source: error,
        disposition,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_join_preserves_root() {
        assert_eq!(remote_join("/", "a.txt"), "/a.txt");
        assert_eq!(remote_join("/tmp/", "a.txt"), "/tmp/a.txt");
    }

    #[test]
    fn unsafe_item_names_are_rejected() {
        assert!(validate_name("../secret").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("file.txt").is_ok());
    }

    #[test]
    fn phase_error_preserves_retry_disposition() {
        for disposition in [
            RetryDisposition::SafeToRetry,
            RetryDisposition::AmbiguousMutation,
            RetryDisposition::RecoveryRequired,
        ] {
            assert_eq!(
                phase_err(io::Error::other("phase"), disposition).retry_disposition(),
                disposition
            );
        }
    }

    #[test]
    fn remote_to_remote_plan_shape_is_sftp_only() {
        let plan = TransferPlan {
            source: Location::Sftp {
                host: "a".into(),
                path: "/src".into(),
            },
            destination: Location::Sftp {
                host: "b".into(),
                path: "/dst".into(),
            },
            intent: TransferIntent::Copy,
            method: TransferMethod::Sftp,
            s3_spec: None,
            webdav_spec: None,
        };
        assert!(matches!(plan.source, Location::Sftp { .. }));
        assert!(matches!(plan.destination, Location::Sftp { .. }));
        assert_eq!(plan.method, TransferMethod::Sftp);
    }

    #[test]
    fn backup_suffix_preserves_original_name() {
        let path = Path::new("/tmp/file.txt");
        assert_eq!(
            path_with_suffix(path, ".arx-bak-1"),
            Path::new("/tmp/file.txt.arx-bak-1")
        );
    }
}
