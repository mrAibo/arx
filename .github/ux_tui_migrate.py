from pathlib import Path

path = Path('src/tui.rs')
text = path.read_text()


def once(old: str, new: str, label: str) -> None:
    global text
    count = text.count(old)
    if count != 1:
        raise SystemExit(f'{label}: expected 1 match, found {count}')
    text = text.replace(old, new, 1)

once(
    '''use arx::app::{
    Action, ActionAvailability, AppState, CommandItem, CommandKind, CommandTarget, OverlayKind,
    Pane, PaneState, PanelMode, SortMode, action_meta, build_command_items,
};
''',
    '''use arx::app::{
    Action, ActionAvailability, AppState, CommandItem, CommandKind, CommandTarget, OverlayKind,
    Pane, PaneState, PanelMode, SortMode, WorkspaceSyncUxState, action_meta, build_command_items,
};
''',
    'tui app imports',
)
once(
    '''use arx::services::{
    DesktopService, FileInfoService, GitService, MutationError, MutationService, PaneLoadPurpose,
    PaneLoadResponse, PaneLoader, PreviewService, WorkspaceScanError, WorkspaceScanOptions,
    WorkspaceScanResponse, WorkspaceScanner,
};
''',
    '''use arx::services::{
    DesktopService, FileInfoService, GitService, MutationError, MutationService, PaneLoadPurpose,
    PaneLoadResponse, PaneLoader, PreviewService, WorkspaceScanError, WorkspaceScanOptions,
    WorkspaceScanResponse, WorkspaceScanner, WorkspaceSyncController,
};
''',
    'tui service imports',
)
once(
    'use arx::workspace_sync::{WorkspaceSide, WorkspaceSyncOperation};\n',
    '''use arx::workspace_sync::{SyncDirection, WorkspaceSide, WorkspaceSyncOperation};
use arx::workspace_sync_execution::SyncPlanId;
use arx::workspace_sync_verification::{
    SyncVerificationEvent, SyncVerificationStatus, SyncVerificationVerdict,
};
''',
    'tui sync imports',
)

insert_after = 'use tokio::sync::mpsc;\n'
insert = '''use tokio::sync::mpsc;

#[derive(Clone)]
struct SyncUiRuntime {
    controller: WorkspaceSyncController,
    jobs: arx::jobs::JobManager,
    job_events: mpsc::UnboundedSender<arx::jobs::JobEvent>,
    verification_events: mpsc::UnboundedSender<SyncVerificationEvent>,
    launch_events: mpsc::UnboundedSender<SyncLaunchResponse>,
}

struct SyncLaunchResponse {
    plan_id: SyncPlanId,
    result: Result<String, String>,
}
'''
once(insert_after, insert, 'sync runtime types')

once(
    '''    let job_manager = arx::jobs::JobManager::new();
    let (job_tx, mut job_rx) = mpsc::unbounded_channel::<arx::jobs::JobEvent>();
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(50));
''',
    '''    let job_manager = arx::jobs::JobManager::new();
    let (job_tx, mut job_rx) = mpsc::unbounded_channel::<arx::jobs::JobEvent>();
    let (verification_tx, mut verification_rx) =
        mpsc::unbounded_channel::<SyncVerificationEvent>();
    let (sync_launch_tx, mut sync_launch_rx) = mpsc::unbounded_channel::<SyncLaunchResponse>();
    let sync_runtime = SyncUiRuntime {
        controller: WorkspaceSyncController::new(state.registry.clone()),
        jobs: job_manager.clone(),
        job_events: job_tx.clone(),
        verification_events: verification_tx,
        launch_events: sync_launch_tx,
    };
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(50));
''',
    'sync runtime initialization',
)

# Add sync launch + verification branches before normal Job events.
once(
    '''            Some(ev) = job_rx.recv() => {
        // The manager already accepted this transition before publishing it.
        state.jobs = job_manager.snapshot();
''',
    '''            Some(response) = sync_launch_rx.recv() => {
                let still_current = state
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
                continue;
            }
            Some(event) = verification_rx.recv() => {
                let left_root = state.left.location.clone();
                let right_root = state.right.location.clone();
                if state.remote_workspace.apply_verification(
                    &event.verification,
                    &left_root,
                    &right_root,
                ) {
                    state.remote_workspace.sync_verification_stage(&event.job_id);
                    state.jobs = job_manager.snapshot();
                }
                continue;
            }
            Some(ev) = job_rx.recv() => {
        // The manager already accepted this transition before publishing it.
        state.jobs = job_manager.snapshot();
        let sync_job_id = job_event_id(&ev);
        if let Some(job) = job_manager.get(sync_job_id) {
            state.remote_workspace.sync_from_job(&job);
        }
''',
    'sync event loop branches',
)

