use super::*;

pub(super) fn handle_action(
    state: &mut AppState,
    action: &Action,
    scanner: &WorkspaceScanner,
    sync: &SyncUiRuntime,
) -> bool {
    if !matches!(
        action,
        Action::ToggleWorkspaceComparison
            | Action::PreviewWorkspaceSync
            | Action::ReverseWorkspaceDirection
            | Action::ToggleWorkspaceSyncMode
            | Action::ExecuteWorkspaceSync
            | Action::ConfirmWorkspaceSync
            | Action::CancelWorkspaceSync
            | Action::ShowWorkspaceSyncDetails
            | Action::ShowWorkspaceVerificationDiff
            | Action::ReturnToWorkspaceSyncPreview
            | Action::CloseWorkspaceSyncOverlay
    ) {
        return false;
    }

    if matches!(
        action,
        Action::ToggleWorkspaceComparison
            | Action::PreviewWorkspaceSync
            | Action::ReverseWorkspaceDirection
            | Action::ToggleWorkspaceSyncMode
    ) && !supersede_workspace_launch_for_new_action(state, sync)
    {
        return true;
    }

    match action {
        Action::ToggleWorkspaceComparison
        | Action::PreviewWorkspaceSync
        | Action::ReverseWorkspaceDirection
        | Action::ToggleWorkspaceSyncMode
            if state.remote_workspace.ux.is_locked_flow() =>
        {
            state.open_overlay(OverlayKind::SyncPreview);
            state.message = Some(
                "Workspace sync is already preparing or active; the current immutable plan is locked."
                    .into(),
            );
        }
        Action::ToggleWorkspaceComparison => {
            if state.remote_workspace.enabled {
                state.remote_workspace.disable();
                state.show_diff = false;
                state.message = Some("Remote Workspace comparison off".into());
            } else {
                start_workspace_scan(scanner, state, false);
            }
        }
        Action::PreviewWorkspaceSync => {
            start_workspace_scan(scanner, state, true);
            state.open_overlay(OverlayKind::SyncPreview);
        }
        Action::ReverseWorkspaceDirection => state.remote_workspace.reverse_direction(),
        Action::ToggleWorkspaceSyncMode => state.remote_workspace.toggle_mode(),
        Action::ExecuteWorkspaceSync => prepare_workspace_sync(state, sync),
        Action::ConfirmWorkspaceSync => launch_workspace_sync(state, sync, true),
        Action::CancelWorkspaceSync => cancel_workspace_sync(state, sync),
        Action::ShowWorkspaceSyncDetails => {
            if state.remote_workspace.ux.is_job_flow() {
                state.open_overlay(OverlayKind::SyncPreview);
            }
        }
        Action::ShowWorkspaceVerificationDiff => {
            if let Some(job_id) = state.remote_workspace.ux.job_id().map(str::to_string) {
                state.remote_workspace.show_verification_diff(job_id);
                state.open_overlay(OverlayKind::SyncPreview);
                // The overlay renders the recursive Job-bound verification
                // snapshot. Do not reuse shallow pane-level diff highlighting.
                state.show_diff = false;
            }
        }
        Action::ReturnToWorkspaceSyncPreview => {
            if state.remote_workspace.return_from_verification_diff() {
                state.open_overlay(OverlayKind::SyncPreview);
            } else if matches!(
                state.remote_workspace.ux,
                WorkspaceSyncUxState::ConfirmationRequired { .. }
                    | WorkspaceSyncUxState::Blocked { .. }
                    | WorkspaceSyncUxState::Finished { .. }
            ) {
                state.remote_workspace.mark_preview();
            } else if state.remote_workspace.ux.is_job_flow() {
                state.message =
                    Some("The active sync remains in its Job view until it is finished.".into());
            }
        }
        Action::CloseWorkspaceSyncOverlay => state.close_overlay(OverlayKind::SyncPreview),
        _ => unreachable!("workspace action set checked above"),
    }
    true
}

