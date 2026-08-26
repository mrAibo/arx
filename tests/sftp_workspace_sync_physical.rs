use std::error::Error;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use arx::jobs::{Job, JobEvent, JobManager, JobResult, JobStatus};
use arx::journal::OperationJournal;
use arx::remote::{Host, validate_ssh_alias};
use arx::services::{
    WorkspaceScanOptions, WorkspaceSyncController, WorkspaceSyncLaunchError, scan_workspace,
};
use arx::vfs::sftp::SftpProvider;
use arx::vfs::{Location, ProviderRegistry, capabilities};
use arx::workspace_sync::{
    ConflictPolicy, SyncMode, SyncPolicy, WorkspaceDiff, WorkspaceSyncPlan,
};
use arx::workspace_sync_execution::{SyncPlanValidator, SyncValidationError};
use arx::workspace_sync_executor::SyncTerminalState;
use arx::workspace_sync_verification::{
    SyncVerificationEvent, SyncVerificationResult, SyncVerificationSnapshot,
    SyncVerificationStatus, SyncVerificationVerdict,
};
use tokio::sync::mpsc;
use tokio::time::{sleep, timeout};

type AnyError = Box<dyn Error + Send + Sync>;

#[derive(Clone)]
struct Fixture {
    host_a: String,
    host_b: String,
    root_a: PathBuf,
    root_b: PathBuf,
}

struct StartedSync {
    id: String,
    jobs: JobManager,
    _job_rx: mpsc::UnboundedReceiver<JobEvent>,
    verification_rx: mpsc::UnboundedReceiver<SyncVerificationEvent>,
}

fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn ssh_run(host: &str, script: &str) -> io::Result<()> {
    let status = Command::new("ssh")
        .arg(host)
        .arg(format!("sh -c {}", sh_quote(script)))
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "ssh {host} exited with {status}: {script}"
        )))
    }
}

fn ssh_write(host: &str, path: &str, bytes: &[u8]) -> io::Result<()> {
    let script = format!("set -eu; umask 077; cat > {}", sh_quote(path));
    let mut child = Command::new("ssh")
        .arg(host)
        .arg(format!("sh -c {}", sh_quote(&script)))
        .stdin(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("ssh stdin unavailable"))?
        .write_all(bytes)?;
    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "ssh write {host}:{path} exited with {status}"
        )))
    }
}

fn ssh_read(host: &str, path: &str) -> io::Result<Vec<u8>> {
    let output = Command::new("ssh")
        .arg(host)
        .arg(format!("cat -- {}", sh_quote(path)))
        .output()?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(io::Error::other(format!(
            "ssh read {host}:{path} exited with {}",
            output.status
        )))
    }
}

fn ssh_exists(host: &str, path: &str) -> io::Result<bool> {
    Ok(Command::new("ssh")
        .arg(host)
        .arg(format!("test -e {}", sh_quote(path)))
        .status()?
        .success())
}

fn ssh_has_part_artifact(host: &str, root: &str) -> io::Result<bool> {
    let script = format!(
        "find {} -maxdepth 4 -type f -name '*.arx-part-*' -print -quit",
        sh_quote(root)
    );
    let output = Command::new("ssh")
        .arg(host)
        .arg(format!("sh -c {}", sh_quote(&script)))
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "artifact scan failed on {host}:{}",
            output.status
        )));
    }
    Ok(!output.stdout.is_empty())
}

fn fixture() -> Result<Fixture, AnyError> {
    if std::env::var("ARX_SFTP_SYNC_PHYSICAL").as_deref() != Ok("1") {
        return Err(io::Error::other(
            "ARX_SFTP_SYNC_PHYSICAL=1 is required for this ignored physical test",
        )
        .into());
    }
    let host_a = std::env::var("ARX_SFTP_SYNC_HOST_A")?;
    let host_b = std::env::var("ARX_SFTP_SYNC_HOST_B")?;
    validate_ssh_alias(&host_a)?;
    validate_ssh_alias(&host_b)?;
    if host_a == host_b {
        return Err(io::Error::other("physical fixture requires two distinct SSH aliases").into());
    }
    Ok(Fixture {
        host_a,
        host_b,
        root_a: PathBuf::from(std::env::var("ARX_SFTP_SYNC_ROOT_A")?),
        root_b: PathBuf::from(std::env::var("ARX_SFTP_SYNC_ROOT_B")?),
    })
}

