use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use russh_sftp::client::error::Error as SftpError;
use russh_sftp::protocol::StatusCode;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::remote::openssh_sftp::OpenSshSftpConnection;
use crate::vfs::Location;

use super::executor::{TransferExecutionError, TransferOutcome, TransferProgress};
use super::{TransferIntent, TransferMethod, TransferPlan};

const COPY_BUFFER_SIZE: usize = 64 * 1024;

pub(crate) async fn execute_sftp_copy(
    plan: &TransferPlan,
    names: &[String],
    cancel: Arc<AtomicBool>,
    on_progress: &mut impl FnMut(TransferProgress),
) -> Result<TransferOutcome, TransferExecutionError> {
    if plan.intent != TransferIntent::Copy {
        return Err(invalid("SFTP Move requires a separate copy/verify/delete-source transaction"));
    }

    let (host, direction) = match (&plan.source, &plan.destination) {
        (Location::Local(src), Location::Sftp { host, path }) => {
            (host.as_str(), Direction::Upload { src, remote_dir: path })
        }
        (Location::Sftp { host, path }, Location::Local(dst)) => {
            (host.as_str(), Direction::Download { remote_dir: path, dst })
        }
        _ => {
            return Err(invalid(
                "SFTP copy requires exactly one local and one SFTP endpoint",
            ));
        }
    };

    let connection = OpenSshSftpConnection::connect(host).await?;
    let total = names.len();
    let mut completed = 0;

    for name in names {
        validate_name(name)?;
        check_cancelled(&cancel, completed)?;

        match direction {
            Direction::Upload { src, remote_dir } => {
                upload_file(&connection, src, remote_dir, name, &cancel, completed).await?;
            }
            Direction::Download { remote_dir, dst } => {
                download_file(&connection, remote_dir, dst, name, &cancel, completed).await?;
            }
        }

        completed += 1;
        on_progress(TransferProgress { completed, total });
    }

    let _ = connection.close().await;
    Ok(TransferOutcome { completed, total })
}

#[derive(Clone, Copy)]
enum Direction<'a> {
    Upload { src: &'a Path, remote_dir: &'a str },
    Download { remote_dir: &'a str, dst: &'a Path },
}

async fn upload_file(
    connection: &OpenSshSftpConnection,
    src_dir: &Path,
    remote_dir: &str,
    name: &str,
    cancel: &AtomicBool,
    completed: usize,
) -> Result<(), TransferExecutionError> {
    let local_path = src_dir.join(name);
    let local_meta = tokio::fs::metadata(&local_path).await?;
    if !local_meta.is_file() {
        return Err(invalid("SFTP fallback currently supports regular files only"));
    }

    let target = remote_join(remote_dir, name);
    let token = operation_token();
    let temp = format!("{target}.arx-part-{token}");
    let backup = format!("{target}.arx-bak-{token}");

    let mut local = tokio::fs::File::open(&local_path).await?;
    let mut remote = connection
        .session
        .create(temp.clone())
        .await
        .map_err(|error| sftp_failure(name, error))?;

    if let Err(error) = copy_stream(&mut local, &mut remote, cancel, completed).await {
        let _ = connection.session.remove_file(temp.clone()).await;
        return Err(error);
    }
    if let Err(error) = remote.flush().await {
        let _ = connection.session.remove_file(temp.clone()).await;
        return Err(error.into());
    }
    if let Err(error) = remote.shutdown().await {
        let _ = connection.session.remove_file(temp.clone()).await;
        return Err(error.into());
    }

    let staged = match connection.session.metadata(temp.clone()).await {
        Ok(staged) => staged,
        Err(error) => {
            let _ = connection.session.remove_file(temp.clone()).await;
            return Err(sftp_failure(name, error));
        }
    };
    if staged.len() != local_meta.len() {
        let _ = connection.session.remove_file(temp.clone()).await;
        return Err(invalid("SFTP upload verification failed: staged size differs"));
    }

    let had_target = match remote_exists(&connection.session, &target).await {
        Ok(exists) => exists,
        Err(error) => {
            let _ = connection.session.remove_file(temp.clone()).await;
            return Err(error);
        }
    };
    if had_target {
        if let Err(error) = connection
            .session
            .rename(target.clone(), backup.clone())
            .await
        {
            let _ = connection.session.remove_file(temp.clone()).await;
            return Err(sftp_failure(name, error));
        }
    }

    if let Err(error) = connection.session.rename(temp.clone(), target.clone()).await {
        let _ = connection.session.remove_file(temp).await;
        if had_target {
            let _ = connection.session.rename(backup, target).await;
        }
        return Err(sftp_failure(name, error));
    }

    Ok(())
}