# Both direct key routing and Command Center use the same controller-backed dispatcher.
once(
    '''                                &right_entries,
                                &workspace_scanner,
                            );
''',
    '''                                &right_entries,
                                &workspace_scanner,
                                &sync_runtime,
                            );
''',
    'key router sync runtime argument',
)
# Command-center execute target has a distinct following pane_loader arg.
once(
    '''                                        &right_entries,
                                        &workspace_scanner,
                                        &pane_loader,
                                    ) {
''',
    '''                                        &right_entries,
                                        &workspace_scanner,
                                        &pane_loader,
                                        &sync_runtime,
                                    ) {
''',
    'command target sync runtime argument',
)

# Rich renderer replaces the intentionally-disabled preview.
start = text.find('fn render_sync_preview(frame: &mut ratatui::Frame, area: Rect, state: &AppState) {')
end = text.find('\nfn render_help(', start)
if start < 0 or end < 0:
    raise SystemExit('sync renderer boundaries not found')
renderer = r'''fn render_sync_preview(frame: &mut ratatui::Frame, area: Rect, state: &AppState) {
    let popup = centered_rect(86, 82, area);
    frame.render_widget(Clear, popup);

    let mut lines = Vec::new();
    let mut title = " Workspace Sync ".to_string();
    let mut border = Style::default().fg(Color::Cyan);

    match &state.remote_workspace.ux {
        WorkspaceSyncUxState::Idle | WorkspaceSyncUxState::Scanning => {
            title = " Workspace Sync — SCANNING ".into();
            lines.push(Line::from("Scanning both workspace roots…"));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "No files will be changed. Esc hides this view.",
                Style::default().fg(Color::DarkGray),
            )));
        }
        WorkspaceSyncUxState::Preview { .. } => {
            render_sync_plan_lines(state, &mut lines);
            if let Some(plan) = state.remote_workspace.plan.as_ref() {
                title = if plan.can_execute() {
                    " Sync Preview — READY ".into()
                } else {
                    border = Style::default().fg(Color::Yellow);
                    " Sync Preview — RESOLVE CONFLICTS ".into()
                };
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "D reverse   M update/mirror   Enter execute   Esc hide",
                Style::default().fg(Color::DarkGray),
            )));
        }
        WorkspaceSyncUxState::ConfirmationRequired {
            digest,
            destructive_operations,
            ..
        } => {
            title = " Confirm Mirror Sync ".into();
            border = Style::default().fg(Color::Yellow);
            render_sync_plan_lines(state, &mut lines);
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("DELETE {destructive_operations} destructive operation(s)"),
                Style::default().fg(Color::Red),
            )));
            lines.push(Line::from(
                "Destination-only entries in this frozen plan may be removed.",
            ));
            lines.push(Line::from(format!(
                "Preview digest: {}…",
                &digest.as_hex()[..8]
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Esc back to preview                 Enter confirm exact plan",
                Style::default().fg(Color::DarkGray),
            )));
        }
        WorkspaceSyncUxState::Launching { .. } => {
            title = " Workspace Sync — PREPARING ".into();
            lines.push(Line::from("Freezing transport choice and execution steps…"));
            lines.push(Line::from("No Job has been created yet."));
        }
        WorkspaceSyncUxState::Blocked { message } => {
            title = " Workspace Sync — CANNOT EXECUTE ".into();
            border = Style::default().fg(Color::Yellow);
            lines.extend(message.lines().map(|line| Line::from(line.to_string())));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Esc hide   D/M adjust preview and try again",
                Style::default().fg(Color::DarkGray),
            )));
        }
        WorkspaceSyncUxState::Queued { job_id }
        | WorkspaceSyncUxState::Running { job_id }
        | WorkspaceSyncUxState::Cancelling { job_id }
        | WorkspaceSyncUxState::Verifying { job_id }
        | WorkspaceSyncUxState::Finished { job_id } => {
            if let Some(job) = state.jobs.iter().find(|job| job.id == *job_id) {
                title = sync_job_title(job, &state.remote_workspace.ux);
                render_sync_job_lines(job, &state.remote_workspace.ux, &mut lines);
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
    let (source, destination) = sync_display_roots(
        &plan.left_root,
        &plan.right_root,
        plan.policy.direction,
    );
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

    lines.push(Line::from(Span::styled(
        format!("{source}  →  {destination}"),
        Style::default().fg(Color::Cyan),
    )));
    lines.push(Line::from(format!(
        "{} · {}",
        state.remote_workspace.direction_label(),
        state.remote_workspace.mode_label()
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(format!("Copy / update     {copies}")));
    lines.push(Line::from(format!("Delete            {deletes}")));
    lines.push(Line::from(format!("Conflicts         {}", plan.conflicts)));
    lines.push(Line::from(format!(
        "Transfer          {}",
        format_size(plan.bytes_to_transfer)
    )));
    lines.push(Line::from(""));
    if plan.destructive_operations == 0 {
        lines.push(Line::from("This plan is non-destructive."));
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
        WorkspaceSyncUxState::Queued { .. } => format!(" Sync queued → {destination} "),
        WorkspaceSyncUxState::Running { .. } => format!(" Syncing → {destination} "),
        WorkspaceSyncUxState::Cancelling { .. } => " Cancelling… ".into(),
        WorkspaceSyncUxState::Verifying { .. } => " Execution finished — VERIFYING ".into(),
        WorkspaceSyncUxState::Finished { .. } => " Workspace Sync — RESULT ".into(),
        _ => " Workspace Sync ".into(),
    }
}

fn render_sync_job_lines(
    job: &arx::jobs::Job,
    ux: &WorkspaceSyncUxState,
    lines: &mut Vec<Line<'static>>,
) {
    if let (Some(source), Some(destination)) = (job.display_source(), job.display_destination()) {
        lines.push(Line::from(Span::styled(
            format!("{source}  →  {destination}"),
            Style::default().fg(Color::Cyan),
        )));
        lines.push(Line::from(""));
    }

    if let arx::jobs::JobProgress::WorkspaceSync(progress) = &job.progress {
        let percent = progress.percent().unwrap_or(0);
        let filled = usize::from(percent) / 5;
        lines.push(Line::from(format!(
            "[{}{}] {percent}%",
            "█".repeat(filled),
            "░".repeat(20usize.saturating_sub(filled))
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
            match &outcome.terminal {
                arx::workspace_sync_executor::SyncTerminalState::Completed => {
                    lines.push(Line::from("✓ Execution completed"));
                }
                arx::workspace_sync_executor::SyncTerminalState::Cancelled { .. } => {
                    lines.push(Line::from("Sync cancelled"));
                    lines.push(Line::from(format!(
                        "✓ {} physical step(s) completed",
                        outcome.completed.len()
                    )));
                    lines.push(Line::from(format!(
                        "○ {} physical step(s) not completed",
                        outcome.remaining.len()
                    )));
                    if outcome.workspace_may_have_changed {
                        lines.push(Line::from("Workspace may have changed."));
                    }
                }
                arx::workspace_sync_executor::SyncTerminalState::Failed { step, error } => {
                    lines.push(Line::from(Span::styled(
                        "✗ Sync partially completed",
                        Style::default().fg(Color::Red),
                    )));
                    lines.push(Line::from(format!(
                        "✓ {} physical step(s) completed",
                        outcome.completed.len()
                    )));
                    lines.push(Line::from(format!("✗ Step {} failed: {error}", step.0)));
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
                    "⚠ Audit record finalization failed",
                    Style::default().fg(Color::Yellow),
                )));
                lines.push(Line::from(format!("{error}")));
                lines.push(Line::from("The physical result was preserved."));
            }
        }
        Some(arx::jobs::JobResult::Generic { .. }) | None => {}
    }

    if matches!(ux, WorkspaceSyncUxState::Cancelling { .. }) {
        lines.push(Line::from(Span::styled(
            "Cancelling… waiting for the executor's terminal outcome.",
            Style::default().fg(Color::Yellow),
        )));
    }

    if matches!(ux, WorkspaceSyncUxState::Verifying { .. }) {
        lines.push(Line::from(""));
        lines.push(Line::from("Verifying current workspace…"));
        lines.push(Line::from("Scanning both workspace roots again."));
    }

    if matches!(ux, WorkspaceSyncUxState::Finished { .. }) {
        lines.push(Line::from(""));
        render_verification_lines(job, lines);
    }

    lines.push(Line::from(""));
    let footer = match ux {
        WorkspaceSyncUxState::Queued { .. } | WorkspaceSyncUxState::Running { .. } => {
            "C cancel   Esc hide"
        }
        WorkspaceSyncUxState::Cancelling { .. } | WorkspaceSyncUxState::Verifying { .. } => {
            "Esc hide"
        }
        WorkspaceSyncUxState::Finished { .. } => "B back to current preview   Esc hide",
        _ => "Esc hide",
    };
    lines.push(Line::from(Span::styled(
        footer,
        Style::default().fg(Color::DarkGray),
    )));
}

fn render_verification_lines(job: &arx::jobs::Job, lines: &mut Vec<Line<'static>>) {
    let Some(verification) = &job.verification else {
        return;
    };
    match &verification.status {
        SyncVerificationStatus::Finished(result) => match &result.verdict {
            SyncVerificationVerdict::Synchronized => {
                lines.push(Line::from("✓ Workspace verified"));
                lines.push(Line::from("Both workspace roots are synchronized."));
            }
            SyncVerificationVerdict::DifferencesRemain {
                changed,
                conflicts,
                unverified,
            } => {
                lines.push(Line::from(Span::styled(
                    "⚠ Verification found differences",
                    Style::default().fg(Color::Yellow),
                )));
                lines.push(Line::from(format!("{changed} entries still differ")));
                lines.push(Line::from(format!("{conflicts} conflict(s)")));
                if *unverified > 0 {
                    lines.push(Line::from(format!("{unverified} entry/entries unverified")));
                }
                lines.push(Line::from("Preview the next sync to resolve current differences."));
            }
            SyncVerificationVerdict::Inconclusive { unverified } => {
                lines.push(Line::from("? Verification inconclusive"));
                lines.push(Line::from(format!(
                    "ARX cannot prove {unverified} entry/entries are identical."
                )));
                lines.push(Line::from("No mismatch was proven."));
            }
        },
        SyncVerificationStatus::Failed { error, .. } => {
            lines.push(Line::from(Span::styled(
                "⚠ Workspace verification could not finish",
                Style::default().fg(Color::Yellow),
            )));
            lines.push(Line::from(error.clone()));
            lines.push(Line::from("Execution truth above is unchanged."));
        }
        SyncVerificationStatus::Cancelled => {
            lines.push(Line::from("Verification cancelled."));
        }
        SyncVerificationStatus::Superseded => {
            lines.push(Line::from("Verification superseded by a newer workspace state."));
        }
        SyncVerificationStatus::Pending | SyncVerificationStatus::Running { .. } => {
            lines.push(Line::from("Verifying current workspace…"));
        }
    }
}
'''
text = text[:start] + renderer + text[end:]

