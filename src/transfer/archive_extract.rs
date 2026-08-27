use std::io::{self, Write};
use std::path::{Component, Path};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::io::AsyncReadExt;

use super::ArchiveTransferSpec;
use crate::transfer_queue::{PauseGate, TypedTransferProgress};

const CHILD_POLL: std::time::Duration = std::time::Duration::from_millis(20);
const METADATA_LIMIT: usize = 64 * 1024;

fn validate_member(member: &str) -> io::Result<()> {
    if member.is_empty()
        || !Path::new(member)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "archive member path must be relative and traversal-free",
        ));
    }
    Ok(())
}

fn is_zip(archive: &Path) -> bool {
    archive
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".zip"))
}

fn command(archive: &Path, member: &str, metadata: bool) -> tokio::process::Command {
    let mut command = if is_zip(archive) {
        let mut command = tokio::process::Command::new("unzip");
        if metadata {
            command.args(["-Z", "-l"]);
        } else {
            command.arg("-p");
        }
        command.arg(archive).arg(member);
        command
    } else {
        let mut command = tokio::process::Command::new("tar");
        if metadata {
            command.args(["--list", "--verbose", "--file"]);
        } else {
            command.args(["--extract", "--to-stdout", "--file"]);
        }
        command.arg(archive).arg("--").arg(member);
        command
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    command
}

async fn stream_child(
    mut command: tokio::process::Command,
    cancel: &Arc<AtomicBool>,
    pause: &PauseGate,
    mut write: impl FnMut(&[u8]) -> io::Result<()>,
) -> io::Result<u64> {
    let mut child = command.spawn()?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("archive child stdout was not piped"))?;
    let mut buffer = [0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        pause.checkpoint().await;
        if cancel.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "archive extraction cancelled",
            ));
        }
        let read = tokio::select! {
            read = stdout.read(&mut buffer) => read?,
            _ = tokio::time::sleep(CHILD_POLL) => continue,
        };
        if read == 0 {
            break;
        }
        write(&buffer[..read])?;
        total = total.saturating_add(read as u64);
    }
    let status = child.wait().await?;
    if !status.success() {
        return Err(io::Error::other("archive member extraction failed"));
    }
    Ok(total)
}

async fn require_single_regular_member(
    archive: &Path,
    member: &str,
    cancel: &Arc<AtomicBool>,
    pause: &PauseGate,
) -> io::Result<()> {
    let mut metadata = Vec::new();
    stream_child(command(archive, member, true), cancel, pause, |chunk| {
        if metadata.len().saturating_add(chunk.len()) > METADATA_LIMIT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "archive member metadata exceeds safety limit",
            ));
        }
        metadata.extend_from_slice(chunk);
        Ok(())
    })
    .await?;
    let text = String::from_utf8(metadata)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "archive metadata is not UTF-8"))?;
    let entries = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    if entries.len() != 1 || !entries[0].starts_with('-') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "archive extraction requires one exact regular-file member",
        ));
    }
    Ok(())
}

/// Extract one exact regular archive member into one new Local file.
///
/// The destination is staged in its parent directory and committed with
/// `persist_noclobber`; cancellation or failure drops the stage and child.
pub async fn extract_one(
    archive: &Path,
    spec: &ArchiveTransferSpec,
    cancel: Arc<AtomicBool>,
    pause: PauseGate,
    mut on_progress: impl FnMut(TypedTransferProgress),
) -> io::Result<()> {
    validate_member(&spec.source.member_path)?;
    let destination_dir = spec.local_destination.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "archive destination must have a Local parent directory",
        )
    })?;
    let mut staged = tempfile::NamedTempFile::new_in(destination_dir)?;
    require_single_regular_member(archive, &spec.source.member_path, &cancel, &pause).await?;
    let bytes = stream_child(
        command(archive, &spec.source.member_path, false),
        &cancel,
        &pause,
        |chunk| staged.as_file_mut().write_all(chunk),
    )
    .await?;
    on_progress(TypedTransferProgress::Bytes {
        completed: bytes,
        total: Some(bytes),
    });
    pause.checkpoint().await;
    if cancel.load(Ordering::Acquire) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "archive extraction cancelled",
        ));
    }
    staged
        .persist_noclobber(&spec.local_destination)
        .map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_validation_rejects_absolute_and_parent_paths() {
        assert!(validate_member("/absolute").is_err());
        assert!(validate_member("../escape").is_err());
        assert!(validate_member("safe/../escape").is_err());
        assert!(validate_member("safe/資料 file.txt").is_ok());
    }
}
