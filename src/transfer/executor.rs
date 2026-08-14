use std::ffi::OsString;
use std::io;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::vfs::{Location, ProviderRegistry, local::LocalFs};

use super::s3_download;
use super::s3_upload;
use super::{S3TransferSpec, TransferIntent, TransferMethod, TransferPlan};
use crate::transfer::sftp_copy;

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
    #[error("transfer worker failed: {0}")]
    Worker(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Execute a previously validated transfer plan without blocking the async TUI.
///
/// Native filesystem work runs on Tokio's blocking pool. External processes use
/// Tokio process I/O. Remote async executors can therefore plug into the same
/// contract without `block_on` bridges.
pub async fn execute_transfer(
    plan: &TransferPlan,
    names: &[String],
    registry: &ProviderRegistry,
    cancel: Arc<AtomicBool>,
    mut on_progress: impl FnMut(TransferProgress),
) -> Result<TransferOutcome, TransferExecutionError> {
    match plan.method {
        TransferMethod::Native => execute_native(plan, names, cancel, &mut on_progress).await,
        TransferMethod::Rsync => execute_rsync(plan, names, cancel, &mut on_progress).await,
        TransferMethod::Sftp => {
            sftp_copy::execute_sftp_copy(plan, names, cancel, &mut on_progress).await
        }
        TransferMethod::Scp => Err(TransferExecutionError::InvalidPlan {
            method: plan.method,
            reason: "SCP executor is not implemented".into(),
        }),
        TransferMethod::S3 => {
            let spec =
                plan.s3_spec
                    .as_ref()
                    .ok_or_else(|| TransferExecutionError::InvalidPlan {
                        method: plan.method,
                        reason: "S3 transfer plan missing frozen spec".into(),
                    })?;
            let target = match spec {
                S3TransferSpec::UploadOne { destination, .. } => &destination.target,
                S3TransferSpec::DownloadOne { source, .. } => &source.target,
            };
            let provider = registry.s3_provider_for_transfer(target).map_err(|e| {
                TransferExecutionError::InvalidPlan {
                    method: TransferMethod::S3,
                    reason: e.to_string(),
                }
            })?;
            // ponytail: spec is the sole identity authority; names unused for S3
            match spec {
                S3TransferSpec::UploadOne { .. } => {
                    s3_upload::upload_one(
                        &provider,
                        spec,
                        s3_upload::S3OverwritePolicy::Forbid,
                        cancel.clone(),
                        &mut on_progress,
                    )
                    .await
                    .map_err(TransferExecutionError::Io)?;
                }
                S3TransferSpec::DownloadOne { .. } => {
                    s3_download::download_one(&provider, spec, cancel.clone())
                        .await
                        .map_err(TransferExecutionError::Io)?;
                }
            }
            Ok(TransferOutcome {
                completed: 1,
                total: 1,
            })
        }
    }
}

async fn execute_native(
    plan: &TransferPlan,
    names: &[String],
    cancel: Arc<AtomicBool>,
    on_progress: &mut impl FnMut(TransferProgress),
) -> Result<TransferOutcome, TransferExecutionError> {
    let (Location::Local(src), Location::Local(dst)) = (&plan.source, &plan.destination) else {
        return Err(TransferExecutionError::InvalidPlan {
            method: TransferMethod::Native,
            reason: "native executor currently requires local source and destination".into(),
        });
    };

    let src = src.clone();
    let dst = dst.clone();
    let intent = plan.intent;
    let total = names.len();
    let mut completed = 0;

    for name in names {
        ensure_not_cancelled(&cancel, completed)?;
        let src = src.clone();
        let dst = dst.clone();
        let name = name.clone();

        tokio::task::spawn_blocking(move || {
            let one = [name];
            match intent {
                TransferIntent::Copy => LocalFs::copy_files(&src, &dst, &one).map(|_| ()),
                TransferIntent::Move => LocalFs::move_files(&src, &dst, &one).map(|_| ()),
                TransferIntent::Synchronize => Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "native synchronization is not implemented",
                )),
            }
        })
        .await
        .map_err(|error| TransferExecutionError::Worker(error.to_string()))??;

        completed += 1;
        on_progress(TransferProgress { completed, total });
    }

    Ok(TransferOutcome { completed, total })
}

