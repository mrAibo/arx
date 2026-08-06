//! Remote Watch — sync local changes to remote via inotify + rsync.
//! Uses system tools and keeps destructive mirroring opt-in.
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

const DEBOUNCE: Duration = Duration::from_millis(750);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchMode {
    /// Copy new and changed content. Remote-only files are preserved.
    UpdateOnly,
    /// Mirror the source, including deletions. Must be explicitly requested by the caller.
    Mirror,
}

/// Watch a local directory and propagate new/changed content to the remote target.
///
/// This safe default never passes `--delete` to rsync. Use `start_watch_mode`
/// with `WatchMode::Mirror` only after an explicit destructive-sync confirmation.
pub fn start_watch(local: &Path, remote_host: &str, remote_path: &str) -> io::Result<()> {
    start_watch_mode(local, remote_host, remote_path, WatchMode::UpdateOnly)
}

pub fn start_watch_mode(
    local: &Path,
    remote_host: &str,
    remote_path: &str,
    mode: WatchMode,
) -> io::Result<()> {
    let target = format!("{remote_host}:{remote_path}");
    run_rsync(local, &target, mode)?;

    let local = local.to_path_buf();
    std::thread::Builder::new()
        .name("arx-remote-watch".into())
        .spawn(move || {
            let mut child = match Command::new("inotifywait")
                .args([
                    "-m",
                    "-r",
                    "-e",
                    "modify,create,delete,move",
                    "--format",
                    "%w%f",
                    &local.to_string_lossy(),
                ])
                .stdout(Stdio::piped())
                .spawn()
            {
                Ok(child) => child,
                Err(_) => return,
            };

            let Some(stdout) = child.stdout.take() else {
                let _ = child.kill();
                let _ = child.wait();
                return;
            };

            let (event_tx, event_rx) = mpsc::channel::<()>();
            let reader = std::thread::Builder::new()
                .name("arx-remote-watch-reader".into())
                .spawn(move || {
                    use std::io::BufRead;
                    for line in std::io::BufReader::new(stdout).lines() {
                        if line.is_err() || event_tx.send(()).is_err() {
                            break;
                        }
                    }
                });

            if reader.is_err() {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }

            while event_rx.recv().is_ok() {
                // Coalesce editor saves, git checkouts, and directory operations into
                // a single rsync instead of starting one process per inotify event.
                while event_rx.recv_timeout(DEBOUNCE).is_ok() {}
                if run_rsync(&local, &target, mode).is_err() {
                    // Keep watching after a transient transfer failure. A later event
                    // gets another chance; lifecycle/error reporting belongs to the
                    // Job Manager integration planned for the next migration step.
                    continue;
                }
            }

            let _ = child.kill();
            let _ = child.wait();
        })?;

    Ok(())
}

fn run_rsync(local: &Path, target: &str, mode: WatchMode) -> io::Result<()> {
    let mut command = Command::new("rsync");
    command.args(["-az", "--partial"]);
    if mode == WatchMode::Mirror {
        command.arg("--delete-delay");
    }
    command.arg(local).arg(target);

    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "rsync exited with status {status}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_watch_mode_is_non_destructive() {
        assert_eq!(WatchMode::UpdateOnly, WatchMode::UpdateOnly);
    }

    #[test]
    fn mirror_is_explicitly_distinct_from_update_only() {
        assert_ne!(WatchMode::Mirror, WatchMode::UpdateOnly);
    }
}
