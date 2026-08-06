use std::ffi::OsString;
use std::io;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::vfs::{Location, local::LocalFs};

use super::{TransferIntent, TransferMethod, TransferPlan};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferProgress {
    pub completed: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferOutcome {
    pub completed: usize,
    pub total: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum TransferExecutionError {
    #[error("transfer was cancelled after {completed} item(s)")]
    Cancelled { completed: usize },
    #[error("invalid transfer plan for {method:?}: {reason}")]
    InvalidPlan {
        method: TransferMethod,
        reason: String,
    },
    #[error("rsync failed for {item} with exit code {code:?}")]
    RsyncFailed { item: String, code: Option<i32> },
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Execute a previously validated transfer plan.
///
/// This layer owns transfer implementation details. It does not create jobs or
/// touch TUI state; callers can map `TransferProgress` to Job events.
pub fn execute_transfer(
    plan: &TransferPlan,
    names: &[String],
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(TransferProgress),
) -> Result<TransferOutcome, TransferExecutionError> {
    match plan.method {
        TransferMethod::Native => execute_native(plan, names, cancel, &mut on_progress),
        TransferMethod::Rsync => execute_rsync(plan, names, cancel, &mut on_progress),
        TransferMethod::Sftp | TransferMethod::Scp => Err(TransferExecutionError::InvalidPlan {
            method: plan.method,
            reason: "executor is not implemented yet".into(),
        }),
    }
}

fn execute_native(
    plan: &TransferPlan,
    names: &[String],
    cancel: &AtomicBool,
    on_progress: &mut impl FnMut(TransferProgress),
) -> Result<TransferOutcome, TransferExecutionError> {
    let (Location::Local(src), Location::Local(dst)) = (&plan.source, &plan.destination) else {
        return Err(TransferExecutionError::InvalidPlan {
            method: TransferMethod::Native,
            reason: "native executor currently requires local source and destination".into(),
        });
    };

    let total = names.len();
    let mut completed = 0;
    for name in names {
        ensure_not_cancelled(cancel, completed)?;
        let one = [name.clone()];
        match plan.intent {
            TransferIntent::Copy => {
                LocalFs::copy_files(src, dst, &one)?;
            }
            TransferIntent::Move => {
                LocalFs::move_files(src, dst, &one)?;
            }
            TransferIntent::Synchronize => {
                return Err(TransferExecutionError::InvalidPlan {
                    method: TransferMethod::Native,
                    reason: "native synchronization is not implemented".into(),
                });
            }
        }
        completed += 1;
        on_progress(TransferProgress { completed, total });
    }

    Ok(TransferOutcome { completed, total })
}

fn execute_rsync(
    plan: &TransferPlan,
    names: &[String],
    cancel: &AtomicBool,
    on_progress: &mut impl FnMut(TransferProgress),
) -> Result<TransferOutcome, TransferExecutionError> {
    if plan.intent == TransferIntent::Move {
        return Err(TransferExecutionError::InvalidPlan {
            method: TransferMethod::Rsync,
            reason: "rsync move requires a separately verified source-cleanup phase".into(),
        });
    }

    if matches!(
        (&plan.source, &plan.destination),
        (Location::Sftp { .. }, Location::Sftp { .. })
    ) {
        return Err(TransferExecutionError::InvalidPlan {
            method: TransferMethod::Rsync,
            reason: "remote-to-remote rsync is not supported".into(),
        });
    }

    let items: Vec<Option<&str>> = if plan.intent == TransferIntent::Synchronize {
        vec![None]
    } else {
        names.iter().map(|name| Some(name.as_str())).collect()
    };

    let total = items.len();
    let mut completed = 0;
    for item in items {
        ensure_not_cancelled(cancel, completed)?;
        let args = build_rsync_args(plan, item)?;
        run_rsync(&args, cancel, completed, item.unwrap_or("."))?;
        completed += 1;
        on_progress(TransferProgress { completed, total });
    }

    Ok(TransferOutcome { completed, total })
}

fn build_rsync_args(
    plan: &TransferPlan,
    item: Option<&str>,
) -> Result<Vec<OsString>, TransferExecutionError> {
    let source = endpoint_arg(
        &plan.source,
        item,
        plan.intent == TransferIntent::Synchronize,
    )?;
    let destination = endpoint_arg(&plan.destination, None, false)?;
    let suffix = backup_suffix();

    Ok(vec![
        OsString::from("--archive"),
        OsString::from("--partial"),
        OsString::from("--protect-args"),
        OsString::from("--backup"),
        OsString::from(format!("--suffix={suffix}")),
        OsString::from("--"),
        source,
        destination,
    ])
}

fn endpoint_arg(
    location: &Location,
    item: Option<&str>,
    sync_contents: bool,
) -> Result<OsString, TransferExecutionError> {
    match location {
        Location::Local(path) => {
            let path = match item {
                Some(name) => path.join(name),
                None if sync_contents => path.join("."),
                None => path.clone(),
            };
            Ok(path.into_os_string())
        }
        Location::Sftp { host, path } => {
            let mut remote_path = path.trim_end_matches('/').to_string();
            if let Some(name) = item {
                if !remote_path.is_empty() {
                    remote_path.push('/');
                }
                remote_path.push_str(name);
            } else if sync_contents {
                if !remote_path.is_empty() {
                    remote_path.push('/');
                }
                remote_path.push('.');
            }
            Ok(OsString::from(format!("{host}:{remote_path}")))
        }
        Location::Archive { .. } => Err(TransferExecutionError::InvalidPlan {
            method: TransferMethod::Rsync,
            reason: "rsync cannot address archive locations directly".into(),
        }),
    }
}

fn run_rsync(
    args: &[OsString],
    cancel: &AtomicBool,
    completed: usize,
    item: &str,
) -> Result<(), TransferExecutionError> {
    let mut child = Command::new("rsync")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(TransferExecutionError::Cancelled { completed });
        }

        if let Some(status) = child.try_wait()? {
            if status.success() {
                return Ok(());
            }
            return Err(TransferExecutionError::RsyncFailed {
                item: item.to_string(),
                code: status.code(),
            });
        }

        thread::sleep(Duration::from_millis(50));
    }
}

fn ensure_not_cancelled(
    cancel: &AtomicBool,
    completed: usize,
) -> Result<(), TransferExecutionError> {
    if cancel.load(Ordering::Relaxed) {
        Err(TransferExecutionError::Cancelled { completed })
    } else {
        Ok(())
    }
}

fn backup_suffix() -> String {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!(".arx-bak-{stamp}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn local(path: PathBuf) -> Location {
        Location::Local(path)
    }

    fn sftp(host: &str, path: &str) -> Location {
        Location::Sftp {
            host: host.into(),
            path: path.into(),
        }
    }

    #[test]
    fn native_copy_executes_and_reports_progress() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        fs::write(src.path().join("a.txt"), b"a").unwrap();
        fs::write(src.path().join("b.txt"), b"b").unwrap();

        let plan = TransferPlan {
            source: local(src.path().to_path_buf()),
            destination: local(dst.path().to_path_buf()),
            intent: TransferIntent::Copy,
            method: TransferMethod::Native,
        };
        let cancel = AtomicBool::new(false);
        let mut progress = Vec::new();
        let outcome =
            execute_transfer(&plan, &["a.txt".into(), "b.txt".into()], &cancel, |event| {
                progress.push(event)
            })
            .unwrap();

        assert_eq!(outcome.completed, 2);
        assert_eq!(progress.last().unwrap().completed, 2);
        assert!(dst.path().join("a.txt").exists());
        assert!(dst.path().join("b.txt").exists());
    }

    #[test]
    fn native_executor_honors_pre_cancel() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        fs::write(src.path().join("a.txt"), b"a").unwrap();

        let plan = TransferPlan {
            source: local(src.path().to_path_buf()),
            destination: local(dst.path().to_path_buf()),
            intent: TransferIntent::Move,
            method: TransferMethod::Native,
        };
        let cancel = AtomicBool::new(true);
        let error = execute_transfer(&plan, &["a.txt".into()], &cancel, |_| {}).unwrap_err();

        assert!(matches!(
            error,
            TransferExecutionError::Cancelled { completed: 0 }
        ));
        assert!(src.path().join("a.txt").exists());
        assert!(!dst.path().join("a.txt").exists());
    }

    #[test]
    fn rsync_args_are_non_destructive_and_use_backup() {
        let plan = TransferPlan {
            source: local(PathBuf::from("/src")),
            destination: sftp("prod", "/dst"),
            intent: TransferIntent::Copy,
            method: TransferMethod::Rsync,
        };
        let args = build_rsync_args(&plan, Some("file.txt")).unwrap();
        let rendered = args
            .iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>();

        assert!(rendered.iter().any(|arg| arg.as_ref() == "--backup"));
        assert!(
            rendered
                .iter()
                .any(|arg| arg.starts_with("--suffix=.arx-bak-"))
        );
        assert!(!rendered.iter().any(|arg| arg.as_ref() == "--delete"));
        assert!(rendered.iter().any(|arg| arg.as_ref() == "prod:/dst"));
    }

    #[test]
    fn remote_to_remote_rsync_is_rejected() {
        let plan = TransferPlan {
            source: sftp("a", "/src"),
            destination: sftp("b", "/dst"),
            intent: TransferIntent::Copy,
            method: TransferMethod::Rsync,
        };
        let cancel = AtomicBool::new(false);

        assert!(matches!(
            execute_transfer(&plan, &["x".into()], &cancel, |_| {}),
            Err(TransferExecutionError::InvalidPlan { .. })
        ));
    }

    #[test]
    fn rsync_move_requires_separate_cleanup_phase() {
        let plan = TransferPlan {
            source: local(PathBuf::from("/src")),
            destination: sftp("prod", "/dst"),
            intent: TransferIntent::Move,
            method: TransferMethod::Rsync,
        };
        let cancel = AtomicBool::new(false);

        assert!(matches!(
            execute_transfer(&plan, &["x".into()], &cancel, |_| {}),
            Err(TransferExecutionError::InvalidPlan { .. })
        ));
    }
}
