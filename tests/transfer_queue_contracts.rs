use std::time::{Duration, Instant};

use arx::jobs::{JobKind, JobManager, JobStatus};
use arx::transfer::{TransferIntent, TransferMethod, TransferPlan};
use arx::transfer_queue::{
    CancelAction, DEFAULT_TRANSFER_CONCURRENCY, MAX_TOTAL_TRANSFER_ATTEMPTS, PauseAction,
    QueueError, RetryDecision, RetryDisposition, SchedulerState, TransferProgressTracker,
    TransferQueueConfig, TransferQueueCore, TransferRateEstimator, TypedTransferProgress,
};
use arx::transfer_queue_runtime::TransferQueueRuntime;
use arx::vfs::Location;

/// Build a runtime driving Local<->Local transfers through the real executor.
/// No second engine: `enqueue`/`cancel` exercise the same `execute_transfer`
/// path the TUI uses.
fn local_runtime(concurrency: usize) -> (TransferQueueRuntime, std::path::PathBuf) {
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

fn local_copy_plan(src: &std::path::Path, dst: &std::path::Path) -> TransferPlan {
    TransferPlan {
        source: Location::Local(src.to_path_buf()),
        destination: Location::Local(dst.to_path_buf()),
        intent: TransferIntent::Copy,
        method: TransferMethod::Native,
        s3_spec: None,
        webdav_spec: None,
    }
}

#[test]
fn queue_contract_default_parallelism_is_two_and_fifo() {
    let config = TransferQueueConfig::default();
    assert_eq!(config.concurrency(), DEFAULT_TRANSFER_CONCURRENCY);

    let mut queue = TransferQueueCore::new(config);
    for id in ["job-a", "job-b", "job-c"] {
        queue.enqueue(id).unwrap();
    }

    assert_eq!(queue.next_runnable().as_deref(), Some("job-a"));
    assert_eq!(queue.next_runnable().as_deref(), Some("job-b"));
    assert_eq!(queue.active_count(), 2);
    assert!(queue.next_runnable().is_none());

    queue.finish("job-a").unwrap();
    assert_eq!(queue.next_runnable().as_deref(), Some("job-c"));
    assert_eq!(queue.active_count(), 2);
}

#[test]
fn queue_contract_queued_cancel_never_consumes_an_attempt() {
    let mut queue = TransferQueueCore::default();
    queue.enqueue("cancel-me").unwrap();
    queue.enqueue("run-me").unwrap();

    assert_eq!(
        queue.request_cancel("cancel-me").unwrap(),
        CancelAction::TerminalizeWithoutExecution
    );
    assert_eq!(queue.attempts_started("cancel-me"), Some(0));
    assert_eq!(queue.next_runnable().as_deref(), Some("run-me"));
}

#[test]
fn queue_contract_pause_is_pending_until_safe_checkpoint() {
    let mut queue = TransferQueueCore::new(TransferQueueConfig::new(1).unwrap());
    queue.enqueue("job-a").unwrap();
    queue.enqueue("job-b").unwrap();
    assert_eq!(queue.next_runnable().as_deref(), Some("job-a"));

    assert_eq!(
        queue.request_pause("job-a").unwrap(),
        PauseAction::AwaitSafeCheckpoint
    );
    assert_eq!(queue.state("job-a"), Some(SchedulerState::PausePending));
    assert!(queue.next_runnable().is_none());

    queue.confirm_paused("job-a").unwrap();
    assert_eq!(queue.state("job-a"), Some(SchedulerState::Parked));
    assert_eq!(queue.next_runnable().as_deref(), Some("job-b"));
}

#[test]
fn queue_contract_resume_keeps_job_id_and_attempt_number() {
    let mut queue = TransferQueueCore::new(TransferQueueConfig::new(1).unwrap());
    queue.enqueue("same-job").unwrap();
    assert_eq!(queue.next_runnable().as_deref(), Some("same-job"));
    assert_eq!(queue.attempts_started("same-job"), Some(1));

    queue.request_pause("same-job").unwrap();
    queue.confirm_paused("same-job").unwrap();
    queue.resume("same-job").unwrap();

    assert_eq!(queue.next_runnable().as_deref(), Some("same-job"));
    assert_eq!(queue.attempts_started("same-job"), Some(1));
}

#[test]
fn queue_contract_safe_retry_is_three_total_attempts_not_four() {
    let mut queue = TransferQueueCore::new(TransferQueueConfig::new(1).unwrap());
    queue.enqueue("job").unwrap();

    for attempt in 1..=MAX_TOTAL_TRANSFER_ATTEMPTS {
        assert_eq!(queue.next_runnable().as_deref(), Some("job"));
        assert_eq!(queue.attempts_started("job"), Some(attempt));

        let decision = queue.failure("job", RetryDisposition::SafeToRetry).unwrap();
        if attempt < MAX_TOTAL_TRANSFER_ATTEMPTS {
            assert_eq!(
                decision,
                RetryDecision::RetryAfterBackoff {
                    next_attempt: attempt + 1,
                    max_total_attempts: MAX_TOTAL_TRANSFER_ATTEMPTS,
                }
            );
            queue.release_retry("job").unwrap();
        } else {
            assert_eq!(
                decision,
                RetryDecision::Stop {
                    disposition: RetryDisposition::SafeToRetry,
                    attempts_started: MAX_TOTAL_TRANSFER_ATTEMPTS,
                }
            );
        }
    }

    assert_eq!(queue.state("job"), Some(SchedulerState::Finished));
}

#[test]
fn queue_contract_ambiguous_and_recovery_failures_are_one_shot() {
    for disposition in [
        RetryDisposition::AmbiguousMutation,
        RetryDisposition::RecoveryRequired,
        RetryDisposition::NeverRetry,
    ] {
        let mut queue = TransferQueueCore::new(TransferQueueConfig::new(1).unwrap());
        queue.enqueue("job").unwrap();
        assert_eq!(queue.next_runnable().as_deref(), Some("job"));

        assert_eq!(
            queue.failure("job", disposition).unwrap(),
            RetryDecision::Stop {
                disposition,
                attempts_started: 1,
            }
        );
        assert_eq!(queue.attempts_started("job"), Some(1));
        assert!(queue.next_runnable().is_none());
    }
}

#[test]
fn queue_contract_progress_cannot_regress_or_change_units_mid_attempt() {
    let mut tracker = TransferProgressTracker::default();
    tracker
        .observe(TypedTransferProgress::Bytes {
            completed: 64,
            total: Some(256),
        })
        .unwrap();
    tracker
        .observe(TypedTransferProgress::Bytes {
            completed: 128,
            total: Some(256),
        })
        .unwrap();

    assert!(matches!(
        tracker.observe(TypedTransferProgress::Bytes {
            completed: 127,
            total: Some(256),
        }),
        Err(QueueError::ProgressRegressed { .. })
    ));
    assert_eq!(
        tracker.observe(TypedTransferProgress::Items {
            completed: 128,
            total: Some(256),
        }),
        Err(QueueError::ProgressUnitChanged)
    );
}

#[test]
fn queue_contract_unknown_total_has_no_fake_percentage() {
    assert_eq!(
        TypedTransferProgress::Bytes {
            completed: 1_024,
            total: None,
        }
        .percent(),
        None
    );
    assert_eq!(
        TypedTransferProgress::Items {
            completed: 0,
            total: Some(0),
        }
        .percent(),
        None
    );
}

#[test]
fn queue_contract_rate_and_eta_use_monotonic_injected_time() {
    let start = Instant::now();
    let mut estimator = TransferRateEstimator::new(Duration::from_secs(4));

    assert_eq!(estimator.observe_at(start, 0).unwrap(), 0);
    assert_eq!(
        estimator
            .observe_at(start + Duration::from_secs(2), 2_000)
            .unwrap(),
        1_000
    );
    assert_eq!(
        estimator.eta(2_000, Some(5_000)),
        Some(Duration::from_secs(3))
    );
    assert_eq!(estimator.eta(2_000, None), None);
}

// ── Product-path contracts (§6 / §18 I1–I10) ───────────────────────────────
// These drive the real `TransferQueueRuntime` + `execute_transfer` (no second
// engine). The test runtime is current-thread so spawned `run_one` tasks only
// execute when the test awaits, keeping queue/cancel assertions deterministic.

#[tokio::test(flavor = "current_thread")]
async fn product_path_copy_routes_through_persistent_runtime() {
    // I1 + I2 + I3: one JobId, one JobManager lifecycle, one executor call.
    let (runtime, scratch) = local_runtime(DEFAULT_TRANSFER_CONCURRENCY);
    let src = scratch.join("src");
    let dst = scratch.join("dst");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("f.txt"), b"hello").unwrap();
    std::fs::create_dir_all(&dst).unwrap();

    let id = runtime
        .enqueue(local_copy_plan(&src, &dst), vec!["f.txt".into()])
        .expect("enqueue");

    // I2: exactly one transfer job exists (no second spawn / manual JobManager).
    let snap = runtime.manager().snapshot();
    assert_eq!(snap.len(), 1, "one job created");
    assert_eq!(snap[0].id, id, "same authoritative JobId");
    assert_eq!(snap[0].kind, JobKind::Transfer);

    // Let the real executor run to terminal.
    tokio::task::yield_now().await;
    let mut waited = 0;
    while runtime.manager().snapshot()[0].status == JobStatus::Running && waited < 100 {
        tokio::time::sleep(Duration::from_millis(10)).await;
        waited += 1;
    }
    // I3: the same JobId reached a terminal state.
    let final_status = runtime.manager().snapshot()[0].status;
    assert!(
        matches!(
            final_status,
            JobStatus::Completed | JobStatus::Cancelled | JobStatus::Failed
        ),
        "job {id} reached terminal: {final_status:?}"
    );
    assert!(dst.join("f.txt").exists(), "copy actually happened");
}

