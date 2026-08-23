use arx::app::AppState;
use arx::jobs::{JobEvent, JobManager, JobResult};

pub(super) struct JobResponseOutcome {
    pub refresh_panes: bool,
    pub failure_notification: Option<String>,
}

/// Apply an already-accepted JobManager event. Lifecycle state lives in JobManager.
pub(super) fn apply_job_event(
    event: &JobEvent,
    state: &mut AppState,
    job_manager: &JobManager,
) -> JobResponseOutcome {
    state.jobs = job_manager.snapshot();
    let id = job_event_id(event);
    if let Some(job) = job_manager.get(id) {
        state.remote_workspace.sync_from_job(&job);
    }

    JobResponseOutcome {
        refresh_panes: handle_job_event(event, state),
        failure_notification: match event {
            JobEvent::Failed { id, error, .. } => Some(format!("Job {id} failed: {error}")),
            _ => None,
        },
    }
}

fn job_event_id(event: &JobEvent) -> &str {
    match event {
        JobEvent::Running { id }
        | JobEvent::PausePending { id }
        | JobEvent::Paused { id }
        | JobEvent::RetryWaiting { id }
        | JobEvent::Progress { id, .. }
        | JobEvent::Completed { id, .. }
        | JobEvent::Failed { id, .. }
        | JobEvent::Cancelled { id, .. } => id,
    }
}

fn handle_job_event(event: &JobEvent, state: &mut AppState) -> bool {
    match event {
        JobEvent::Completed { id, result } => match result {
            JobResult::Generic { message, .. } => {
                state.message = Some(
                    message
                        .clone()
                        .unwrap_or_else(|| format!("Job {id} completed")),
                );
                true
            }
            JobResult::WorkspaceSync(outcome) => {
                state.message = Some(format!(
                    "Sync completed: {} physical step(s), {} bytes",
                    outcome.completed.len(),
                    outcome.transferred_bytes
                ));
                true
            }
            JobResult::RemoteEdit(_) => {
                state.message = Some(format!("Remote edit job {id} completed"));
                true
            }
            #[cfg(target_os = "linux")]
            JobResult::StorageScan(summary) => {
                state.message = Some(storage_scan_message(summary));
                false
            }
        },
        JobEvent::Failed { error, .. } => {
            state.message = Some(error.clone());
            true
        }
        JobEvent::Cancelled { id, result } => match result {
            JobResult::Generic { message, .. } => {
                state.message = Some(
                    message
                        .clone()
                        .unwrap_or_else(|| format!("Job {id} cancelled")),
                );
                true
            }
            JobResult::WorkspaceSync(outcome) => {
                state.message = Some(format!(
                    "Sync cancelled after {} completed physical step(s)",
                    outcome.completed.len()
                ));
                true
            }
            JobResult::RemoteEdit(_) => {
                state.message = Some(format!("Remote edit job {id} cancelled"));
                true
            }
            #[cfg(target_os = "linux")]
            JobResult::StorageScan(summary) => {
                state.message = Some(storage_scan_message(summary));
                false
            }
        },
        JobEvent::Running { .. }
        | JobEvent::PausePending { .. }
        | JobEvent::Progress { .. }
        | JobEvent::Paused { .. }
        | JobEvent::RetryWaiting { .. } => false,
    }
}