fn registry(fixture: &Fixture) -> ProviderRegistry {
    let registry = ProviderRegistry::new();
    registry.insert_sftp(
        &fixture.host_a,
        Box::new(SftpProvider::new(Host::from_alias(&fixture.host_a))),
        capabilities::SFTP_CAPABILITIES,
    );
    registry.insert_sftp(
        &fixture.host_b,
        Box::new(SftpProvider::new(Host::from_alias(&fixture.host_b))),
        capabilities::SFTP_CAPABILITIES,
    );
    registry
}

fn sftp(host: &str, path: &Path) -> Location {
    Location::Sftp {
        host: host.to_string(),
        path: path.to_string_lossy().into_owned(),
    }
}

fn reset_root(host: &str, path: &Path) -> Result<(), AnyError> {
    let path = path.to_string_lossy();
    ssh_run(
        host,
        &format!(
            "set -eu; rm -rf -- {0}; mkdir -p -- {0}; chmod 700 -- {0}",
            sh_quote(&path)
        ),
    )?;
    Ok(())
}

fn cross_host_roots(
    fixture: &Fixture,
    case: &str,
) -> Result<(Location, Location, PathBuf, PathBuf), AnyError> {
    let left_path = fixture.root_a.join(format!("{case}-left"));
    let right_path = fixture.root_b.join(format!("{case}-right"));
    reset_root(&fixture.host_a, &left_path)?;
    reset_root(&fixture.host_b, &right_path)?;
    Ok((
        sftp(&fixture.host_a, &left_path),
        sftp(&fixture.host_b, &right_path),
        left_path,
        right_path,
    ))
}

fn same_host_roots(
    fixture: &Fixture,
    case: &str,
) -> Result<(Location, Location, PathBuf, PathBuf), AnyError> {
    let left_path = fixture.root_a.join(format!("{case}-left"));
    let right_path = fixture.root_a.join(format!("{case}-right"));
    reset_root(&fixture.host_a, &left_path)?;
    reset_root(&fixture.host_a, &right_path)?;
    Ok((
        sftp(&fixture.host_a, &left_path),
        sftp(&fixture.host_a, &right_path),
        left_path,
        right_path,
    ))
}

async fn fresh_diff(
    registry: &ProviderRegistry,
    left: &Location,
    right: &Location,
) -> Result<WorkspaceDiff, AnyError> {
    let cancel = AtomicBool::new(false);
    let left_entries = scan_workspace(
        registry,
        left,
        WorkspaceScanOptions::default(),
        &cancel,
    )
    .await?;
    let right_entries = scan_workspace(
        registry,
        right,
        WorkspaceScanOptions::default(),
        &cancel,
    )
    .await?;
    Ok(WorkspaceDiff::compare(
        left.clone(),
        right.clone(),
        left_entries,
        right_entries,
    ))
}

fn journal_path() -> PathBuf {
    let token = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "arx-sftp-sync-physical-{}-{token}.jsonl",
        std::process::id()
    ))
}

async fn start_sync(
    registry: ProviderRegistry,
    diff: WorkspaceDiff,
    policy: SyncPolicy,
    confirmed: bool,
) -> Result<StartedSync, AnyError> {
    let controller = WorkspaceSyncController::with_journal(
        registry,
        OperationJournal::open(journal_path())?,
    );
    let logical = WorkspaceSyncPlan::build(&diff, policy);
    let frozen = controller.freeze(&logical, &diff)?;
    let jobs = JobManager::new();
    let (job_tx, job_rx) = mpsc::unbounded_channel();
    let (verification_tx, verification_rx) = mpsc::unbounded_channel();
    let id = controller
        .launch(
            frozen,
            diff,
            confirmed,
            jobs.clone(),
            job_tx,
            verification_tx,
        )
        .await?;
    Ok(StartedSync {
        id,
        jobs,
        _job_rx: job_rx,
        verification_rx,
    })
}