async fn download_file(
    connection: &OpenSshSftpConnection,
    remote_dir: &str,
    dst_dir: &Path,
    name: &str,
    cancel: &AtomicBool,
    completed: usize,
) -> Result<(), TransferExecutionError> {
    let source = remote_join(remote_dir, name);
    let remote_meta = connection
        .session
        .metadata(source.clone())
        .await
        .map_err(|error| sftp_failure(name, error))?;
    if !remote_meta.is_regular() {
        return Err(invalid("SFTP fallback currently supports regular files only"));
    }

    let token = operation_token();
    let target = dst_dir.join(name);
    let temp = dst_dir.join(format!(".arx-part-{token}"));
    let backup = path_with_suffix(&target, &format!(".arx-bak-{token}"));

    let mut remote = connection
        .session
        .open(source)
        .await
        .map_err(|error| sftp_failure(name, error))?;
    let mut local = tokio::fs::File::create(&temp).await?;

    if let Err(error) = copy_stream(&mut remote, &mut local, cancel, completed).await {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(error);
    }
    if let Err(error) = local.flush().await {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(error.into());
    }
    if let Err(error) = local.sync_all().await {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(error.into());
    }

    let staged = match tokio::fs::metadata(&temp).await {
        Ok(staged) => staged,
        Err(error) => {
            let _ = tokio::fs::remove_file(&temp).await;
            return Err(error.into());
        }
    };
    if staged.len() != remote_meta.len() {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(invalid("SFTP download verification failed: staged size differs"));
    }

    let had_target = match local_exists(&target).await {
        Ok(exists) => exists,
        Err(error) => {
            let _ = tokio::fs::remove_file(&temp).await;
            return Err(error);
        }
    };
    if had_target {
        if let Err(error) = tokio::fs::rename(&target, &backup).await {
            let _ = tokio::fs::remove_file(&temp).await;
            return Err(error.into());
        }
    }

    if let Err(error) = tokio::fs::rename(&temp, &target).await {
        let _ = tokio::fs::remove_file(&temp).await;
        if had_target {
            let _ = tokio::fs::rename(&backup, &target).await;
        }
        return Err(error.into());
    }

    Ok(())
}

async fn copy_stream<R, W>(
    reader: &mut R,
    writer: &mut W,
    cancel: &AtomicBool,
    completed: usize,
) -> Result<(), TransferExecutionError>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut buffer = vec![0_u8; COPY_BUFFER_SIZE];
    loop {
        check_cancelled(cancel, completed)?;
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return Ok(());
        }
        writer.write_all(&buffer[..read]).await?;
    }
}

async fn remote_exists(
    session: &russh_sftp::client::SftpSession,
    path: &str,
) -> Result<bool, TransferExecutionError> {
    match session.metadata(path.to_string()).await {
        Ok(_) => Ok(true),
        Err(SftpError::Status(status)) if status.status_code == StatusCode::NoSuchFile => Ok(false),
        Err(error) => Err(sftp_failure(path, error)),
    }
}

async fn local_exists(path: &Path) -> Result<bool, TransferExecutionError> {
    match tokio::fs::metadata(path).await {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn validate_name(name: &str) -> Result<(), TransferExecutionError> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\0') {
        return Err(invalid("transfer item must be a single safe path component"));
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

fn check_cancelled(
    cancel: &AtomicBool,
    completed: usize,
) -> Result<(), TransferExecutionError> {
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
    fn backup_suffix_preserves_original_name() {
        let path = Path::new("/tmp/file.txt");
        assert_eq!(
            path_with_suffix(path, ".arx-bak-1"),
            Path::new("/tmp/file.txt.arx-bak-1")
        );
    }
}
