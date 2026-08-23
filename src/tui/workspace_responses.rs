use super::{AppState, SessionCallout, SyncLaunchResponse};
use arx::jobs::{Job, JobManager, JobResult, JobStatus};
use arx::services::{WorkspaceScanError, WorkspaceScanResponse, WorkspaceSyncController};
use arx::workspace_sync::WorkspaceSide;
use arx::workspace_sync_executor::SyncTerminalState;
use arx::workspace_sync_verification::{
    SyncVerificationEvent, SyncVerificationStatus, SyncVerificationVerdict,
};

pub(super) fn handle_workspace_scan_response(
    response: WorkspaceScanResponse,
    state: &mut AppState,
) {
    if !state.remote_workspace.accepts_scan(&response) {
        return;
    }

    let current_root = match response.side {
        WorkspaceSide::Left => &state.left.location,
        WorkspaceSide::Right => &state.right.location,
    };
    if current_root != &response.root {
        state
            .remote_workspace
            .finish_scan(response.side, response.id);
        return;
    }

    let side = response.side;
    let id = response.id;
    match response.result {
        Ok(entries) => match side {
            WorkspaceSide::Left => state.remote_workspace.left_entries = Some(entries),
            WorkspaceSide::Right => state.remote_workspace.right_entries = Some(entries),
        },
        Err(WorkspaceScanError::Cancelled) => {
            state.remote_workspace.finish_scan(side, id);
            return;
        }
        Err(error) => {
            state.remote_workspace.finish_scan(side, id);
            state.message = Some(format!("Workspace scan failed: {error}"));
            return;
        }
    }
    state.remote_workspace.finish_scan(side, id);

    if state
        .remote_workspace
        .try_build_recursive_diff(state.left.location.clone(), state.right.location.clone())
    {
        observe_compare_success(state);
        state.message = Some(state.remote_workspace.summary());
    } else {
        let waiting = match side {
            WorkspaceSide::Left => "right",
            WorkspaceSide::Right => "left",
        };
        state.message = Some(format!("Remote Workspace: waiting for {waiting} pane…"));
    }
}

pub(super) fn apply_sync_launch_response(
    response: SyncLaunchResponse,
    state: &mut AppState,
    controller: &WorkspaceSyncController,
    job_manager: &JobManager,
) {
    let still_current = controller.is_launch_current(response.launch_id)
        && state
            .remote_workspace
            .frozen_plan
            .as_ref()
            .is_some_and(|frozen| frozen.id() == response.plan_id);
    if still_current {
        match response.result {
            Ok(job_id) => {
                state.jobs = job_manager.snapshot();
                if let Some(job) = job_manager.get(&job_id) {
                    state.remote_workspace.sync_from_job(&job);
                }
            }
            Err(message) => state.remote_workspace.mark_blocked(message),
        }
    }
}

pub(super) fn apply_verification_event(
    event: SyncVerificationEvent,
    state: &mut AppState,
    job_manager: &JobManager,
) {
    let left_root = state.left.location.clone();
    let right_root = state.right.location.clone();
    let accepted =
        state
            .remote_workspace
            .apply_verification(&event.verification, &left_root, &right_root);
    // JobManager accepted the verification before publishing this event, so its
    // render snapshot is useful even when pane roots moved and the old diff is rejected.
    state.jobs = job_manager.snapshot();
    if let Some(job) = job_manager.get(&event.job_id) {
        observe_verified_sync_success(state, &job);
    }
    if accepted {
        state
            .remote_workspace
            .sync_verification_stage(&event.job_id);
    } else {
        state
            .remote_workspace
            .settle_rejected_verification(&event.job_id, &event.verification);
    }
}

fn observe_compare_success(state: &mut AppState) {
    let Some(diff) = state.remote_workspace.diff.as_ref() else {
        return;
    };
    let differences = diff.changed_count();
    let bytes_to_transfer = state
        .remote_workspace
        .plan
        .as_ref()
        .map(|plan| plan.bytes_to_transfer)
        .unwrap_or(0);
    if state.milestones.take_compare_success() {
        state.session_callout = Some(SessionCallout::CompareCompleted {
            differences,
            bytes_to_transfer,
        });
    }
}

fn is_verified_sync_success(job: &Job) -> bool {
    if job.status != JobStatus::Completed {
        return false;
    }
    let execution_completed = matches!(
        &job.result,
        Some(JobResult::WorkspaceSync(outcome))
            if matches!(&outcome.terminal, SyncTerminalState::Completed)
    );
    if !execution_completed {
        return false;
    }

    job.verification.as_ref().is_some_and(|verification| {
        matches!(
            &verification.status,
            SyncVerificationStatus::Finished(result)
                if result.verdict == SyncVerificationVerdict::Synchronized
        )
    })
}

pub(super) fn observe_verified_sync_success(state: &mut AppState, job: &Job) {
    if !is_verified_sync_success(job) {
        return;
    }
    if state.milestones.take_verified_sync_success() {
        state.session_callout = Some(SessionCallout::WorkspaceSyncVerified {
            job_id: job.id.clone(),
        });
    }
}
