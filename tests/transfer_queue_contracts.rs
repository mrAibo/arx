use std::time::{Duration, Instant};

use arx::transfer_queue::{
    CancelAction, DEFAULT_TRANSFER_CONCURRENCY, MAX_TOTAL_TRANSFER_ATTEMPTS, PauseAction,
    QueueError, RetryDecision, RetryDisposition, SchedulerState, TransferProgressTracker,
    TransferQueueConfig, TransferQueueCore, TransferRateEstimator, TypedTransferProgress,
};

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

        let decision = queue
            .failure("job", RetryDisposition::SafeToRetry)
            .unwrap();
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