fn supersede_workspace_launch_for_new_action(state: &mut AppState, sync: &SyncUiRuntime) -> bool {
    if !matches!(
        state.remote_workspace.ux,
        WorkspaceSyncUxState::Launching { .. }
    ) {
        return true;
    }

    if sync.controller.supersede_launch() {
        state.remote_workspace.supersede_launch_presentation();
        true
    } else {
        state.open_overlay(OverlayKind::SyncPreview);
        state.message = Some(
            "Workspace sync has already crossed the Job queue boundary; waiting for its Job view."
                .into(),
        );
        false
    }
}

fn prepare_workspace_sync(state: &mut AppState, sync: &SyncUiRuntime) {
    let (Some(plan), Some(diff)) = (
        state.remote_workspace.plan.as_ref(),
        state.remote_workspace.diff.as_ref(),
    ) else {
        state.remote_workspace.mark_blocked(
            "Plan cannot be executed\nThe workspace preview is not ready.\nNo files were changed.",
        );
        return;
    };
    match sync.controller.freeze(plan, diff) {
        Ok(frozen) => {
            let requires_confirmation = frozen.requires_confirmation();
            state.remote_workspace.set_frozen_plan(frozen);
            if !requires_confirmation {
                launch_workspace_sync(state, sync, false);
            }
        }
        Err(error) => state.remote_workspace.mark_blocked(format!(
            "Plan cannot be executed\n{error}\nNo files were changed."
        )),
    }
}

fn launch_workspace_sync(state: &mut AppState, sync: &SyncUiRuntime, confirmed: bool) {
    let (Some(frozen), Some(diff)) = (
        state.remote_workspace.frozen_plan.clone(),
        state.remote_workspace.diff.clone(),
    ) else {
        state.remote_workspace.mark_blocked(
            "Plan cannot be executed\nThe frozen preview is no longer current.\nNo files were changed.",
        );
        return;
    };
    let plan_id = frozen.id();
    let launch_id = sync.controller.begin_launch();
    state.remote_workspace.mark_launching();
    let controller = sync.controller.clone();
    let jobs = sync.jobs.clone();
    let job_events = sync.job_events.clone();
    let verification_events = sync.verification_events.clone();
    let launch_events = sync.launch_events.clone();
    tokio::spawn(async move {
        let result = controller
            .launch_guarded(
                launch_id,
                frozen,
                diff,
                confirmed,
                jobs,
                (job_events, verification_events),
            )
            .await
            .map_err(|error| error.user_message());
        let _ = launch_events.send(SyncLaunchResponse {
            launch_id,
            plan_id,
            result,
        });
    });
}

fn cancel_workspace_sync(state: &mut AppState, sync: &SyncUiRuntime) {
    let Some(job_id) = state.remote_workspace.ux.job_id().map(str::to_string) else {
        return;
    };
    if sync.jobs.cancel(&job_id) {
        state.jobs = sync.jobs.snapshot();
        if let Some(job) = sync.jobs.get(&job_id) {
            state.remote_workspace.sync_from_job(&job);
        }
    }
}

fn start_workspace_scan(scanner: &WorkspaceScanner, state: &mut AppState, keep_preview_open: bool) {
    let left_root = state.left.location.clone();
    let right_root = state.right.location.clone();
    let cancel = state.remote_workspace.begin_recursive_scan();
    state.remote_workspace.enabled = true;
    state.remote_workspace.preview_open = keep_preview_open;
    state.show_diff = true;

    let options = WorkspaceScanOptions::default();
    let left_id = scanner.scan(WorkspaceSide::Left, left_root, options, cancel.clone());
    let right_id = scanner.scan(WorkspaceSide::Right, right_root, options, cancel);
    state
        .remote_workspace
        .register_scan(WorkspaceSide::Left, left_id);
    state
        .remote_workspace
        .register_scan(WorkspaceSide::Right, right_id);
    state.message = Some("Remote Workspace: scanning both panes…".into());
}