async fn execute_rsync(
    plan: &TransferPlan,
    names: &[String],
    cancel: Arc<AtomicBool>,
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
        ensure_not_cancelled(&cancel, completed)?;
        let args = build_rsync_args(plan, item)?;
        run_rsync(&args, &cancel, completed, item.unwrap_or(".")).await?;
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
        // ponytail: rsync cannot address S3; explicit typed rejection, no s3:// encode
        Location::S3 { .. } => Err(TransferExecutionError::InvalidPlan {
            method: TransferMethod::Rsync,
            reason: "rsync cannot address S3 locations directly".into(),
        }),
    }
}

async fn run_rsync(
    args: &[OsString],
    cancel: &AtomicBool,
    completed: usize,
    item: &str,
) -> Result<(), TransferExecutionError> {
    let mut child = tokio::process::Command::new("rsync")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;

    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill().await;
            let _ = child.wait().await;
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

        tokio::time::sleep(Duration::from_millis(50)).await;
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
    use crate::config::S3TargetConfig;
    use crate::vfs::S3ObjectRef;
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

    #[tokio::test]
    async fn native_copy_executes_and_reports_progress() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        fs::write(src.path().join("a.txt"), b"a").unwrap();
        fs::write(src.path().join("b.txt"), b"b").unwrap();

        let plan = TransferPlan {
            source: local(src.path().to_path_buf()),
            destination: local(dst.path().to_path_buf()),
            intent: TransferIntent::Copy,
            method: TransferMethod::Native,
            s3_spec: None,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let mut progress = Vec::new();
        let outcome = execute_transfer(
            &plan,
            &["a.txt".into(), "b.txt".into()],
            &ProviderRegistry::new(),
            cancel,
            |event| progress.push(event),
        )
        .await
        .unwrap();

        assert_eq!(outcome.completed, 2);
        assert_eq!(progress.last().unwrap().completed, 2);
        assert!(dst.path().join("a.txt").exists());
        assert!(dst.path().join("b.txt").exists());
    }

    #[tokio::test]
    async fn native_executor_honors_pre_cancel() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        fs::write(src.path().join("a.txt"), b"a").unwrap();

        let plan = TransferPlan {
            source: local(src.path().to_path_buf()),
            destination: local(dst.path().to_path_buf()),
            intent: TransferIntent::Move,
            method: TransferMethod::Native,
            s3_spec: None,
        };
        let cancel = Arc::new(AtomicBool::new(true));
        let error = execute_transfer(
            &plan,
            &["a.txt".into()],
            &ProviderRegistry::new(),
            cancel,
            |_| {},
        )
        .await
        .unwrap_err();

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
            s3_spec: None,
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
    fn byte_progress_saturates_instead_of_overflowing() {
        let progress = crate::jobs::Progress::Bytes {
            done: u64::MAX,
            total: u64::MAX,
            rate: 1,
        };
        assert_eq!(progress.percent(), Some(100));
    }

    #[tokio::test]
    async fn remote_to_remote_rsync_is_rejected() {
        let plan = TransferPlan {
            source: sftp("a", "/src"),
            destination: sftp("b", "/dst"),
            intent: TransferIntent::Copy,
            method: TransferMethod::Rsync,
            s3_spec: None,
        };
        let cancel = Arc::new(AtomicBool::new(false));

        assert!(matches!(
            execute_transfer(
                &plan,
                &["x".into()],
                &ProviderRegistry::new(),
                cancel,
                |_| {}
            )
            .await,
            Err(TransferExecutionError::InvalidPlan { .. })
        ));
    }

    #[tokio::test]
    async fn rsync_move_requires_separate_cleanup_phase() {
        let plan = TransferPlan {
            source: local(PathBuf::from("/src")),
            destination: sftp("prod", "/dst"),
            intent: TransferIntent::Move,
            method: TransferMethod::Rsync,
            s3_spec: None,
        };
        let cancel = Arc::new(AtomicBool::new(false));

        assert!(matches!(
            execute_transfer(
                &plan,
                &["x".into()],
                &ProviderRegistry::new(),
                cancel,
                |_| {}
            )
            .await,
            Err(TransferExecutionError::InvalidPlan { .. })
        ));
    }

    #[test]
    fn endpoint_arg_rejects_s3_location() {
        let loc = Location::S3 {
            target: "aws".into(),
            bucket: Some("bucket".into()),
            prefix: "prefix".into(),
        };
        let res = endpoint_arg(&loc, None, false);
        assert!(matches!(
            res,
            Err(TransferExecutionError::InvalidPlan {
                method: TransferMethod::Rsync,
                ..
            })
        ));
    }

    // ── S3-36/39: executor routing & fail-closed paths ──

    fn s3_plan(method: TransferMethod, s3_spec: Option<S3TransferSpec>) -> TransferPlan {
        TransferPlan {
            source: local(PathBuf::from("/src")),
            destination: local(PathBuf::from("/dst")),
            intent: TransferIntent::Copy,
            method,
            s3_spec,
        }
    }

    fn s3_upload_spec(target: &str, bucket: &str) -> S3TransferSpec {
        S3TransferSpec::UploadOne {
            local_source: PathBuf::from("/nonexistent/arx/upload/source.txt"),
            destination: S3ObjectRef {
                target: target.into(),
                bucket: bucket.into(),
                key: "a.txt".into(),
            },
        }
    }

    fn s3_target_config(id: &str, bucket: &str) -> S3TargetConfig {
        S3TargetConfig {
            id: id.into(),
            name: id.into(),
            bucket: Some(bucket.into()),
            region: None,
            profile: None,
            endpoint_url: None,
            force_path_style: false,
        }
    }

    #[tokio::test]
    async fn s3_without_frozen_spec_is_invalid_plan() {
        let plan = s3_plan(TransferMethod::S3, None);
        let registry = ProviderRegistry::new();
        let cancel = Arc::new(AtomicBool::new(false));
        let err = execute_transfer(&plan, &[], &registry, cancel, |_| {})
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            TransferExecutionError::InvalidPlan {
                method: TransferMethod::S3,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn s3_unknown_target_is_invalid_plan() {
        let plan = s3_plan(TransferMethod::S3, Some(s3_upload_spec("ghost", "bk")));
        let registry = ProviderRegistry::new();
        let cancel = Arc::new(AtomicBool::new(false));
        let err = execute_transfer(&plan, &[], &registry, cancel, |_| {})
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            TransferExecutionError::InvalidPlan {
                method: TransferMethod::S3,
                ..
            }
        ));
    }

    // Routing proof: a registered target resolves to the S3Provider and the S3
    // arm dispatches into upload_one, which fails locally (missing source) before
    // any AWS network work — proving the executor reached the core.
    #[tokio::test]
    async fn s3_upload_arm_dispatches_to_upload_core() {
        let registry = ProviderRegistry::new();
        registry.register_s3_targets(&[s3_target_config("tgt", "bk")]);
        let plan = s3_plan(TransferMethod::S3, Some(s3_upload_spec("tgt", "bk")));
        let cancel = Arc::new(AtomicBool::new(false));
        let err = execute_transfer(&plan, &[], &registry, cancel, |_| {})
            .await
            .unwrap_err();
        assert!(matches!(err, TransferExecutionError::Io(_)));
    }

    // Routing proof for download: registered target resolves; the S3 arm
    // dispatches into download_one, which fails locally (nonexistent destination
    // parent) before any GetObject network work.
    #[tokio::test]
    async fn s3_download_arm_dispatches_to_download_core() {
        let registry = ProviderRegistry::new();
        registry.register_s3_targets(&[s3_target_config("tgt", "bk")]);
        let spec = S3TransferSpec::DownloadOne {
            source: S3ObjectRef {
                target: "tgt".into(),
                bucket: "bk".into(),
                key: "a.txt".into(),
            },
            local_destination: PathBuf::from("/nonexistent-arx-dst/a.txt"),
        };
        let plan = s3_plan(TransferMethod::S3, Some(spec));
        let cancel = Arc::new(AtomicBool::new(false));
        let err = execute_transfer(&plan, &[], &registry, cancel, |_| {})
            .await
            .unwrap_err();
        assert!(matches!(err, TransferExecutionError::Io(_)));
    }
}