#[tokio::test(flavor = "current_thread")]
async fn product_path_move_uses_same_runtime() {
    // I8: Move enqueues through the same runtime, one Transfer job.
    let (runtime, scratch) = local_runtime(DEFAULT_TRANSFER_CONCURRENCY);
    let src = scratch.join("src");
    let dst = scratch.join("dst");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("m.txt"), b"move-me").unwrap();
    std::fs::create_dir_all(&dst).unwrap();

    let mut plan = local_copy_plan(&src, &dst);
    plan.intent = TransferIntent::Move;

    let id = runtime
        .enqueue(plan, vec!["m.txt".into()])
        .expect("enqueue move");
    let snap = runtime.manager().snapshot();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].kind, JobKind::Transfer);

    tokio::task::yield_now().await;
    let mut waited = 0;
    while runtime.manager().snapshot()[0].status == JobStatus::Running && waited < 100 {
        tokio::time::sleep(Duration::from_millis(10)).await;
        waited += 1;
    }
    assert!(dst.join("m.txt").exists(), "move actually happened");
    assert!(!src.join("m.txt").exists(), "move source consumed");
    assert_eq!(runtime.manager().snapshot()[0].id, id);
}

#[tokio::test(flavor = "current_thread")]
async fn product_path_third_job_queues_at_concurrency_two() {
    // I4 + I5: with N=2, the third job stays queued; max active == 2.
    let (runtime, scratch) = local_runtime(2);
    for name in ["a", "b", "c"] {
        let src = scratch.join(format!("s-{name}"));
        let dst = scratch.join(format!("d-{name}"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("x"), name).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        runtime
            .enqueue(local_copy_plan(&src, &dst), vec!["x".into()])
            .expect("enqueue");
    }
    // Synchronous scheduler observation: enqueue pumps synchronously.
    let summary = runtime.summary();
    assert_eq!(summary.running, 2, "at most N=2 active");
    assert_eq!(summary.waiting, 1, "third job queued");
}

#[tokio::test(flavor = "current_thread")]
async fn product_path_queued_cancel_consumes_zero_executions() {
    // I6: cancel the queued (third) job through the real cancel route; it must
    // never reach the executor.
    let (runtime, scratch) = local_runtime(2);
    let mut ids = Vec::new();
    for name in ["a", "b", "c"] {
        let src = scratch.join(format!("s-{name}"));
        let dst = scratch.join(format!("d-{name}"));
        // 'c' is intentionally given a non-existent source so that, were it to
        // run, it would FAIL rather than silently "not run". We cancel it
        // first, so it must never execute at all.
        if name != "c" {
            std::fs::create_dir_all(&src).unwrap();
            std::fs::write(src.join("x"), name).unwrap();
        }
        std::fs::create_dir_all(&dst).unwrap();
        ids.push(
            runtime
                .enqueue(local_copy_plan(&src, &dst), vec!["x".into()])
                .expect("enqueue"),
        );
    }
    let c = ids[2].clone();
    // Cancel C before any spawned task polls (current_thread, no await yet).
    let action = runtime.cancel(&c).expect("cancel route");
    assert_eq!(action, CancelAction::TerminalizeWithoutExecution);

    // C is terminalized without ever entering Running.
    let c_status = runtime
        .manager()
        .snapshot()
        .into_iter()
        .find(|j| j.id == c)
        .map(|j| j.status);
    assert_eq!(c_status, Some(JobStatus::Cancelled));
}

#[tokio::test(flavor = "current_thread")]
async fn product_path_active_cancel_keeps_slot_until_executor_settles() {
    // I7: cancelling an active job returns SignalActiveExecution and the job
    // transitions to Cancelling (slot held; terminal truth comes from executor).
    let (runtime, scratch) = local_runtime(1);
    let src = scratch.join("src");
    let dst = scratch.join("dst");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("x"), b"y").unwrap();
    std::fs::create_dir_all(&dst).unwrap();
    let id = runtime
        .enqueue(local_copy_plan(&src, &dst), vec!["x".into()])
        .expect("enqueue");

    // Cancel immediately (no await): the spawned run_one task has not polled
    // yet, so the scheduler still holds the job Active -> SignalActiveExecution
    // and the slot stays held until the executor reports terminal truth.
    let action = runtime.cancel(&id).expect("cancel active");
    assert_eq!(action, CancelAction::SignalActiveExecution);
    let status = runtime
        .manager()
        .snapshot()
        .into_iter()
        .find(|j| j.id == id)
        .map(|j| j.status);
    assert_eq!(status, Some(JobStatus::Cancelling));
}