#[cfg(target_os = "linux")]
fn storage_scan_message(summary: &arx::storage_inspector_job::StorageScanSummary) -> String {
    match summary.outcome {
        arx::storage_inspector::UsageScanOutcome::Complete => "Storage scan completed".to_string(),
        arx::storage_inspector::UsageScanOutcome::Partial => {
            format!("Storage scan partial: {} error(s)", summary.totals.errors)
        }
        arx::storage_inspector::UsageScanOutcome::Cancelled => "Storage scan cancelled".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arx::jobs::{JobKind, JobStatus};
    use arx::vfs::EntryKind;
    use arx::workspace_sync::{
        SyncPolicy, WorkspaceDiff, WorkspaceEntry, WorkspaceFingerprint, WorkspaceSyncPlan,
    };
    use arx::workspace_sync_execution::SyncPlanValidator;
    use arx::workspace_sync_executor::{
        SyncExecutionOutcome, SyncJournalFinalization, SyncTerminalState,
    };

    fn manager_with_job(kind: JobKind) -> (JobManager, String) {
        let manager = JobManager::new();
        let job = manager.create_job("response", kind, "response test", None, None);
        (manager, job.id)
    }

    fn generic(message: Option<&str>) -> JobResult {
        JobResult::Generic {
            message: message.map(str::to_string),
            completed_items: None,
        }
    }

    fn sync_outcome(terminal: SyncTerminalState) -> SyncExecutionOutcome {
        let diff = WorkspaceDiff::compare(
            arx::vfs::Location::Local("/left".into()),
            arx::vfs::Location::Local("/right".into()),
            vec![WorkspaceEntry {
                relative_path: "a.txt".into(),
                fingerprint: WorkspaceFingerprint {
                    kind: EntryKind::File,
                    size: Some(1),
                    modified_unix_ms: None,
                    content_hash: None,
                },
            }],
            Vec::new(),
        );
        let plan = WorkspaceSyncPlan::build(&diff, SyncPolicy::default());
        let plan_id = SyncPlanValidator::freeze(&plan, &diff, &arx::vfs::default_registry())
            .expect("freeze test plan")
            .id();
        SyncExecutionOutcome {
            plan_id,
            completed: Vec::new(),
            terminal,
            remaining: Vec::new(),
            transferred_bytes: 0,
            workspace_may_have_changed: true,
            journal: SyncJournalFinalization::Recorded,
        }
    }

    #[test]
    fn job_response_running_uses_existing_manager_snapshot_without_refresh() {
        let (manager, id) = manager_with_job(JobKind::Copy);
        let event = JobEvent::Running { id: id.clone() };
        assert!(manager.apply_event(&event));
        let mut state = AppState::default();

        let outcome = apply_job_event(&event, &mut state, &manager);

        let stored = manager.get(&id).expect("existing manager job");
        assert_eq!(state.jobs.len(), 1);
        assert_eq!(state.jobs[0].id, stored.id);
        assert_eq!(state.jobs[0].status, stored.status);
        assert_eq!(state.jobs[0].status, JobStatus::Running);
        assert!(!outcome.refresh_panes);
        assert!(outcome.failure_notification.is_none());
    }

    #[test]
    fn job_response_completed_generic_refreshes() {
        let (manager, id) = manager_with_job(JobKind::Copy);
        let event = JobEvent::Completed {
            id: id.clone(),
            result: generic(None),
        };
        assert!(manager.apply_event(&event));
        let mut state = AppState::default();

        let outcome = apply_job_event(&event, &mut state, &manager);

        assert!(outcome.refresh_panes);
        assert_eq!(
            state.message.as_deref(),
            Some(format!("Job {id} completed").as_str())
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn job_response_storage_scan_completion_does_not_refresh() {
        use arx::storage_inspector::{UsageScanOutcome, UsageTotals};
        use arx::storage_inspector_job::StorageScanSummary;

        let (manager, id) = manager_with_job(JobKind::StorageScan);
        let event = JobEvent::Completed {
            id,
            result: JobResult::StorageScan(StorageScanSummary {
                root: "/root".into(),
                outcome: UsageScanOutcome::Complete,
                totals: UsageTotals::default(),
            }),
        };
        assert!(manager.apply_event(&event));
        let mut state = AppState::default();

        let outcome = apply_job_event(&event, &mut state, &manager);

        assert!(!outcome.refresh_panes);
        assert_eq!(state.message.as_deref(), Some("Storage scan completed"));
    }

    #[test]
    fn job_response_failed_returns_exact_notification_and_refreshes() {
        let (manager, id) = manager_with_job(JobKind::Copy);
        let event = JobEvent::Failed {
            id: id.clone(),
            error: "disk full".into(),
            result: None,
        };
        assert!(manager.apply_event(&event));
        let mut state = AppState::default();

        let outcome = apply_job_event(&event, &mut state, &manager);

        assert!(outcome.refresh_panes);
        assert_eq!(state.message.as_deref(), Some("disk full"));
        assert_eq!(
            outcome.failure_notification.as_deref(),
            Some(format!("Job {id} failed: disk full").as_str())
        );
    }

    #[test]
    fn job_response_cancelled_workspace_sync_preserves_message_and_refresh() {
        let (manager, id) = manager_with_job(JobKind::Synchronize);
        let event = JobEvent::Cancelled {
            id,
            result: JobResult::WorkspaceSync(sync_outcome(SyncTerminalState::Cancelled {
                completed_steps: 0,
            })),
        };
        assert!(manager.apply_event(&event));
        let mut state = AppState::default();

        let outcome = apply_job_event(&event, &mut state, &manager);

        assert!(outcome.refresh_panes);
        assert_eq!(
            state.message.as_deref(),
            Some("Sync cancelled after 0 completed physical step(s)")
        );
    }

    #[tokio::test]
    async fn job_response_syncs_matching_workspace_from_existing_manager() {
        use arx::journal::OperationJournal;
        use arx::services::{WorkspaceScanOptions, WorkspaceSyncController, scan_workspace};
        use arx::vfs::{Location, default_registry};
        use std::sync::atomic::AtomicBool;
        use tokio::sync::mpsc;

        let left = tempfile::tempdir().unwrap();
        let right = tempfile::tempdir().unwrap();
        tokio::fs::write(left.path().join("a.txt"), b"a")
            .await
            .unwrap();
        let registry = default_registry();
        let left_root = Location::Local(left.path().to_path_buf());
        let right_root = Location::Local(right.path().to_path_buf());
        let diff = WorkspaceDiff::compare(
            left_root.clone(),
            right_root,
            scan_workspace(
                &registry,
                &left_root,
                WorkspaceScanOptions::default(),
                &AtomicBool::new(false),
            )
            .await
            .unwrap(),
            Vec::new(),
        );
        let plan = WorkspaceSyncPlan::build(&diff, SyncPolicy::default());
        let journal_dir = tempfile::tempdir().unwrap();
        let controller = WorkspaceSyncController::with_journal(
            registry,
            OperationJournal::open(journal_dir.path().join("ops.jsonl")).unwrap(),
        );
        let frozen = controller.freeze(&plan, &diff).unwrap();
        let manager = JobManager::new();
        let (job_tx, mut job_rx) = mpsc::unbounded_channel();
        let (verification_tx, _verification_rx) = mpsc::unbounded_channel();
        let id = controller
            .launch(
                frozen,
                diff.clone(),
                false,
                manager.clone(),
                job_tx,
                verification_tx,
            )
            .await
            .unwrap();
        let event = job_rx.recv().await.expect("published job event");
        let mut state = AppState::default();
        state.remote_workspace.diff = Some(diff);

        apply_job_event(&event, &mut state, &manager);

        assert_eq!(state.remote_workspace.ux.job_id(), Some(id.as_str()));
        assert_eq!(state.jobs.len(), manager.snapshot().len());
        assert_eq!(state.jobs[0].id, manager.snapshot()[0].id);
    }
}