fn sync_heading(label: &'static str, color: Color) -> Line<'static> {
    Line::from(Span::styled(label, Style::default().fg(color)))
}

pub(super) fn render(frame: &mut ratatui::Frame, area: Rect, state: &AppState) {
    let popup = centered_rect(86, 82, area);
    frame.render_widget(Clear, popup);

    let mut lines = Vec::new();
    let mut title = " Workspace Sync ".to_string();
    let mut border = Style::default().fg(Color::Cyan);

    match &state.remote_workspace.ux {
        WorkspaceSyncUxState::Idle | WorkspaceSyncUxState::Scanning => {
            title = " Workspace Sync · SCANNING ".into();
            lines.push(sync_heading("SCAN", Color::Cyan));
            lines.push(Line::from("Scanning both workspace roots…"));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "No files will be changed while ARX builds the preview.",
                Style::default().fg(Color::DarkGray),
            )));
        }
        WorkspaceSyncUxState::Preview { .. } => {
            render_sync_plan_lines(state, &mut lines);
            if let Some(plan) = state.remote_workspace.plan.as_ref() {
                title = if plan.can_execute() {
                    " Workspace Sync · PREVIEW READY ".into()
                } else {
                    border = Style::default().fg(Color::Yellow);
                    " Workspace Sync · PREVIEW BLOCKED ".into()
                };
            }
        }
        WorkspaceSyncUxState::ConfirmationRequired {
            digest,
            destructive_operations,
            ..
        } => {
            title = " Workspace Sync · CONFIRM MIRROR ".into();
            border = Style::default().fg(Color::Yellow);
            lines.push(sync_heading("DESTRUCTIVE CONFIRMATION", Color::Red));
            lines.push(Line::from(format!(
                "This frozen plan contains {destructive_operations} destructive operation(s)."
            )));
            lines.push(Line::from(
                "Destination-only entries in this exact plan may be removed.",
            ));
            lines.push(Line::from(""));
            render_sync_plan_lines(state, &mut lines);
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("Frozen preview digest  {}…", &digest.as_hex()[..8]),
                Style::default().fg(Color::DarkGray),
            )));
            lines.push(Line::from(Span::styled(
                "Confirmation applies only to this exact frozen plan.",
                Style::default().fg(Color::DarkGray),
            )));
        }
        WorkspaceSyncUxState::Launching { .. } => {
            title = " Workspace Sync · PREPARING ".into();
            lines.push(sync_heading("PREPARING EXECUTION", Color::Cyan));
            lines.push(Line::from("Freezing transport choice and execution steps…"));
            lines.push(Line::from("No Job has been created yet."));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "A newer compare, direction, or mode action supersedes preparation before a Job is queued.",
                Style::default().fg(Color::DarkGray),
            )));
        }
        WorkspaceSyncUxState::Blocked { message } => {
            title = " Workspace Sync · CANNOT EXECUTE ".into();
            border = Style::default().fg(Color::Yellow);
            lines.push(sync_heading("BLOCKED", Color::Yellow));
            lines.extend(message.lines().map(|line| Line::from(line.to_string())));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Adjust the current preview before trying again.",
                Style::default().fg(Color::DarkGray),
            )));
        }
        WorkspaceSyncUxState::VerificationDiff { job_id } => {
            title = " Workspace Sync · VERIFICATION DIFF ".into();
            if let Some(job) = state.jobs.iter().find(|job| job.id == *job_id) {
                render_sync_verification_diff_lines(job, &mut lines);
            } else {
                lines.push(Line::from("The verification Job is no longer available."));
            }
        }
        WorkspaceSyncUxState::Queued { job_id }
        | WorkspaceSyncUxState::Running { job_id }
        | WorkspaceSyncUxState::Cancelling { job_id }
        | WorkspaceSyncUxState::Verifying { job_id }
        | WorkspaceSyncUxState::Finished { job_id } => {
            if let Some(job) = state.jobs.iter().find(|job| job.id == *job_id) {
                title = sync_job_title(job, &state.remote_workspace.ux);
                let show_first_success = matches!(
                    state.session_callout.as_ref(),
                    Some(SessionCallout::WorkspaceSyncVerified { job_id }) if job_id == &job.id
                );
                render_sync_job_lines(
                    job,
                    &state.remote_workspace.ux,
                    show_first_success,
                    &mut lines,
                );
            } else {
                lines.push(Line::from("Waiting for JobManager snapshot…"));
            }
        }
    }

    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title)
                    .border_style(border),
            )
            .wrap(Wrap { trim: false }),
        popup,
    );
}
fn render_sync_plan_lines(state: &AppState, lines: &mut Vec<Line<'static>>) {
    let Some(plan) = state.remote_workspace.plan.as_ref() else {
        lines.push(Line::from("Preview is not ready yet."));
        return;
    };
    let (source, destination) =
        sync_display_roots(&plan.left_root, &plan.right_root, plan.policy.direction);
    let copies = plan
        .operations
        .iter()
        .filter(|operation| matches!(operation, WorkspaceSyncOperation::Copy { .. }))
        .count();
    let deletes = plan
        .operations
        .iter()
        .filter(|operation| matches!(operation, WorkspaceSyncOperation::Delete { .. }))
        .count();
    let create_dirs = state
        .remote_workspace
        .diff
        .as_ref()
        .map(|diff| {
            plan.operations
                .iter()
                .filter(|operation| match operation {
                    WorkspaceSyncOperation::Copy {
                        relative_path,
                        from,
                        ..
                    } => diff
                        .entries
                        .iter()
                        .find(|entry| entry.relative_path == *relative_path)
                        .and_then(|entry| match from {
                            WorkspaceSide::Left => entry.left.as_ref(),
                            WorkspaceSide::Right => entry.right.as_ref(),
                        })
                        .is_some_and(|fingerprint| fingerprint.kind == EntryKind::Directory),
                    _ => false,
                })
                .count()
        })
        .unwrap_or(0);

    lines.push(sync_heading("ROUTE", Color::Cyan));
    lines.push(Line::from(Span::styled(
        format!("{source}  →  {destination}"),
        Style::default().fg(Color::Cyan),
    )));
    lines.push(Line::from(Span::styled(
        format!(
            "{} · {}",
            state.remote_workspace.direction_label(),
            state.remote_workspace.mode_label()
        ),
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::from(""));

    lines.push(sync_heading("PLAN", Color::Cyan));
    lines.push(Line::from(format!("Copy / update     {copies}")));
    lines.push(Line::from(format!("Create dirs       {create_dirs}")));
    lines.push(Line::from(format!("Delete            {deletes}")));
    lines.push(Line::from(format!("Conflicts         {}", plan.conflicts)));
    lines.push(Line::from(format!(
        "Transfer          {}",
        format_size(plan.bytes_to_transfer)
    )));
    lines.push(Line::from(""));

    lines.push(sync_heading(
        "SAFETY",
        if plan.destructive_operations == 0 {
            Color::Green
        } else {
            Color::Yellow
        },
    ));
    if plan.destructive_operations == 0 && plan.policy.mode == SyncMode::Update {
        lines.push(Line::from(Span::styled(
            "Safe update — destination-only entries are preserved.",
            Style::default().fg(Color::Green),
        )));
    } else if plan.destructive_operations == 0 {
        lines.push(Line::from(Span::styled(
            "This plan is non-destructive.",
            Style::default().fg(Color::Green),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            format!(
                "This plan contains {} destructive operation(s).",
                plan.destructive_operations
            ),
            Style::default().fg(Color::Yellow),
        )));
    }
}
fn sync_display_roots<'a>(
    left: &'a Location,
    right: &'a Location,
    direction: SyncDirection,
) -> (&'a Location, &'a Location) {
    match direction {
        SyncDirection::LeftToRight => (left, right),
        SyncDirection::RightToLeft => (right, left),
    }
}

fn sync_job_title(job: &arx::jobs::Job, ux: &WorkspaceSyncUxState) -> String {
    let destination = job
        .display_destination()
        .map(ToString::to_string)
        .unwrap_or_else(|| "destination".into());
    match ux {
        WorkspaceSyncUxState::Queued { .. } => {
            format!(" Workspace Sync · QUEUED → {destination} ")
        }
        WorkspaceSyncUxState::Running { .. } => {
            format!(" Workspace Sync · RUNNING → {destination} ")
        }
        WorkspaceSyncUxState::Cancelling { .. } => " Workspace Sync · CANCELLING ".into(),
        WorkspaceSyncUxState::Verifying { .. } => " Workspace Sync · VERIFYING ".into(),
        WorkspaceSyncUxState::Finished { .. } => " Workspace Sync · RESULT ".into(),
        _ => " Workspace Sync ".into(),
    }
}
fn render_sync_job_lines(
    job: &arx::jobs::Job,
    ux: &WorkspaceSyncUxState,
    show_first_success: bool,
    lines: &mut Vec<Line<'static>>,
) {
    if let (Some(source), Some(destination)) = (job.display_source(), job.display_destination()) {
        lines.push(sync_heading("ROUTE", Color::Cyan));
        lines.push(Line::from(Span::styled(
            format!("{source}  →  {destination}"),
            Style::default().fg(Color::Cyan),
        )));
        lines.push(Line::from(""));
    }

    if let arx::jobs::JobProgress::WorkspaceSync(progress) = &job.progress {
        let percent = progress.percent().unwrap_or(0);
        let filled = usize::from(percent) / 5;
        lines.push(sync_heading("PROGRESS", Color::Cyan));
        lines.push(Line::from(Span::styled(
            format!(
                "[{}{}] {percent}%",
                "█".repeat(filled),
                "░".repeat(20usize.saturating_sub(filled))
            ),
            Style::default().fg(Color::Cyan),
        )));
        lines.push(Line::from(format!(
            "{} / {} physical steps",
            progress.completed_steps, progress.total_steps
        )));
        lines.push(Line::from(format!(
            "{} / {} transferred",
            format_size(progress.transferred_bytes),
            format_size(progress.total_bytes)
        )));
        if let Some(path) = &progress.current_path {
            lines.push(Line::from(format!("Current  → {path}")));
        }
        lines.push(Line::from(""));
    }

    match &job.result {
        Some(arx::jobs::JobResult::WorkspaceSync(outcome)) => {
            lines.push(sync_heading("EXECUTION", Color::Cyan));
            match &outcome.terminal {
                arx::workspace_sync_executor::SyncTerminalState::Completed => {
                    lines.push(Line::from(Span::styled(
                        "OK Execution completed",
                        Style::default().fg(Color::Green),
                    )));
                }
                arx::workspace_sync_executor::SyncTerminalState::Cancelled { .. } => {
                    lines.push(Line::from(Span::styled(
                        "Sync cancelled",
                        Style::default().fg(Color::Yellow),
                    )));
                    lines.push(Line::from(format!(
                        "OK {} physical step(s) completed",
                        outcome.completed.len()
                    )));
                    lines.push(Line::from(format!(
                        "○ {} physical step(s) not completed",
                        outcome.remaining.len()
                    )));
                    if outcome.workspace_may_have_changed {
                        lines.push(Line::from(Span::styled(
                            "Workspace may have changed.",
                            Style::default().fg(Color::Yellow),
                        )));
                    }
                }
                arx::workspace_sync_executor::SyncTerminalState::Failed { step, error } => {
                    lines.push(Line::from(Span::styled(
                        "X Sync partially completed",
                        Style::default().fg(Color::Red),
                    )));
                    lines.push(Line::from(format!(
                        "OK {} physical step(s) completed",
                        outcome.completed.len()
                    )));
                    lines.push(Line::from(format!("X Step {} failed: {error}", step.0)));
                    lines.push(Line::from(format!(
                        "○ {} physical step(s) not started",
                        outcome.remaining.len()
                    )));
                    lines.push(Line::from("No global rollback was attempted."));
                }
            }
            lines.push(Line::from(format!(
                "{} transferred",
                format_size(outcome.transferred_bytes)
            )));
            if let arx::workspace_sync_executor::SyncJournalFinalization::Failed { error } =
                &outcome.journal
            {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "! Audit record finalization failed",
                    Style::default().fg(Color::Yellow),
                )));
                lines.push(Line::from(error.to_string()));
                lines.push(Line::from("The physical result was preserved."));
            }
        }
        Some(arx::jobs::JobResult::RemoteEdit(_)) => {}
        #[cfg(target_os = "linux")]
        Some(arx::jobs::JobResult::StorageScan(_)) => {}
        Some(arx::jobs::JobResult::Generic { .. }) | None => {}
    }

    if matches!(ux, WorkspaceSyncUxState::Cancelling { .. }) {
        lines.push(Line::from(""));
        lines.push(sync_heading("CANCELLATION", Color::Yellow));
        lines.push(Line::from(Span::styled(
            "Cancelling… waiting for the executor's terminal outcome.",
            Style::default().fg(Color::Yellow),
        )));
    }

    if matches!(ux, WorkspaceSyncUxState::Verifying { .. }) {
        lines.push(Line::from(""));
        lines.push(sync_heading("POST-SYNC VERIFICATION", Color::Cyan));
        lines.push(Line::from("Verifying the current workspace…"));
        lines.push(Line::from(Span::styled(
            "Scanning both workspace roots again after execution.",
            Style::default().fg(Color::DarkGray),
        )));
    }

    if matches!(ux, WorkspaceSyncUxState::Finished { .. }) {
        lines.push(Line::from(""));
        lines.push(sync_heading("POST-SYNC VERIFICATION", Color::Cyan));
        render_verification_lines(job, lines);
        if show_first_success {
            lines.push(Line::from(""));
            lines.push(sync_heading("FIRST SUCCESS THIS SESSION", Color::Green));
            lines.push(Line::from(Span::styled(
                "OK Remote Workspace workflow completed end-to-end.",
                Style::default().fg(Color::Green),
            )));
            lines.push(Line::from(Span::styled(
                "ARX executed the frozen plan and verified the result.",
                Style::default().fg(Color::DarkGray),
            )));
        }
    }
}
fn render_sync_verification_diff_lines(job: &arx::jobs::Job, lines: &mut Vec<Line<'static>>) {
    let Some(verification) = &job.verification else {
        lines.push(Line::from("No post-sync verification result is available."));
        return;
    };
    let SyncVerificationStatus::Finished(result) = &verification.status else {
        lines.push(Line::from("Verification has not finished yet."));
        return;
    };
    if !matches!(
        result.verdict,
        SyncVerificationVerdict::DifferencesRemain { .. }
    ) {
        lines.push(Line::from(
            "Verification did not report remaining differences.",
        ));
        return;
    }

    lines.push(sync_heading("SNAPSHOT", Color::Cyan));
    lines.push(Line::from(format!("LEFT   {}", result.left_root)));
    lines.push(Line::from(format!("RIGHT  {}", result.right_root)));
    lines.push(Line::from(Span::styled(
        "This is the recursive post-sync verification snapshot for this Job.",
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::from(""));
    lines.push(sync_heading("SUMMARY", Color::Yellow));
    lines.push(Line::from(format!(
        "{} proven difference(s) · {} conflict(s) · {} unverified",
        result.changed_entries, result.conflicts, result.unverified_entries
    )));
    lines.push(Line::from(""));
    lines.push(sync_heading("DIFFERENCES", Color::Yellow));

    let visible = result
        .diff
        .entries
        .iter()
        .filter(|entry| entry.state != DiffState::SameFingerprint)
        .collect::<Vec<_>>();
    for entry in visible.iter().take(40) {
        let (label, color) = match entry.state {
            DiffState::OnlyLeft => ("LEFT ONLY", Color::Yellow),
            DiffState::OnlyRight => ("RIGHT ONLY", Color::Yellow),
            DiffState::LeftNewer => ("LEFT NEWER", Color::Cyan),
            DiffState::RightNewer => ("RIGHT NEWER", Color::Cyan),
            DiffState::Different => ("COMPARE", Color::Red),
            DiffState::SameFingerprint => continue,
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{label:>11}"), Style::default().fg(color)),
            Span::raw(format!("  {}", entry.relative_path)),
        ]));
    }
    if visible.len() > 40 {
        lines.push(Line::from(format!(
            "… {} more verification entry/entries",
            visible.len() - 40
        )));
    }
}
fn render_verification_lines(job: &arx::jobs::Job, lines: &mut Vec<Line<'static>>) {
    let Some(verification) = &job.verification else {
        lines.push(Line::from(Span::styled(
            "Verification result is not available.",
            Style::default().fg(Color::DarkGray),
        )));
        return;
    };
    match &verification.status {
        SyncVerificationStatus::Finished(result) => match &result.verdict {
            SyncVerificationVerdict::Synchronized => {
                lines.push(Line::from(Span::styled(
                    "OK VERIFIED",
                    Style::default().fg(Color::Green),
                )));
                lines.push(Line::from("Both workspace roots are synchronized."));
            }
            SyncVerificationVerdict::DifferencesRemain {
                changed,
                conflicts,
                unverified,
            } => {
                lines.push(Line::from(Span::styled(
                    "! DIFFERENCES REMAIN",
                    Style::default().fg(Color::Yellow),
                )));
                lines.push(Line::from(format!("{changed} entries still differ")));
                lines.push(Line::from(format!("{conflicts} conflict(s)")));
                if *unverified > 0 {
                    lines.push(Line::from(format!("{unverified} entry/entries unverified")));
                }
                lines.push(Line::from(
                    "Preview the next sync to resolve current differences.",
                ));
            }
            SyncVerificationVerdict::Inconclusive { unverified } => {
                lines.push(Line::from(Span::styled(
                    "? INCONCLUSIVE",
                    Style::default().fg(Color::Yellow),
                )));
                lines.push(Line::from(format!(
                    "ARX cannot prove {unverified} entry/entries are identical."
                )));
                lines.push(Line::from("No mismatch was proven."));
            }
        },
        SyncVerificationStatus::Failed { error, .. } => {
            lines.push(Line::from(Span::styled(
                "! VERIFICATION FAILED",
                Style::default().fg(Color::Yellow),
            )));
            lines.push(Line::from(error.clone()));
            lines.push(Line::from("Execution truth above is unchanged."));
        }
        SyncVerificationStatus::Cancelled => {
            lines.push(Line::from(Span::styled(
                "Verification cancelled.",
                Style::default().fg(Color::Yellow),
            )));
        }
        SyncVerificationStatus::Superseded => {
            lines.push(Line::from(Span::styled(
                "Verification superseded by a newer workspace state.",
                Style::default().fg(Color::DarkGray),
            )));
        }
        SyncVerificationStatus::Pending | SyncVerificationStatus::Running { .. } => {
            lines.push(Line::from(Span::styled(
                "Verifying current workspace…",
                Style::default().fg(Color::Cyan),
            )));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn preview_renders_scanning_title_and_safety_text() {
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = AppState::default();

        terminal
            .draw(|frame| render(frame, frame.area(), &state))
            .unwrap();

        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("Workspace Sync · SCANNING"));
        assert!(text.contains("No files will be changed while ARX builds the preview."));
    }
}