# Job event helper and controller-backed action dispatcher.
once(
    '/// Present an already-accepted JobManager event. Lifecycle state lives in JobManager.\n',
    '''fn job_event_id(event: &arx::jobs::JobEvent) -> &str {
    match event {
        arx::jobs::JobEvent::Running { id }
        | arx::jobs::JobEvent::Paused { id }
        | arx::jobs::JobEvent::Progress { id, .. }
        | arx::jobs::JobEvent::Completed { id, .. }
        | arx::jobs::JobEvent::Failed { id, .. }
        | arx::jobs::JobEvent::Cancelled { id, .. } => id,
    }
}

/// Present an already-accepted JobManager event. Lifecycle state lives in JobManager.
''',
    'job event id helper',
)

# Replace dispatcher + command-target function as one block to avoid argument drift.
start = text.find('fn dispatch_ui_action(')
end = text.find('\nfn start_workspace_scan(', start)
if start < 0 or end < 0:
    raise SystemExit('dispatcher boundaries not found')
dispatcher = r'''fn dispatch_ui_action(
    state: &mut AppState,
    action: Action,
    focused: Option<&Entry>,
    _left_entries: &[Entry],
    _right_entries: &[Entry],
    workspace_scanner: &WorkspaceScanner,
    sync: &SyncUiRuntime,
) {
    match action {
        Action::Quit => state.apply(action),
        Action::OpenCommandCenter => {
            state.open_overlay(OverlayKind::CommandCenter);
            state.filter.clear();
            state.command_matches = build_command_items("", state);
            state
                .overlay_list_state
                .select((!state.command_matches.is_empty()).then_some(0));
        }
        Action::OpenBookmarks => state.toggle_overlay(OverlayKind::Bookmarks),
        Action::OpenJobs => state.toggle_overlay(OverlayKind::Jobs),
        Action::OpenHosts => {
            if state.hosts.is_empty() {
                state.message = Some("No hosts configured — add ~/.config/arx/hosts.toml".into());
            } else {
                state.toggle_overlay(OverlayKind::Hosts);
            }
        }
        Action::OpenHelp => state.toggle_overlay(OverlayKind::Help),
        Action::BeginSymlink => {
            if let Some(entry) = focused {
                state.cmd = format!("ln -s '{}' ", entry.name);
                state.cmd_input = true;
            }
        }
        Action::BeginChmod => {
            state.cmd = "chmod ".into();
            state.cmd_input = true;
        }
        Action::BeginHardLink => {
            if let Some(entry) = focused {
                state.cmd = format!("ln '{}' ", entry.name);
                state.cmd_input = true;
            }
        }
        Action::BeginChown => {
            state.cmd = "chown ".into();
            state.cmd_input = true;
        }
        Action::ToggleWorkspaceComparison => {
            if state.remote_workspace.enabled {
                state.remote_workspace.disable();
                state.show_diff = false;
                state.message = Some("Remote Workspace comparison off".into());
            } else {
                start_workspace_scan(workspace_scanner, state, false);
            }
        }
        Action::PreviewWorkspaceSync => {
            start_workspace_scan(workspace_scanner, state, true);
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
        Action::ReturnToWorkspaceSyncPreview => {
            if matches!(
                state.remote_workspace.ux,
                WorkspaceSyncUxState::ConfirmationRequired { .. }
                    | WorkspaceSyncUxState::Blocked { .. }
                    | WorkspaceSyncUxState::Finished { .. }
            ) {
                state.remote_workspace.mark_preview();
            } else if state.remote_workspace.ux.is_job_flow() {
                state.message = Some("The active sync remains in its Job view until it is finished.".into());
            }
        }
        Action::CloseWorkspaceSyncPreview => state.close_overlay(OverlayKind::SyncPreview),
        _ => state.apply(action),
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
    state.remote_workspace.mark_launching();
    let controller = sync.controller.clone();
    let jobs = sync.jobs.clone();
    let job_events = sync.job_events.clone();
    let verification_events = sync.verification_events.clone();
    let launch_events = sync.launch_events.clone();
    tokio::spawn(async move {
        let result = controller
            .launch(
                frozen,
                diff,
                confirmed,
                jobs,
                job_events,
                verification_events,
            )
            .await
            .map_err(|error| error.user_message());
        let _ = launch_events.send(SyncLaunchResponse { plan_id, result });
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

fn execute_command_target(
    state: &mut AppState,
    target: CommandTarget,
    focused: Option<&Entry>,
    left_entries: &[Entry],
    right_entries: &[Entry],
    workspace_scanner: &WorkspaceScanner,
    pane_loader: &PaneLoader,
    sync: &SyncUiRuntime,
) -> Option<Effect> {
    match target {
        CommandTarget::Action(action) => {
            dispatch_ui_action(
                state,
                action,
                focused,
                left_entries,
                right_entries,
                workspace_scanner,
                sync,
            );
            None
        }
        CommandTarget::Location(location) => {
            let active = state.active;
            schedule_pane_navigation(
                pane_loader,
                state,
                active,
                location,
                PaneLoadPurpose::Navigate {
                    remember_current: true,
                },
            );
            None
        }
        CommandTarget::Host { ssh_alias, path } => {
            let active = state.active;
            schedule_pane_navigation(
                pane_loader,
                state,
                active,
                Location::Sftp {
                    host: ssh_alias,
                    path,
                },
                PaneLoadPurpose::Navigate {
                    remember_current: true,
                },
            );
            None
        }
        CommandTarget::TmuxSession(session) => {
            state.message = Some(format!("Attaching tmux: {session} (Ctrl+B D to detach)"));
            Some(Effect::AttachTmux { session })
        }
        CommandTarget::ScreenSession(session) => {
            state.message = Some(format!("Attaching screen: {session}"));
            Some(Effect::AttachScreen { session })
        }
        CommandTarget::ShellCommand(command) => Some(Effect::SpawnShell { command }),
    }
}
'''
text = text[:start] + dispatcher + text[end:]

path.write_text(text)
