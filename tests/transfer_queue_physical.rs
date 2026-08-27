use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use arx::jobs::{JobManager, JobProgress, JobStatus, Progress};
use arx::transfer::{TransferIntent, TransferMethod, TransferPlan};
use arx::transfer_queue::{PauseAction, TransferQueueConfig};
use arx::transfer_queue_runtime::TransferQueueRuntime;
use arx::vfs::Location;

// Kept identical to the contract-test helper: integration tests are separate crates,
// so tests/transfer_queue_contracts.rs cannot export its private helper directly.
fn local_runtime(concurrency: usize) -> (TransferQueueRuntime, PathBuf) {
    let manager = JobManager::new();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let registry = arx::vfs::ProviderRegistry::new();
    let config = TransferQueueConfig::new(concurrency).unwrap();
    let runtime = TransferQueueRuntime::new(manager, tx, registry, config);
    let scratch = std::env::temp_dir().join(format!(
        "arx-queue-contract-{}-{}",
        std::process::id(),
        concurrency
    ));
    let _ = std::fs::create_dir_all(&scratch);
    (runtime, scratch)
}

fn local_copy_plan(src: &Path, dst: &Path) -> TransferPlan {
    TransferPlan {
        source: Location::Local(src.to_path_buf()),
        destination: Location::Local(dst.to_path_buf()),
        intent: TransferIntent::Copy,
        method: TransferMethod::Native,
        archive_spec: None,
        s3_spec: None,
        webdav_spec: None,
    }
}

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn unique_scratch(root: &Path, label: &str) -> Scratch {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = root.join(format!("physical-{label}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    Scratch(path)
}

fn make_source(root: &Path, count: usize, bytes: usize) -> (PathBuf, Vec<String>) {
    let source = root.join("source");
    std::fs::create_dir_all(&source).unwrap();
    let payload = vec![0x5a; bytes];
    let names = (0..count)
        .map(|index| format!("file-{index:05}.bin"))
        .collect::<Vec<_>>();
    for name in &names {
        std::fs::write(source.join(name), &payload).unwrap();
    }
    (source, names)
}

fn status(runtime: &TransferQueueRuntime, id: &str) -> JobStatus {
    runtime.manager().get(id).unwrap().status
}

fn item_progress(runtime: &TransferQueueRuntime, id: &str) -> (usize, usize) {
    match runtime.manager().get(id).unwrap().progress {
        JobProgress::Generic(Progress::Items { done, total }) => (done, total),
        JobProgress::Generic(Progress::Indeterminate) => (0, 0),
        other => panic!("unexpected Local transfer progress: {other:?}"),
    }
}

async fn wait_until(mut condition: impl FnMut() -> bool, message: &str) {
    tokio::time::timeout(Duration::from_secs(30), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out: {message}"));
}

fn tree(root: &Path) -> Vec<(PathBuf, u64)> {
    fn walk(base: &Path, path: &Path, out: &mut Vec<(PathBuf, u64)>) {
        if !path.exists() {
            return;
        }
        let mut entries = std::fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap())
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let metadata = entry.metadata().unwrap();
            if metadata.is_dir() {
                walk(base, &path, out);
            } else {
                out.push((
                    path.strip_prefix(base).unwrap().to_path_buf(),
                    metadata.len(),
                ));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out
}

fn assert_no_partials(root: &Path) {
    let leftovers = tree(root)
        .into_iter()
        .filter(|(path, _)| {
            let name = path.file_name().unwrap().to_string_lossy();
            name.contains(".part")
                || name.contains(".partial")
                || name.ends_with(".tmp")
                || name.starts_with(".arx-")
        })
        .collect::<Vec<_>>();
    assert!(leftovers.is_empty(), "orphan temp artifacts: {leftovers:?}");
}

fn assert_existing_files_byte_exact(source: &Path, destination: &Path) {
    for (relative, bytes) in tree(destination) {
        assert_eq!(
            bytes,
            std::fs::metadata(source.join(&relative)).unwrap().len(),
            "destination artifact is not byte-exact: {relative:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p1_p12_real_local_transfer_queue_acceptance() {
    const FILES: usize = 6_000;
    const BYTES: usize = 16 * 1024;

    // P1-P3: real overlapping filesystem work, FIFO waiting, isolated cancel.
    let (runtime, root) = local_runtime(2);
    let scratch = unique_scratch(&root, "parallel");
    let (source, names) = make_source(&scratch.0, FILES, BYTES);
    let destinations = (1..=3)
        .map(|n| {
            let path = scratch.0.join(format!("destination-{n}"));
            std::fs::create_dir_all(&path).unwrap();
            path
        })
        .collect::<Vec<_>>();
    let ids = destinations
        .iter()
        .map(|destination| {
            runtime
                .enqueue(local_copy_plan(&source, destination), names.clone())
                .unwrap()
        })
        .collect::<Vec<_>>();

    wait_until(
        || {
            ids[..2].iter().all(|id| {
                let (done, total) = item_progress(&runtime, id);
                status(&runtime, id) == JobStatus::Running && done > 0 && done < total
            })
        },
        "two workers both make in-flight Local I/O progress",
    )
    .await;
    let overlap = [
        item_progress(&runtime, &ids[0]),
        item_progress(&runtime, &ids[1]),
    ];
    let summary = runtime.summary();
    assert_eq!((summary.running, summary.waiting), (2, 1));
    assert_eq!(status(&runtime, &ids[2]), JobStatus::Pending);
    println!("P1 PASS two active Local workers, progress={overlap:?}");
    println!(
        "P2 waiting observed: third={:?}, summary={summary:?}",
        status(&runtime, &ids[2])
    );

    runtime.cancel(&ids[0]).unwrap();
    wait_until(
        || status(&runtime, &ids[0]) == JobStatus::Cancelled,
        "cancelled worker terminalizes",
    )
    .await;
    wait_until(
        || {
            let third = status(&runtime, &ids[2]);
            matches!(third, JobStatus::Running | JobStatus::Completed)
                && item_progress(&runtime, &ids[2]).0 > 0
        },
        "third worker starts after cancelled slot frees",
    )
    .await;
    println!(
        "P2 PASS third transitioned Pending->Running/Completed after first={:?}; third_progress={:?}",
        status(&runtime, &ids[0]),
        item_progress(&runtime, &ids[2])
    );
    wait_until(
        || status(&runtime, &ids[1]) == JobStatus::Completed,
        "unrelated active copy completes after peer cancellation",
    )
    .await;
    assert_eq!(status(&runtime, &ids[0]), JobStatus::Cancelled);
    println!(
        "P3 PASS cancelled={:?}, unrelated={:?}",
        status(&runtime, &ids[0]),
        status(&runtime, &ids[1])
    );
    runtime.shutdown().await;

    // P4-P7: pause at a real per-file checkpoint, stable progress, same JobId, exact result.
    let (runtime, root) = local_runtime(1);
    let scratch = unique_scratch(&root, "pause");
    let (source, names) = make_source(&scratch.0, FILES, BYTES);
    let destination = scratch.0.join("destination");
    std::fs::create_dir_all(&destination).unwrap();
    let id = runtime
        .enqueue(local_copy_plan(&source, &destination), names.clone())
        .unwrap();
    wait_until(
        || {
            let (done, total) = item_progress(&runtime, &id);
            status(&runtime, &id) == JobStatus::Running && done > 0 && done < total
        },
        "copy enters measurable in-flight state",
    )
    .await;
    let before_pause = item_progress(&runtime, &id);
    assert_eq!(
        runtime.request_pause(&id),
        Ok(PauseAction::AwaitSafeCheckpoint)
    );
    // The checkpoint waiter is spawned before request_pause() returns, so a fast
    // executor may already have completed the truthful PausePending -> Paused
    // transition by the caller's first status read.
    let immediate_pause_status = status(&runtime, &id);
    assert!(
        matches!(
            immediate_pause_status,
            JobStatus::PausePending | JobStatus::Paused
        ),
        "pause request must be pending or already confirmed, got {immediate_pause_status:?}"
    );
    wait_until(
        || status(&runtime, &id) == JobStatus::Paused,
        "pause reaches a real executor checkpoint",
    )
    .await;
    let paused_1 = item_progress(&runtime, &id);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let paused_2 = item_progress(&runtime, &id);
    assert_eq!(paused_1, paused_2);
    assert_eq!(runtime.manager().snapshot().len(), 1);
    assert_eq!(runtime.manager().snapshot()[0].id, id);
    runtime.resume(&id).unwrap();
    assert_eq!(status(&runtime, &id), JobStatus::Running);
    let resumed = item_progress(&runtime, &id);
    assert!(resumed.0 >= paused_2.0, "resume must not reset progress");
    wait_until(
        || status(&runtime, &id) == JobStatus::Completed,
        "resumed worker completes",
    )
    .await;
    let final_progress = item_progress(&runtime, &id);
    assert_eq!(final_progress, (FILES, FILES));
    assert_eq!(
        runtime.manager().get(&id).unwrap().progress.percent(),
        Some(100)
    );
    assert_eq!(tree(&destination).len(), FILES);
    let sample = &names[FILES / 2];
    assert_eq!(
        std::fs::metadata(source.join(sample)).unwrap().len(),
        std::fs::metadata(destination.join(sample)).unwrap().len()
    );
    assert_eq!(
        std::fs::metadata(destination.join(sample)).unwrap().len(),
        BYTES as u64
    );
    println!(
        "P4 PASS Running({before_pause:?})->PausePending->Paused({paused_1:?})->Running({resumed:?})->Completed"
    );
    println!("P5 PASS paused progress stable for 200ms: {paused_1:?} == {paused_2:?}");
    println!(
        "P6 observable subset PASS same JobId={id}, one JobManager record, progress not reset; attempt counter is not public"
    );
    println!(
        "P7 PASS destination_files={FILES}, sample_bytes={BYTES}, progress={final_progress:?}, percent=100"
    );
    runtime.shutdown().await;

    // P11-P12: shutdown an active worker and a truly paused worker, then prove quiescence.
    let (runtime, root) = local_runtime(2);
    let scratch = unique_scratch(&root, "shutdown");
    let (source, names) = make_source(&scratch.0, FILES, BYTES);
    let active_destination = scratch.0.join("active-destination");
    let paused_destination = scratch.0.join("paused-destination");
    std::fs::create_dir_all(&active_destination).unwrap();
    std::fs::create_dir_all(&paused_destination).unwrap();
    let active = runtime
        .enqueue(local_copy_plan(&source, &active_destination), names.clone())
        .unwrap();
    let paused = runtime
        .enqueue(local_copy_plan(&source, &paused_destination), names)
        .unwrap();
    wait_until(
        || item_progress(&runtime, &active).0 > 0 && item_progress(&runtime, &paused).0 > 0,
        "both shutdown-test workers make real progress",
    )
    .await;
    runtime.request_pause(&paused).unwrap();
    wait_until(
        || status(&runtime, &paused) == JobStatus::Paused,
        "shutdown-test worker pauses",
    )
    .await;
    assert_eq!(status(&runtime, &active), JobStatus::Running);
    runtime.shutdown().await;
    assert!(
        runtime
            .manager()
            .snapshot()
            .iter()
            .all(|job| job.status.is_terminal())
    );
    let summary = runtime.summary();
    assert_eq!(
        (summary.running, summary.waiting, summary.paused),
        (0, 0, 0)
    );
    assert_no_partials(&scratch.0);
    assert_existing_files_byte_exact(&source, &active_destination);
    assert_existing_files_byte_exact(&source, &paused_destination);
    let settled = tree(&scratch.0);
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(
        settled,
        tree(&scratch.0),
        "filesystem mutated after shutdown joined"
    );
    println!(
        "P11 PASS shutdown returned terminal statuses active={:?}, paused={:?}, summary={summary:?}; tree stable for 250ms",
        status(&runtime, &active),
        status(&runtime, &paused)
    );
    println!(
        "P12 PASS no .part/.partial/.tmp/.arx-* artifacts; stable entries={}",
        settled.len()
    );

    println!("P8 exercised separately by transfer_queue_s3_retry_physical against real MinIO");
    println!("P9/P10 PROVIDER-SCOPED: no WebDAV/SFTP server used by this Local-only battery");
}