async fn wait_terminal(started: &StartedSync) -> Result<Job, AnyError> {
    let id = started.id.clone();
    let jobs = started.jobs.clone();
    timeout(Duration::from_secs(90), async move {
        loop {
            if let Some(job) = jobs.snapshot().into_iter().find(|job| job.id == id)
                && job.status.is_terminal()
            {
                return job;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "workspace job timed out"))
    .map_err(Into::into)
}

async fn wait_verification(
    started: &mut StartedSync,
) -> Result<SyncVerificationSnapshot, AnyError> {
    let id = started.id.clone();
    timeout(Duration::from_secs(90), async {
        loop {
            let event = started.verification_rx.recv().await.ok_or_else(|| {
                io::Error::new(io::ErrorKind::UnexpectedEof, "verification channel closed")
            })?;
            if event.job_id == id && event.verification.status.is_terminal() {
                return Ok::<_, io::Error>(event.verification);
            }
        }
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "sync verification timed out"))??
    .pipe(Ok)
}

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}
impl<T> Pipe for T {}

fn finished_verification(
    snapshot: &SyncVerificationSnapshot,
) -> Result<&SyncVerificationResult, AnyError> {
    match &snapshot.status {
        SyncVerificationStatus::Finished(result) => Ok(result.as_ref()),
        other => Err(io::Error::other(format!(
            "expected finished verification, got {other:?}"
        ))
        .into()),
    }
}

fn assert_completed(job: &Job) {
    assert_eq!(job.status, JobStatus::Completed, "job={job:?}");
    assert!(matches!(
        job.result.as_ref(),
        Some(JobResult::WorkspaceSync(outcome))
            if matches!(outcome.terminal, SyncTerminalState::Completed)
    ));
}

fn assert_observed_destination(
    verification: &SyncVerificationResult,
    relative_path: &str,
    expected_size: u64,
) {
    let entry = verification
        .diff
        .entries
        .iter()
        .find(|entry| entry.relative_path == relative_path)
        .unwrap_or_else(|| panic!("verification did not observe {relative_path}"));
    assert_eq!(
        entry.right.as_ref().and_then(|fingerprint| fingerprint.size),
        Some(expected_size),
        "verification={verification:?}"
    );
}

async fn case_cross_host_update(fixture: &Fixture) -> Result<(), AnyError> {
    let (left, right, left_path, right_path) = cross_host_roots(fixture, "update")?;
    let bytes = b"cross-host update\n";
    ssh_write(
        &fixture.host_a,
        &left_path.join("hello.txt").to_string_lossy(),
        bytes,
    )?;

    let reg = registry(fixture);
    let diff = fresh_diff(&reg, &left, &right).await?;
    let mut started = start_sync(reg, diff, SyncPolicy::default(), false).await?;
    let job = wait_terminal(&started).await?;
    assert_completed(&job);
    let verification = wait_verification(&mut started).await?;
    let verification = finished_verification(&verification)?;
    assert_observed_destination(verification, "hello.txt", bytes.len() as u64);
    assert_eq!(
        ssh_read(
            &fixture.host_b,
            &right_path.join("hello.txt").to_string_lossy()
        )?,
        bytes
    );
    Ok(())
}

async fn case_cross_host_replacement(fixture: &Fixture) -> Result<(), AnyError> {
    let (left, right, left_path, right_path) = cross_host_roots(fixture, "replace")?;
    let source = b"replacement-new-content\n";
    let source_path = left_path.join("replace.txt");
    let target_path = right_path.join("replace.txt");
    ssh_write(&fixture.host_a, &source_path.to_string_lossy(), source)?;
    ssh_write(&fixture.host_b, &target_path.to_string_lossy(), b"old\n")?;
    ssh_run(
        &fixture.host_a,
        &format!("touch -m -d @1700000100 -- {}", sh_quote(&source_path.to_string_lossy())),
    )?;
    ssh_run(
        &fixture.host_b,
        &format!("touch -m -d @1700000000 -- {}", sh_quote(&target_path.to_string_lossy())),
    )?;

    let reg = registry(fixture);
    let diff = fresh_diff(&reg, &left, &right).await?;
    let policy = SyncPolicy {
        conflicts: ConflictPolicy::PreferSource,
        ..SyncPolicy::default()
    };
    let mut started = start_sync(reg, diff, policy, false).await?;
    let job = wait_terminal(&started).await?;
    assert_completed(&job);
    let verification = wait_verification(&mut started).await?;
    assert_observed_destination(
        finished_verification(&verification)?,
        "replace.txt",
        source.len() as u64,
    );
    assert_eq!(ssh_read(&fixture.host_b, &target_path.to_string_lossy())?, source);
    assert!(!ssh_has_part_artifact(
        &fixture.host_b,
        &right_path.to_string_lossy()
    )?);
    Ok(())
}

async fn case_nested_directories(fixture: &Fixture) -> Result<(), AnyError> {
    let (left, right, left_path, right_path) = cross_host_roots(fixture, "nested")?;
    ssh_run(
        &fixture.host_a,
        &format!(
            "mkdir -p -- {}; chmod 700 -- {}",
            sh_quote(&left_path.join("a/b").to_string_lossy()),
            sh_quote(&left_path.join("a/b").to_string_lossy())
        ),
    )?;
    let bytes = b"nested-data\n";
    ssh_write(
        &fixture.host_a,
        &left_path.join("a/b/file.txt").to_string_lossy(),
        bytes,
    )?;

    let reg = registry(fixture);
    let diff = fresh_diff(&reg, &left, &right).await?;
    let mut started = start_sync(reg, diff, SyncPolicy::default(), false).await?;
    assert_completed(&wait_terminal(&started).await?);
    let verification = wait_verification(&mut started).await?;
    assert_observed_destination(
        finished_verification(&verification)?,
        "a/b/file.txt",
        bytes.len() as u64,
    );
    assert_eq!(
        ssh_read(
            &fixture.host_b,
            &right_path.join("a/b/file.txt").to_string_lossy()
        )?,
        bytes
    );
    Ok(())
}

async fn case_mirror_delete_and_verify(fixture: &Fixture) -> Result<(), AnyError> {
    let (left, right, _left_path, right_path) = cross_host_roots(fixture, "mirror")?;
    let obsolete = right_path.join("obsolete.txt");
    let empty_dir = right_path.join("empty-dir");
    ssh_write(&fixture.host_b, &obsolete.to_string_lossy(), b"obsolete\n")?;
    ssh_run(
        &fixture.host_b,
        &format!("mkdir -p -- {}", sh_quote(&empty_dir.to_string_lossy())),
    )?;

    let reg = registry(fixture);
    let diff = fresh_diff(&reg, &left, &right).await?;
    let policy = SyncPolicy {
        mode: SyncMode::Mirror,
        ..SyncPolicy::default()
    };
    let mut started = start_sync(reg, diff, policy, true).await?;
    assert_completed(&wait_terminal(&started).await?);
    let verification = wait_verification(&mut started).await?;
    let verification = finished_verification(&verification)?;
    assert!(matches!(
        verification.verdict,
        SyncVerificationVerdict::Synchronized
    ));
    assert!(!ssh_exists(&fixture.host_b, &obsolete.to_string_lossy())?);
    assert!(!ssh_exists(&fixture.host_b, &empty_dir.to_string_lossy())?);
    Ok(())
}

async fn case_same_host_copy(fixture: &Fixture) -> Result<(), AnyError> {
    let (left, right, left_path, right_path) = same_host_roots(fixture, "same-host")?;
    let bytes = b"same-host-streamed-copy\n";
    ssh_write(
        &fixture.host_a,
        &left_path.join("same.txt").to_string_lossy(),
        bytes,
    )?;

    let reg = registry(fixture);
    let diff = fresh_diff(&reg, &left, &right).await?;
    let mut started = start_sync(reg, diff, SyncPolicy::default(), false).await?;
    assert_completed(&wait_terminal(&started).await?);
    let verification = wait_verification(&mut started).await?;
    assert_observed_destination(
        finished_verification(&verification)?,
        "same.txt",
        bytes.len() as u64,
    );
    assert_eq!(
        ssh_read(
            &fixture.host_a,
            &right_path.join("same.txt").to_string_lossy()
        )?,
        bytes
    );
    Ok(())
}

async fn wait_for_stream_stage(dir: &Path) -> Result<(), AnyError> {
    timeout(Duration::from_secs(30), async {
        loop {
            let mut entries = tokio::fs::read_dir(dir).await?;
            while let Some(entry) = entries.next_entry().await? {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.contains(".arx-part-") && entry.metadata().await?.len() > 0 {
                    return Ok::<_, io::Error>(());
                }
            }
            tokio::task::yield_now().await;
            sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "stream stage was never observed"))??;
    Ok(())
}

async fn case_cancellation_during_stream(fixture: &Fixture) -> Result<(), AnyError> {
    let (left, right, left_path, right_path) = cross_host_roots(fixture, "cancel")?;
    let source = left_path.join("big.bin");
    ssh_run(
        &fixture.host_a,
        &format!(
            "dd if=/dev/zero of={} bs=1M count=256 status=none",
            sh_quote(&source.to_string_lossy())
        ),
    )?;

    let reg = registry(fixture);
    let diff = fresh_diff(&reg, &left, &right).await?;
    let mut started = start_sync(reg, diff, SyncPolicy::default(), false).await?;
    wait_for_stream_stage(&right_path).await?;
    assert!(started.jobs.cancel(&started.id));
    let job = wait_terminal(&started).await?;
    assert_eq!(job.status, JobStatus::Cancelled, "job={job:?}");
    assert!(matches!(
        job.result.as_ref(),
        Some(JobResult::WorkspaceSync(outcome))
            if matches!(outcome.terminal, SyncTerminalState::Cancelled { .. })
    ));
    let verification = wait_verification(&mut started).await?;
    let _ = finished_verification(&verification)?;
    assert!(!ssh_exists(
        &fixture.host_b,
        &right_path.join("big.bin").to_string_lossy()
    )?);
    assert!(!ssh_has_part_artifact(
        &fixture.host_b,
        &right_path.to_string_lossy()
    )?);
    Ok(())
}

async fn stale_launch_error(
    controller: &WorkspaceSyncController,
    frozen: arx::workspace_sync_execution::FrozenWorkspaceSyncPlan,
    original: WorkspaceDiff,
) -> Result<(WorkspaceSyncLaunchError, JobManager), AnyError> {
    let jobs = JobManager::new();
    let (job_tx, _job_rx) = mpsc::unbounded_channel();
    let (verification_tx, _verification_rx) = mpsc::unbounded_channel();
    let error = controller
        .launch(
            frozen,
            original,
            false,
            jobs.clone(),
            job_tx,
            verification_tx,
        )
        .await
        .expect_err("stale frozen plan must fail before job creation");
    Ok((error, jobs))
}

async fn case_stale_preview_fails_closed(fixture: &Fixture) -> Result<(), AnyError> {
    let (left, right, left_path, right_path) = cross_host_roots(fixture, "stale-source")?;
    let source = left_path.join("stale.txt");
    ssh_write(&fixture.host_a, &source.to_string_lossy(), b"old\n")?;
    let reg = registry(fixture);
    let original = fresh_diff(&reg, &left, &right).await?;
    let controller = WorkspaceSyncController::with_journal(
        reg,
        OperationJournal::open(journal_path())?,
    );
    let logical = WorkspaceSyncPlan::build(&original, SyncPolicy::default());
    let frozen = SyncPlanValidator::freeze(&logical, &original, &registry(fixture))?;
    ssh_write(&fixture.host_a, &source.to_string_lossy(), b"source changed\n")?;
    let (error, jobs) = stale_launch_error(&controller, frozen, original).await?;
    assert!(matches!(
        error,
        WorkspaceSyncLaunchError::Validation(SyncValidationError::SourceChanged { .. })
    ));
    assert!(jobs.snapshot().is_empty());

    let (left, right, left_path, right_path) = cross_host_roots(fixture, "stale-destination")?;
    let source = left_path.join("stale.txt");
    let target = right_path.join("stale.txt");
    ssh_write(&fixture.host_a, &source.to_string_lossy(), b"source\n")?;
    let reg = registry(fixture);
    let original = fresh_diff(&reg, &left, &right).await?;
    let controller = WorkspaceSyncController::with_journal(
        reg,
        OperationJournal::open(journal_path())?,
    );
    let logical = WorkspaceSyncPlan::build(&original, SyncPolicy::default());
    let frozen = SyncPlanValidator::freeze(&logical, &original, &registry(fixture))?;
    ssh_write(&fixture.host_b, &target.to_string_lossy(), b"appeared\n")?;
    let (error, jobs) = stale_launch_error(&controller, frozen, original).await?;
    assert!(matches!(
        error,
        WorkspaceSyncLaunchError::Validation(SyncValidationError::DestinationChanged { .. })
    ));
    assert!(jobs.snapshot().is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires disposable two-endpoint OpenSSH fixture from setup_sftp_sync_acceptance.sh"]
async fn physical_sftp_workspace_sync_matrix() -> Result<(), AnyError> {
    let fixture = fixture()?;

    case_cross_host_update(&fixture).await?;
    case_cross_host_replacement(&fixture).await?;
    case_nested_directories(&fixture).await?;
    case_mirror_delete_and_verify(&fixture).await?;
    case_same_host_copy(&fixture).await?;
    case_cancellation_during_stream(&fixture).await?;
    case_stale_preview_fails_closed(&fixture).await?;

    Ok(())
}