#[tokio::test(flavor = "current_thread")]
async fn product_path_unrelated_jobkind_uses_legacy_cancel() {
    // I9: a non-Transfer job cannot be cancelled via the queue runtime (it does
    // not own it), but the legacy JobManager cancel still works for it.
    let (runtime, _scratch) = local_runtime(DEFAULT_TRANSFER_CONCURRENCY);
    let legacy = runtime
        .manager()
        .create_job("other", JobKind::Copy, "legacy", None, None);
    let id = legacy.id.clone();

    // Queue runtime does not own this job -> UnknownJob error.
    assert!(runtime.cancel(&id).is_err());

    // Legacy cancel still works for unrelated job kinds.
    assert!(runtime.manager().cancel(&id));
}

#[tokio::test(flavor = "current_thread")]
async fn product_path_terminal_job_does_not_resurrect() {
    // I10: a Completed job, when cancelled again, stays terminal (no re-run).
    let (runtime, scratch) = local_runtime(DEFAULT_TRANSFER_CONCURRENCY);
    let src = scratch.join("src");
    let dst = scratch.join("dst");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("x"), b"y").unwrap();
    std::fs::create_dir_all(&dst).unwrap();
    let id = runtime
        .enqueue(local_copy_plan(&src, &dst), vec!["x".into()])
        .expect("enqueue");

    // Wait for Completion.
    let mut waited = 0;
    while runtime.manager().snapshot()[0].status != JobStatus::Completed && waited < 100 {
        tokio::time::sleep(Duration::from_millis(10)).await;
        waited += 1;
    }
    assert_eq!(runtime.manager().snapshot()[0].status, JobStatus::Completed);

    // Cancel after terminal -> AlreadyFinished, no resurrection.
    let action = runtime.cancel(&id).expect("cancel terminal");
    assert_eq!(action, CancelAction::AlreadyFinished);
    assert_eq!(runtime.manager().snapshot()[0].status, JobStatus::Completed);
}
