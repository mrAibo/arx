use crate::tui_terminal::TuiTerminalSession;
use arx::app::{
    Action, ActionAvailability, ActionContext, ActionId, AppState, CommandItem, CommandKind,
    CommandTarget, OverlayKind, Pane, PaneLoadUiError, PaneState, PanelMode, SessionCallout,
    SortMode, WorkspaceSyncUxState, action_availability, action_meta,
    build_command_items_with_file_context,
};
use arx::effect_dispatcher::{EffectDispatcher, EffectLane, EffectResponse, EffectScope};
use arx::effects::{Effect, EffectEvent};
#[cfg(test)]
use arx::input::contextual_hints_with_file_context;
use arx::input::{ContextHint, KeyResolution, KeyRouter, command_bar_rows, contextual_hints};
use arx::services::{
    DesktopService, FileInfoService, GitService, MutationError, MutationService, PaneLoadPurpose,
    PaneLoadResponse, PaneLoader, SyncLaunchId, WorkspaceScanError, WorkspaceScanOptions,
    WorkspaceScanResponse, WorkspaceScanner, WorkspaceSyncController,
};
use arx::vfs::{Entry, EntryKind, Location, RemoteEditSession, RemoteEditState};
use arx::workspace_sync::{
    DiffState, SyncDirection, SyncMode, WorkspaceSide, WorkspaceSyncOperation,
};
use arx::workspace_sync_execution::SyncPlanId;
use arx::workspace_sync_verification::{
    SyncVerificationEvent, SyncVerificationStatus, SyncVerificationVerdict,
};
use crossterm::event::{self, Event, KeyCode, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::{
    DefaultTerminal,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use std::io;
use std::path::PathBuf;
use tokio::sync::mpsc;

#[derive(Clone)]
struct SyncUiRuntime {
    controller: WorkspaceSyncController,
    jobs: arx::jobs::JobManager,
    job_events: mpsc::UnboundedSender<arx::jobs::JobEvent>,
    verification_events: mpsc::UnboundedSender<SyncVerificationEvent>,
    launch_events: mpsc::UnboundedSender<SyncLaunchResponse>,
}

struct SyncLaunchResponse {
    launch_id: SyncLaunchId,
    plan_id: SyncPlanId,
    result: Result<String, String>,
}

pub async fn run(config: arx::config::ArxConfig) -> io::Result<()> {
    let mut terminal_session = TuiTerminalSession::enter()?;
    let stdout = io::stdout();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(stdout))?;

    let result = event_loop(&mut terminal, &mut terminal_session, config).await;
    let restore_result = terminal_session.restore();
    match (result, restore_result) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

#[allow(clippy::collapsible_if)]
async fn event_loop(
    terminal: &mut DefaultTerminal,
    terminal_session: &mut TuiTerminalSession,
    config: arx::config::ArxConfig,
) -> io::Result<()> {
    let editor = DesktopService::resolve_editor(config.ui.editor.as_deref());
    let mut state = AppState {
        show_hidden: config.ui.show_hidden,
        hosts: arx::remote::hosts_config::load_hosts(),
        menu: AppState::load_menu(),
        ..AppState::default()
    };
    let (pane_loader, mut pane_load_rx) = PaneLoader::channel(state.registry.clone());
    let (workspace_scanner, mut workspace_scan_rx) =
        WorkspaceScanner::channel(state.registry.clone());
    let mut left_entries = Vec::new();
    let mut right_entries = Vec::new();
    schedule_pane_load(&pane_loader, &mut state, Pane::Left);
    schedule_pane_load(&pane_loader, &mut state, Pane::Right);
    let mut left_list = ListState::default();
    let mut right_list = ListState::default();
    let mut split_left_list = ListState::default();
    let mut split_right_list = ListState::default();
    let mut key_router = KeyRouter::default();
    let (effect_dispatcher, mut effect_rx) = EffectDispatcher::channel(state.registry.clone());
    // JobManager is the runtime source of truth. AppState.jobs is only a render snapshot.
    let job_manager = arx::jobs::JobManager::new();
    let (job_tx, mut job_rx) = mpsc::unbounded_channel::<arx::jobs::JobEvent>();
    let (verification_tx, mut verification_rx) = mpsc::unbounded_channel::<SyncVerificationEvent>();
    let (sync_launch_tx, mut sync_launch_rx) = mpsc::unbounded_channel::<SyncLaunchResponse>();
    let sync_runtime = SyncUiRuntime {
        controller: WorkspaceSyncController::new(state.registry.clone()),
        jobs: job_manager.clone(),
        job_events: job_tx.clone(),
        verification_events: verification_tx,
        launch_events: sync_launch_tx,
    };
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(50));
    let parent_entry = virtual_parent_entry();

    loop {
        if state.should_quit {
            break;
        }
        // Cache Git status by location instead of spawning two git processes
        // from render() on every 50ms frame.
        let active_location = state.active_pane().location.clone();
        if state.git_status_location.as_ref() != Some(&active_location) {
            state.git_status = GitService::status_suffix(&active_location).await;
            state.git_status_location = Some(active_location);
        }
        let left_filtered = apply_filter_with_parent(
            &left_entries,
            &state.filter,
            &state.left.location,
            &parent_entry,
        );
        let right_filtered = apply_filter_with_parent(
            &right_entries,
            &state.filter,
            &state.right.location,
            &parent_entry,
        );
        // clamp cursors
        state.left.cursor = state.left.cursor.min(left_filtered.len().saturating_sub(1));
        state.right.cursor = state
            .right
            .cursor
            .min(right_filtered.len().saturating_sub(1));

        left_list.select(Some(state.left.cursor));
        split_left_list.select(Some(state.left.split_cursor));
        right_list.select(Some(state.right.cursor));
        split_right_list.select(Some(state.right.split_cursor));

        let msg = state.message.clone();
        terminal.draw(|frame| {
            render(
                frame,
                &mut state,
                left_entries.len(),
                right_entries.len(),
                &left_filtered,
                &right_filtered,
                &mut left_list,
                &mut right_list,
                &mut split_left_list,
                &mut split_right_list,
                &key_router,
                editor.is_some(),
                msg.as_deref(),
            )
        })?;
        state.message = None; // one-shot clear after render

        // Drain terminal output if active
        if let Some(ref mut term) = state.term {
            term.drain();
        }

        // ── tokio::select! async event loop ──
        // Unified dispatch: crossterm key/mouse + background jobs + PTY
        let next_input = tokio::select! {
            Some(response) = workspace_scan_rx.recv() => {
                handle_workspace_scan_response(response, &mut state);
                continue;
            }
            Some(response) = pane_load_rx.recv() => {
                apply_pane_load_response(
                    response,
                    &mut state,
                    &mut left_entries,
                    &mut right_entries,
                );
                continue;
            }
            Some(response) = effect_rx.recv() => {
                let mut response = response;
                finalize_received_effect(&effect_dispatcher, &mut response);
                handle_effect_response(
                    response,
                    &mut state,
                    &mut left_entries,
                    &mut right_entries,
                    &pane_loader,
                );

                // Defer editor launch until after terminal input is polled,
                // so queued Quit/navigation input is processed first.
                if state.pending_remote_edit_session.is_some() {
                    state.pending_editor = true;
                }

                continue;
            }
            Some(response) = sync_launch_rx.recv() => {
                let still_current = sync_runtime
                    .controller
                    .is_launch_current(response.launch_id)
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
                continue;
            }
            Some(event) = verification_rx.recv() => {
                let left_root = state.left.location.clone();
                let right_root = state.right.location.clone();
                let accepted = state.remote_workspace.apply_verification(
                    &event.verification,
                    &left_root,
                    &right_root,
                );
                // JobManager accepted the verification before publishing this
                // event, so its render snapshot is useful even when pane roots
                // have moved and RemoteWorkspaceState rejects the old diff.
                state.jobs = job_manager.snapshot();
                if let Some(job) = job_manager.get(&event.job_id) {
                    observe_verified_sync_success(&mut state, &job);
                }
                if accepted {
                    state.remote_workspace.sync_verification_stage(&event.job_id);
                } else {
                    state
                        .remote_workspace
                        .settle_rejected_verification(&event.job_id, &event.verification);
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
        if let arx::jobs::JobEvent::Failed { id, error, .. } = &ev {
                    let body = format!("Job {id} failed: {error}");
                    tokio::spawn(async move {
                        DesktopService::notify("ARX", &body).await;
                    });
                }
                let refresh_panes = handle_job_event(&ev, &mut state);
                if refresh_panes {
                    schedule_pane_load(&pane_loader, &mut state, Pane::Left);
                    schedule_pane_load(&pane_loader, &mut state, Pane::Right);
                }
                if let Some(ref mut term) = state.term { term.drain(); }
                continue;
            }
            _ = tick.tick() => {
                if event::poll(std::time::Duration::ZERO)? {
                    Some(event::read()?)
                } else {
                    if let Some(ref mut term) = state.term { term.drain(); }
                    if state.pending_editor {
                        None
                    } else {
                        continue;
                    }
                }
            }
        };
        if let Some(event) = next_input {
            if matches!(&event, Event::Key(_) | Event::Mouse(_)) {
                state.dismiss_session_callout();
            }
            match event {
                Event::Key(key) if state.show_terminal && state.active == Pane::Right => {
                    use crossterm::event::KeyCode as KC;
                    if let Some(ref mut term) = state.term {
                        match key.code {
                            KC::Esc => {
                                // Toggle back to file browser
                                state.show_terminal = false;
                                if let Some(ref mut t) = state.term {
                                    t.kill();
                                }
                                state.term = None;
                                state.message = Some("Terminal closed".into());
                            }
                            KC::Enter => term.write("\r\n"),
                            KC::Backspace => term.write("\x7f"),
                            KC::Tab => term.write("\t"),
                            KC::Up => term.write("\x1b[A"),
                            KC::Down => term.write("\x1b[B"),
                            KC::Left => term.write("\x1b[D"),
                            KC::Right => term.write("\x1b[C"),
                            KC::Home => term.write("\x1b[H"),
                            KC::End => term.write("\x1b[F"),
                            KC::Char(c) => {
                                let mut buf = [0u8; 4];
                                let s = c.encode_utf8(&mut buf);
                                term.write(s);
                            }
                            _ => {}
                        }
                    }
                }
                Event::Mouse(mouse) => {
                    // Check command bar hitboxes first (before pane area)
                    if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
                        let hitboxes: Vec<_> = state.command_hitboxes.clone();
                        for hb in &hitboxes {
                            if mouse.row == hb.row
                                && mouse.column >= hb.col
                                && mouse.column < hb.col + hb.width
                            {
                                if hb.available {
                                    dispatch_ui_action(
                                        &mut state,
                                        hb.action,
                                        None,
                                        &[],
                                        0,
                                        &workspace_scanner,
                                        &sync_runtime,
                                        &effect_dispatcher,
                                        &pane_loader,
                                        terminal_session,
                                        editor.as_deref(),
                                    )
                                    .await?;
                                } else {
                                    let ctx = ActionContext::from_state(&state);
                                    let action_id = action_to_id(hb.action);
                                    let avail = action_availability(action_id, &ctx);
                                    state.message =
                                        Some(avail.reason().unwrap_or("unavailable").to_string());
                                }
                                continue; // handled by command bar
                            }
                        }
                    }
                    // Compute pane + row once for all mouse events
                    let (area, is_left) = if let Some(a) = state.left_area {
                        if mouse.column >= a.x
                            && mouse.column < a.x + a.width
                            && mouse.row > a.y
                            && mouse.row < a.y + a.height
                        {
                            (a, true)
                        } else if let Some(a) = state.right_area {
                            (a, false)
                        } else {
                            continue;
                        }
                    } else {
                        continue;
                    };
                    let row = (mouse.row.saturating_sub(area.y + 1)) as usize;

                    match mouse.kind {
                        MouseEventKind::ScrollDown if !state.viewer_content.is_empty() => {
                            state.viewer_scroll = (state.viewer_scroll + 1)
                                .min(state.viewer_content.len().saturating_sub(1));
                        }
                        MouseEventKind::ScrollUp if !state.viewer_content.is_empty() => {
                            state.viewer_scroll = state.viewer_scroll.saturating_sub(1);
                        }
                        MouseEventKind::Down(MouseButton::Right) => {
                            state.show_context_menu = !state.show_context_menu;
                            state.context_menu_pos = (mouse.column, mouse.row);
                        }
                        MouseEventKind::Drag(MouseButton::Left) => {
                            let filt = if is_left {
                                &left_filtered
                            } else {
                                &right_filtered
                            };
                            if let Some(entry) = filt
                                .get(row)
                                .filter(|entry| entry.name != VIRTUAL_PARENT_NAME)
                            {
                                let pane = if is_left { Pane::Left } else { Pane::Right };
                                let location = if is_left {
                                    state.left.location.clone()
                                } else {
                                    state.right.location.clone()
                                };
                                state.toggle_selection(pane, &location, &entry.name);
                            }
                        }
                        MouseEventKind::Down(_) => {
                            let filt = if is_left {
                                &left_filtered
                            } else {
                                &right_filtered
                            };
                            if row < filt.len() {
                                if is_left {
                                    state.left.cursor = row;
                                } else {
                                    state.right.cursor = row;
                                }
                                state.active = if is_left { Pane::Left } else { Pane::Right };
                            }
                        }
                        _ => {}
                    }
                }
                Event::Key(key) => {
                    // Remote delete confirmation intercepts Enter/Escape
                    if state.pending_delete.is_some() {
                        match key.code {
                            KeyCode::Enter => {
                                dispatch_ui_action(
                                    &mut state,
                                    Action::ConfirmRemoteDelete,
                                    None, // no focused entry during confirmation
                                    &[],
                                    0,
                                    &workspace_scanner,
                                    &sync_runtime,
                                    &effect_dispatcher,
                                    &pane_loader,
                                    terminal_session,
                                    editor.as_deref(),
                                )
                                .await?;
                                continue;
                            }
                            KeyCode::Esc => {
                                dispatch_ui_action(
                                    &mut state,
                                    Action::CancelRemoteDelete,
                                    None,
                                    &[],
                                    0,
                                    &workspace_scanner,
                                    &sync_runtime,
                                    &effect_dispatcher,
                                    &pane_loader,
                                    terminal_session,
                                    editor.as_deref(),
                                )
                                .await?;
                                continue;
                            }
                            _ => {
                                // Ignore other keys during confirmation
                                continue;
                            }
                        }
                    }
                    // Command Center owns keyboard input while open. Keep this
                    // before generic text-input routing so Ctrl+P has a real,
                    // usable interaction model.
                    if state.show_command_center {
                        match key.code {
                            KeyCode::Esc => {
                                state.show_command_center = false;
                                state.filter.clear();
                                state.command_matches.clear();
                                state.overlay_list_state = ratatui::widgets::ListState::default();
                            }
                            KeyCode::Enter => {
                                let idx = state.overlay_list_state.selected().unwrap_or(0);
                                let idx = idx.min(state.command_matches.len().saturating_sub(1));
                                if let Some(item) = state.command_matches.get(idx).cloned() {
                                    if let ActionAvailability::Disabled { reason } =
                                        &item.availability
                                    {
                                        state.message = Some(reason.clone());
                                        continue;
                                    }
                                    state.show_command_center = false;
                                    state.filter.clear();
                                    state.command_matches.clear();
                                    let cursor = {
                                        let pane = state.active_pane();
                                        if pane.split && pane.split_active {
                                            pane.split_cursor
                                        } else {
                                            pane.cursor
                                        }
                                    };
                                    let (focused_entry, visible_count) =
                                        if state.active == Pane::Left {
                                            (left_filtered.get(cursor), left_filtered.len())
                                        } else {
                                            (right_filtered.get(cursor), right_filtered.len())
                                        };
                                    let active_entries: &[&Entry] = if state.active == Pane::Left {
                                        &left_filtered[..]
                                    } else {
                                        &right_filtered[..]
                                    };
                                    if let Some(effect) = execute_command_target(
                                        &mut state,
                                        item.target,
                                        focused_entry.copied(),
                                        active_entries,
                                        visible_count,
                                        &workspace_scanner,
                                        &pane_loader,
                                        &sync_runtime,
                                        &effect_dispatcher,
                                        terminal_session,
                                        editor.as_deref(),
                                    )
                                    .await?
                                    {
                                        let id = effect_dispatcher.dispatch(
                                            EffectLane::GlobalProcess,
                                            EffectScope::Global,
                                            effect,
                                        );
                                        state.register_effect(EffectLane::GlobalProcess, id);
                                    }
                                    schedule_both_pane_loads(&pane_loader, &mut state);
                                }
                            }
                            KeyCode::Up | KeyCode::Char('k')
                                if !key.modifiers.contains(KeyModifiers::CONTROL) =>
                            {
                                let current = state.overlay_list_state.selected().unwrap_or(0);
                                state
                                    .overlay_list_state
                                    .select(Some(current.saturating_sub(1)));
                            }
                            KeyCode::Down | KeyCode::Char('j')
                                if !key.modifiers.contains(KeyModifiers::CONTROL) =>
                            {
                                let current = state.overlay_list_state.selected().unwrap_or(0);
                                let max = state.command_matches.len().saturating_sub(1);
                                state
                                    .overlay_list_state
                                    .select(Some((current + 1).min(max)));
                            }
                            KeyCode::Backspace => {
                                state.filter.pop();
                                state.command_matches = build_command_items_with_file_context(
                                    &state.filter,
                                    &state,
                                    focused_entry(&state, &left_filtered, &right_filtered)
                                        .map(|entry| entry.kind),
                                    editor.is_some(),
                                );
                                state
                                    .overlay_list_state
                                    .select((!state.command_matches.is_empty()).then_some(0));
                            }
                            KeyCode::Char(c)
                                if !key.modifiers.contains(KeyModifiers::CONTROL)
                                    && !key.modifiers.contains(KeyModifiers::ALT) =>
                            {
                                state.filter.push(c);
                                state.command_matches = build_command_items_with_file_context(
                                    &state.filter,
                                    &state,
                                    focused_entry(&state, &left_filtered, &right_filtered)
                                        .map(|entry| entry.kind),
                                    editor.is_some(),
                                );
                                state
                                    .overlay_list_state
                                    .select((!state.command_matches.is_empty()).then_some(0));
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // If composing filter/glob/go-to, keys go to buffer
                    if state.filtering || state.glob_input || state.go_input || state.cmd_input {
                        match key.code {
                            KeyCode::Esc => {
                                state.filter.clear();
                                state.filtering = false;
                                state.glob_input = false;
                                state.go_input = false;
                                state.cmd_input = false;
                            }
                            KeyCode::Enter => {
                                if state.glob_input && !state.filter.is_empty() {
                                    let filt = if state.active == Pane::Left {
                                        &left_filtered
                                    } else {
                                        &right_filtered
                                    };
                                    let active = state.active;
                                    let location = state.active_pane().location.clone();
                                    for e in filt {
                                        if e.name != VIRTUAL_PARENT_NAME
                                            && !state.is_selected(active, &location, &e.name)
                                        {
                                            state.toggle_selection(active, &location, &e.name);
                                        }
                                    }
                                    state.message = Some(format!(
                                        "Selected {}",
                                        state.selection_count(active, &location)
                                    ));
                                    state.filter.clear();
                                } else if state.go_input && !state.filter.is_empty() {
                                    // Navigate to typed path
                                    let target = PathBuf::from(&state.filter);
                                    let resolved = if target.is_absolute() {
                                        target
                                    } else {
                                        match &state.active_pane().location {
                                            Location::Local(p) => p.join(&target),
                                            _ => target,
                                        }
                                    };
                                    let active = state.active;
                                    schedule_pane_navigation(
                                        &pane_loader,
                                        &mut state,
                                        active,
                                        Location::Local(resolved),
                                        PaneLoadPurpose::Navigate {
                                            remember_current: true,
                                        },
                                    );
                                    state.message = Some("Opening path…".into());
                                    state.filter.clear();
                                }
                                if state.cmd_input {
                                    let command = std::mem::take(&mut state.cmd);
                                    let pending_mkdir =
                                        std::mem::take(&mut state.pending_mkdir_location);
                                    state.cmd_input = false;
                                    if command.is_empty() {
                                        state.message = Some(": command cancelled".into());
                                    } else if let Some(loc) = pending_mkdir {
                                        // SFTP provider-backed mkdir
                                        let name = command;
                                        // ponytail: validate location type early.
                                        if !matches!(
                                            &loc,
                                            Location::Local(_) | Location::Sftp { .. }
                                        ) {
                                            state.message =
                                                Some("mkdir: unsupported location".into());
                                            continue;
                                        }
                                        // Validate child name — reject empty, ".", "..", "/", NUL.
                                        if let Err(e) = arx::vfs::validate_child_name(&name) {
                                            state.message = Some(e.to_string());
                                            continue;
                                        }
                                        let registry = state.registry.clone();
                                        let name_for_msg = name.clone();
                                        let pane = state.active;
                                        let pane_location = loc.clone();
                                        let loader = pane_loader.clone();
                                        let job = job_manager.create_job(
                                            "mkdir",
                                            arx::jobs::JobKind::RemoteCommand,
                                            format!("mkdir {name}"),
                                            Some(loc.clone()),
                                            None,
                                        );
                                        state.jobs = job_manager.snapshot();
                                        let jobs = job_manager.clone();
                                        let tx = job_tx.clone();
                                        {
                                            let jid = job.id.clone();
                                            let _ = jobs.publish_event(
                                                &job_tx,
                                                arx::jobs::JobEvent::Running { id: jid },
                                            );
                                        }
                                        tokio::spawn(async move {
                                            let result = registry.mkdir_at(&loc, &name).await;
                                            match result {
                                                Ok(()) => {
                                                    let _ = jobs.publish_event(
                                                        &tx,
                                                        arx::jobs::JobEvent::Completed {
                                                            id: job.id,
                                                            result: arx::jobs::JobResult::generic(
                                                                "created", 1,
                                                            ),
                                                        },
                                                    );
                                                    let _ = loader.load(
                                                        pane,
                                                        pane_location,
                                                        PaneLoadPurpose::Refresh,
                                                    );
                                                }
                                                Err(e) => {
                                                    let _ = jobs.publish_event(
                                                        &tx,
                                                        arx::jobs::JobEvent::Failed {
                                                            id: job.id,
                                                            error: e.to_string(),
                                                            result: None,
                                                        },
                                                    );
                                                }
                                            }
                                        });
                                        state.message = Some(format!("mkdir {name_for_msg}…"));
                                    } else {
                                        let id = effect_dispatcher.dispatch(
                                            EffectLane::GlobalProcess,
                                            EffectScope::Global,
                                            Effect::RunShellCapture { command },
                                        );
                                        state.register_effect(EffectLane::GlobalProcess, id);
                                        state.message = Some("Command started…".into());
                                    }
                                }
                                state.filtering = false;
                                state.glob_input = false;
                                state.go_input = false;
                            }
                            KeyCode::Backspace => {
                                if state.cmd_input {
                                    state.cmd.pop();
                                } else {
                                    state.filter.pop();
                                }
                            }
                            KeyCode::Char(c) => {
                                if state.cmd_input {
                                    state.cmd.push(c);
                                } else {
                                    state.filter.push(c);
                                    if state.show_command_center {
                                        state.command_matches =
                                            build_command_items_with_file_context(
                                                &state.filter,
                                                &state,
                                                focused_entry(
                                                    &state,
                                                    &left_filtered,
                                                    &right_filtered,
                                                )
                                                .map(|entry| entry.kind),
                                                editor.is_some(),
                                            );
                                    }
                                }
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // Viewer mode: takes over until dismissed
                    if !state.viewer_content.is_empty() {
                        match key.code {
                            KeyCode::Esc | KeyCode::Char('q') | KeyCode::F(3) => {
                                state.viewer_content.clear();
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                state.viewer_scroll = state.viewer_scroll.saturating_sub(1);
                            }
                            KeyCode::Down | KeyCode::Char('j')
                                if !key.modifiers.contains(KeyModifiers::CONTROL) =>
                            {
                                let max = state.viewer_content.len().saturating_sub(1);
                                if state.viewer_scroll < max {
                                    state.viewer_scroll += 1;
                                }
                            }
                            KeyCode::PageUp => {
                                state.viewer_scroll = state.viewer_scroll.saturating_sub(20);
                            }
                            KeyCode::PageDown => {
                                let max = state.viewer_content.len().saturating_sub(1);
                                state.viewer_scroll = (state.viewer_scroll + 20).min(max);
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // Bookmarks mode
                    if state.show_bookmarks {
                        match key.code {
                            KeyCode::Esc | KeyCode::Char('b')
                                if key.modifiers.contains(KeyModifiers::CONTROL) =>
                            {
                                state.show_bookmarks = false;
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                state.bookmark_cursor = state.bookmark_cursor.saturating_sub(1);
                            }
                            KeyCode::Down | KeyCode::Char('j')
                                if !key.modifiers.contains(KeyModifiers::CONTROL) =>
                            {
                                let max = state.bookmarks.len().saturating_sub(1);
                                if state.bookmark_cursor < max {
                                    state.bookmark_cursor += 1;
                                }
                            }
                            KeyCode::Enter => {
                                let loc = state.bookmarks.get(state.bookmark_cursor).cloned();
                                if let Some(loc) = loc {
                                    let active = state.active;
                                    state.close_all_overlays();
                                    schedule_pane_navigation(
                                        &pane_loader,
                                        &mut state,
                                        active,
                                        loc,
                                        PaneLoadPurpose::Navigate {
                                            remember_current: true,
                                        },
                                    );
                                    state.message = Some("Opening bookmark…".into());
                                }
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // Hosts panel: Esc to close
                    if state.show_hosts {
                        match key.code {
                            KeyCode::Esc => {
                                state.show_hosts = false;
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                state.host_cursor = state.host_cursor.saturating_sub(1);
                            }
                            KeyCode::Down | KeyCode::Char('j')
                                if !key.modifiers.contains(KeyModifiers::CONTROL) =>
                            {
                                let max = state.hosts.len().saturating_sub(1);
                                if state.host_cursor < max {
                                    state.host_cursor += 1;
                                }
                            }
                            KeyCode::Enter => {
                                let host = state.hosts.get(state.host_cursor).cloned();
                                if let Some(host) = host {
                                    let default_path = host.default_path.as_deref().unwrap_or("/");
                                    let target = Location::Sftp {
                                        host: host.id.clone(),
                                        path: default_path.into(),
                                    };
                                    let active = state.active;
                                    state.close_all_overlays();
                                    schedule_pane_navigation(
                                        &pane_loader,
                                        &mut state,
                                        active,
                                        target,
                                        PaneLoadPurpose::Navigate {
                                            remember_current: true,
                                        },
                                    );
                                    state.message = Some(format!("Connecting to {}…", host.name));
                                }
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // Jobs panel: Ctrl+J
                    if state.show_jobs {
                        match key.code {
                            KeyCode::Esc | KeyCode::Char('j')
                                if key.modifiers.contains(KeyModifiers::CONTROL) =>
                            {
                                state.show_jobs = false;
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                state.job_cursor = state.job_cursor.saturating_sub(1);
                            }
                            KeyCode::Down | KeyCode::Char('j')
                                if !key.modifiers.contains(KeyModifiers::CONTROL) =>
                            {
                                let max = state.jobs.len().saturating_sub(1);
                                if state.job_cursor < max {
                                    state.job_cursor += 1;
                                }
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // User menu: F2
                    if state.show_menu {
                        match key.code {
                            KeyCode::Esc | KeyCode::F(2) => {
                                state.show_menu = false;
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                state.menu_cursor = state.menu_cursor.saturating_sub(1);
                            }
                            KeyCode::Down | KeyCode::Char('j')
                                if !key.modifiers.contains(KeyModifiers::CONTROL) =>
                            {
                                let max = state.menu.len().saturating_sub(1);
                                if state.menu_cursor < max {
                                    state.menu_cursor += 1;
                                }
                            }
                            KeyCode::Enter => {
                                if let Some(entry) = state.menu.get(state.menu_cursor) {
                                    let cmd = entry.command.clone();
                                    state.close_all_overlays();
                                    let id = effect_dispatcher.dispatch(
                                        EffectLane::GlobalProcess,
                                        EffectScope::Global,
                                        Effect::RunShellCapture { command: cmd },
                                    );
                                    state.register_effect(EffectLane::GlobalProcess, id);
                                }
                            }
                            _ => {}
                        }
                        continue;
                    }

                    let entries = if state.active == Pane::Left {
                        &left_filtered
                    } else {
                        &right_filtered
                    };
                    let cursor = {
                        let pane = state.active_pane();
                        if pane.split && pane.split_active {
                            pane.split_cursor
                        } else {
                            pane.cursor
                        }
                    };

                    // First migration slice: resolve stable app actions before
                    // falling back to the legacy key matcher below.
                    match key_router.resolve(state.input_context(), key) {
                        KeyResolution::Pending => {
                            // PR #4 will render key_router.continuations()
                            // here as the Which-Key overlay.
                            continue;
                        }
                        KeyResolution::Action(action) => {
                            let context = ActionContext::from_state(&state).with_file_context(
                                entries.get(cursor).map(|entry| entry.kind),
                                editor.is_some(),
                            );
                            match action_availability(action.id(), &context) {
                                ActionAvailability::Available => {
                                    dispatch_ui_action(
                                        &mut state,
                                        action,
                                        entries.get(cursor).copied(),
                                        entries,
                                        entries.len(),
                                        &workspace_scanner,
                                        &sync_runtime,
                                        &effect_dispatcher,
                                        &pane_loader,
                                        terminal_session,
                                        editor.as_deref(),
                                    )
                                    .await?
                                }
                                ActionAvailability::Disabled { reason } => {
                                    state.message = Some(reason);
                                }
                                ActionAvailability::Hidden => {}
                            }
                            continue;
                        }
                        KeyResolution::Unhandled => {}
                    }

                    // Handle tree-filter Backspace before borrowing the active pane.
                    if key.code == KeyCode::Backspace && state.show_tree {
                        state.tree_filter.pop();
                        continue;
                    }

                    let pane = state.active_pane_mut();

                    match key.code {
                        KeyCode::Char('q') => request_quit(&mut state, &effect_dispatcher),
                        KeyCode::Tab => {
                            let pane = state.active_pane_mut();
                            if pane.split {
                                pane.split_active = !pane.split_active;
                            } else {
                                state.apply(Action::SwitchPane);
                            }
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            if pane.split && pane.split_active {
                                if pane.split_cursor > 0 {
                                    pane.split_cursor -= 1;
                                }
                            } else if cursor > 0 {
                                pane.cursor -= 1;
                            }
                        }
                        KeyCode::Down | KeyCode::Char('j')
                            if !key.modifiers.contains(KeyModifiers::CONTROL) =>
                        {
                            if pane.split && pane.split_active {
                                if pane.split_cursor + 1 < entries.len() {
                                    pane.split_cursor += 1;
                                }
                            } else if cursor + 1 < entries.len() {
                                pane.cursor += 1;
                            }
                        }
                        // Ctrl+Space: hash for files, du/df for dirs
                        KeyCode::Char(' ') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            if let Some(entry) = entries.get(cursor) {
                                match entry.kind {
                                    EntryKind::Directory => {
                                        if let Location::Local(dir) = &pane.location {
                                            let p = dir.join(&entry.name);
                                            state.viewer_content =
                                                FileInfoService::directory_summary(&p).await;
                                            state.viewer_scroll = 0;
                                        }
                                    }
                                    _ => {
                                        if let Location::Local(dir) = &pane.location {
                                            let p = dir.join(&entry.name);
                                            let size =
                                                entry.size.map(format_size).unwrap_or_default();
                                            state.viewer_content =
                                                FileInfoService::file_hash_summary(&p, &size).await;
                                            state.viewer_scroll = 0;
                                        }
                                    }
                                }
                            }
                        }
                        // Alt+/ : recursive file search
                        KeyCode::Char('/') if key.modifiers.contains(KeyModifiers::ALT) => {
                            if let Location::Local(dir) = &state.active_pane().location {
                                state.cmd = format!("find {} -name ''", dir.display());
                                state.cmd_input = true;
                            }
                        }
                        KeyCode::Char('/') => {
                            state.filter.clear();
                            state.filtering = true;
                        }
                        KeyCode::Char('?') => {
                            state.show_help = !state.show_help;
                        }
                        KeyCode::Enter
                            if key.modifiers.contains(KeyModifiers::ALT)
                                && key.modifiers.contains(KeyModifiers::SHIFT) =>
                        {
                            if let Location::Local(dir) = &pane.location {
                                let location = Location::Local(dir.clone());
                                let id = effect_dispatcher.dispatch(
                                    EffectLane::Preview,
                                    EffectScope::Location(location),
                                    Effect::DirectoryChildrenSizes { path: dir.clone() },
                                );
                                state.register_effect(EffectLane::Preview, id);
                                state.message = Some("Calculating directory sizes…".into());
                            }
                        }
                        KeyCode::Enter => {
                            if let Some(entry) = entries.get(cursor) {
                                if let Some(new_location) =
                                    directory_navigation_target(&pane.location, entry)
                                {
                                    let active = state.active;
                                    schedule_pane_navigation(
                                        &pane_loader,
                                        &mut state,
                                        active,
                                        new_location,
                                        PaneLoadPurpose::Navigate {
                                            remember_current: entry.name != VIRTUAL_PARENT_NAME,
                                        },
                                    );
                                    state.message = Some("Opening directory…".into());
                                } else if is_archive(&entry.name) {
                                    // Open archive file
                                    if let Location::Local(dir) = &pane.location {
                                        let archive_path = dir.join(&entry.name);
                                        let target = Location::Archive {
                                            archive: archive_path,
                                            inner_path: String::new(),
                                        };
                                        let active = state.active;
                                        schedule_pane_navigation(
                                            &pane_loader,
                                            &mut state,
                                            active,
                                            target,
                                            PaneLoadPurpose::Navigate {
                                                remember_current: true,
                                            },
                                        );
                                        state.message = Some("Opening archive…".into());
                                    }
                                } else if state.show_diff {
                                    // Content diff: diff this file against other pane's same-named file
                                    if let (Location::Local(left_dir), Location::Local(right_dir)) =
                                        (&state.left.location, &state.right.location)
                                    {
                                        let left_path = left_dir.join(&entry.name);
                                        let right_path = right_dir.join(&entry.name);
                                        let scope = EffectScope::Workspace {
                                            left: state.left.location.clone(),
                                            right: state.right.location.clone(),
                                        };
                                        let id = effect_dispatcher.dispatch(
                                            EffectLane::Preview,
                                            scope,
                                            Effect::UnifiedDiff {
                                                left: left_path,
                                                right: right_path,
                                            },
                                        );
                                        state.register_effect(EffectLane::Preview, id);
                                        state.message = Some("Building diff…".into());
                                    }
                                }
                            }
                        }
                        KeyCode::Backspace => {
                            if let Some(new_loc) = pane.location.parent() {
                                let active = state.active;
                                schedule_pane_navigation(
                                    &pane_loader,
                                    &mut state,
                                    active,
                                    new_loc,
                                    PaneLoadPurpose::Navigate {
                                        remember_current: false,
                                    },
                                );
                                state.message = Some("Opening parent…".into());
                            }
                        }
                        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            schedule_both_pane_loads(&pane_loader, &mut state);
                            state.message = Some("Refreshing panes…".into());
                        }
                        // Ctrl+U: swap panes
                        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            std::mem::swap(&mut state.left, &mut state.right);
                            std::mem::swap(&mut left_entries, &mut right_entries);
                            state.clear_selection();
                            state.remote_workspace.disable();
                            state.show_diff = false;
                            state.message = Some("Swapped".into());
                            schedule_both_pane_loads(&pane_loader, &mut state);
                        }
                        // Shift+F6: rename file under cursor
                        KeyCode::F(6) if key.modifiers.contains(KeyModifiers::SHIFT) => {
                            if let Some(entry) = entries
                                .get(cursor)
                                .filter(|entry| entry.name != VIRTUAL_PARENT_NAME)
                            {
                                state.cmd = format!("mv '{}' ", entry.name);
                                state.cmd_input = true;
                            }
                        }
                        // Ctrl+A: file attributes (permissions/owner)
                        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            if let Some(entry) = entries.get(cursor) {
                                if let Location::Local(dir) = &pane.location {
                                    let p = dir.join(&entry.name);
                                    let size = entry.size.map(format_size).unwrap_or_default();
                                    state.viewer_content = FileInfoService::metadata_summary(
                                        &p,
                                        &entry.name,
                                        entry.kind,
                                        &size,
                                    )
                                    .await
                                    .unwrap_or_else(|error| {
                                        vec![format!("File info failed: {error}")]
                                    });
                                    state.viewer_scroll = 0;
                                }
                            }
                        }
                        // Ctrl+I: file info (stat)
                        KeyCode::Char('i') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            if let Some(entry) = entries.get(cursor) {
                                if let Location::Local(dir) = &pane.location {
                                    let path = dir.join(&entry.name);
                                    let size = entry.size.map(format_size).unwrap_or_default();
                                    state.viewer_content = FileInfoService::metadata_summary(
                                        &path,
                                        &entry.name,
                                        entry.kind,
                                        &size,
                                    )
                                    .await
                                    .unwrap_or_else(|error| {
                                        vec![format!("File info failed: {error}")]
                                    });
                                    state.viewer_scroll = 0;
                                }
                            }
                        }
                        // Alt+O: sync other pane to active pane
                        KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::ALT) => {
                            let src = state.active_pane().location.clone();
                            let destination_pane = match state.active {
                                Pane::Left => Pane::Right,
                                Pane::Right => Pane::Left,
                            };
                            let dst = state.other_pane_mut();
                            dst.location = src;
                            dst.cursor = 0;
                            state.remote_workspace.disable();
                            state.show_diff = false;
                            state.message = Some("Directory synced".into());
                            schedule_pane_load(&pane_loader, &mut state, destination_pane);
                        }
                        // Alt+Down: go back in directory history
                        KeyCode::Down if key.modifiers.contains(KeyModifiers::ALT) => {
                            let pane = state.active_pane_mut();
                            if let Some(prev) = pane.dir_history.last().cloned() {
                                let active = state.active;
                                schedule_pane_navigation(
                                    &pane_loader,
                                    &mut state,
                                    active,
                                    prev,
                                    PaneLoadPurpose::HistoryBack,
                                );
                                state.message = Some("History back…".into());
                            }
                        }
                        // Ctrl+\\: open active directory in file explorer
                        KeyCode::Char('\\') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            if let Location::Local(dir) = &state.active_pane().location {
                                let dir_c = dir.clone();
                                let id = effect_dispatcher.dispatch(
                                    EffectLane::GlobalProcess,
                                    EffectScope::Location(Location::Local(dir_c.clone())),
                                    Effect::OpenPath {
                                        path: dir_c.clone(),
                                    },
                                );
                                state.register_effect(EffectLane::GlobalProcess, id);
                                state.message = Some(format!("Opening {}", dir_c.display()));
                                state.dir_history.push(dir_c);
                                if state.dir_history.len() > 20 {
                                    state.dir_history.remove(0);
                                }
                            }
                        }
                        // Ctrl+Shift+Left/Right: resize panel ratio
                        KeyCode::Left
                            if key.modifiers.contains(KeyModifiers::CONTROL)
                                && key.modifiers.contains(KeyModifiers::SHIFT) =>
                        {
                            state.panel_ratio = state.panel_ratio.saturating_sub(5).max(10);
                            state.message = Some(format!(
                                "Panel: {}/{}",
                                state.panel_ratio,
                                100 - state.panel_ratio
                            ));
                        }
                        KeyCode::Right
                            if key.modifiers.contains(KeyModifiers::CONTROL)
                                && key.modifiers.contains(KeyModifiers::SHIFT) =>
                        {
                            state.panel_ratio = (state.panel_ratio + 5).min(90);
                            state.message = Some(format!(
                                "Panel: {}/{}",
                                state.panel_ratio,
                                100 - state.panel_ratio
                            ));
                        }
                        // Alt+`: tab switcher
                        KeyCode::Char('`') if key.modifiers.contains(KeyModifiers::ALT) => {
                            state.show_tab_switcher = !state.show_tab_switcher;
                            state.tab_switcher_cursor = 0;
                        }
                        // Alt+H: directory history
                        KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::ALT) => {
                            state.show_history = !state.show_history;
                        }
                        // Ctrl+\: hotlist
                        KeyCode::Char('\\') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            state.show_hotlist = !state.show_hotlist;
                            state.hotlist_cursor = 0;
                        }
                        // Ctrl+O: drop to subshell, restore on exit
                        KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
                            let shell_result = terminal_session
                                .suspend_while(|| DesktopService::run_interactive_shell(&shell))
                                .await?;
                            if let Err(error) = shell_result {
                                state.message = Some(format!("Shell failed: {error}"));
                            }
                        }
                        KeyCode::Char('g') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            state.filter.clear();
                            state.go_input = true;
                        }
                        KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            state.show_hidden = !state.show_hidden;
                            state.message = Some(if state.show_hidden {
                                "Hidden files shown".into()
                            } else {
                                "Hidden files hidden".into()
                            });
                            schedule_both_pane_loads(&pane_loader, &mut state);
                        }
                        // *: invert selection on visible entries
                        KeyCode::Char('*') => {
                            let filt = if state.active == Pane::Left {
                                &left_filtered
                            } else {
                                &right_filtered
                            };
                            let active = state.active;
                            let location = state.active_pane().location.clone();
                            for e in filt {
                                if e.name != VIRTUAL_PARENT_NAME {
                                    state.toggle_selection(active, &location, &e.name);
                                }
                            }
                            state.message = Some(format!(
                                "Selected {}",
                                state.selection_count(active, &location)
                            ));
                        }
                        // +: enter glob-select mode (uses filter buffer)
                        KeyCode::Char('+') => {
                            state.filter.clear();
                            state.glob_input = true;
                        }
                        // F2: user menu (if loaded), otherwise cycle sort
                        KeyCode::F(2) => {
                            if !state.menu.is_empty() {
                                state.show_menu = !state.show_menu;
                                state.menu_cursor = 0;
                            } else {
                                state.sort_mode = state.sort_mode.next();
                                state.message = Some(format!("Sort: {}", state.sort_mode.label()));
                                sort_entries(&mut left_entries, state.sort_mode);
                                sort_entries(&mut right_entries, state.sort_mode);
                            }
                        }
                        // Shift+F3: page file with bat
                        KeyCode::F(3) if key.modifiers.contains(KeyModifiers::SHIFT) => {
                            if let Some(entry) = entries.get(cursor) {
                                if entry.kind != EntryKind::Directory {
                                    let path = match &pane.location {
                                        Location::Local(dir) => dir.join(&entry.name),
                                        _ => continue,
                                    };
                                    let _ = DesktopService::page_with_bat(&path).await;
                                }
                            }
                        }
                        // Ctrl+C: copy filename to clipboard
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            if let Some(entry) = entries.get(cursor) {
                                let name = &entry.name;
                                if let Location::Local(dir) = &pane.location {
                                    let full = dir.join(name);
                                    let path = full.to_string_lossy().into_owned();
                                    if let Err(error) =
                                        DesktopService::copy_to_clipboard(&path).await
                                    {
                                        state.message = Some(format!("Clipboard failed: {error}"));
                                        continue;
                                    }
                                }
                                state.message = Some(format!("Copied: {name}"));
                            }
                        }
                        // Ctrl+B: bookmarks
                        KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            state.show_bookmarks = !state.show_bookmarks;
                            state.bookmark_cursor = 0;
                        }
                        // Ctrl+J: jobs panel
                        KeyCode::Delete if state.show_jobs => {
                            if let Some(job) = state.jobs.get(state.job_cursor) {
                                let id = job.id.clone();
                                if job_manager.cancel(&id) {
                                    state.jobs = job_manager.snapshot();
                                }
                            }
                        }
                        KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            state.show_jobs = !state.show_jobs;
                            state.job_cursor = 0;
                        }
                        // Ctrl+I: toggle Infrastructure Center
                        // Ctrl+S: save workspace
                        KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            match crate::workspace::save_workspace(&state) {
                                Ok(()) => state.message = Some("Workspace saved".into()),
                                Err(e) => state.message = Some(format!("Save failed: {e}")),
                            }
                        }
                        KeyCode::Char('i') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            let opening =
                                state.active_overlay() != Some(OverlayKind::Infrastructure);
                            state.toggle_overlay(OverlayKind::Infrastructure);
                            if opening {
                                state.infrastructure_lines = vec!["Checking SSH hosts…".into()];
                                let id = effect_dispatcher.dispatch(
                                    EffectLane::Infrastructure,
                                    EffectScope::Global,
                                    Effect::InfrastructureSnapshot,
                                );
                                state.register_effect(EffectLane::Infrastructure, id);
                            }
                        }
                        // Ctrl+T: toggle Smart Tree
                        KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            let opening = state.active_overlay() != Some(OverlayKind::Tree);
                            state.toggle_overlay(OverlayKind::Tree);
                            state.tree_filter.clear();
                            if opening {
                                let location = state.active_pane().location.clone();
                                state.tree_lines = vec!["Loading tree…".into()];
                                let id = effect_dispatcher.dispatch(
                                    EffectLane::Tree,
                                    EffectScope::Location(location.clone()),
                                    Effect::TreeSnapshot {
                                        location,
                                        filter: String::new(),
                                    },
                                );
                                state.register_effect(EffectLane::Tree, id);
                            }
                        }
                        // Type in tree filter (when tree is shown) — Esc to close
                        KeyCode::Esc if state.show_tree => {
                            state.show_tree = false;
                            state.tree_filter.clear();
                        }
                        KeyCode::Char(c)
                            if state.show_tree
                                && !key.modifiers.contains(KeyModifiers::CONTROL) =>
                        {
                            state.tree_filter.push(c);
                            let location = state.active_pane().location.clone();
                            let id = effect_dispatcher.dispatch(
                                EffectLane::Tree,
                                EffectScope::Location(location.clone()),
                                Effect::TreeSnapshot {
                                    location,
                                    filter: state.tree_filter.clone(),
                                },
                            );
                            state.register_effect(EffectLane::Tree, id);
                        }
                        // Ctrl+X D: toggle directory compare
                        // Alt+T: toggle panel mode (Full ↔ Brief) KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::ALT) => { state.panel_mode = match state.panel_mode { PanelMode::Full => PanelMode::Brief, PanelMode::Brief => PanelMode::Full, }; }
                        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            state.show_diff = !state.show_diff;
                            state.message = Some(if state.show_diff {
                                "Diff on — unique files highlighted".into()
                            } else {
                                "Diff off".into()
                            });
                        }
                        // :: command input
                        KeyCode::Char(':') => {
                            state.cmd.clear();
                            state.cmd_input = true;
                        }
                        KeyCode::Char('\\') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            let pane = state.active_pane_mut();
                            pane.split = !pane.split;
                            if pane.split {
                                pane.split_cursor = pane.cursor;
                            }
                            state.message = Some(format!(
                                "Split {}",
                                if pane.split {
                                    "ON (Tab toggles)"
                                } else {
                                    "OFF"
                                }
                            ));
                        }
                        // Alt+1-9: switch to tab N
                        KeyCode::Char(c)
                            if key.modifiers.contains(KeyModifiers::ALT)
                                && ('1'..='9').contains(&c) =>
                        {
                            let idx = (c as u8 - b'1') as usize;
                            let pane = state.active_pane_mut();
                            if idx < pane.tabs.len() + 1 {
                                if idx != 0 {
                                    // ponytail: swap current tab (implicit idx 0) with target tab
                                    // Current is at position 1..N; saved tabs are at 0..N-1; total N+1 entries.
                                    // To go to tab N: if N==0 (current), no-op; else swap current with saved[idx-1]
                                    pane.switch_tab(idx - 1);
                                }
                                let n = pane.tabs.len() + 1;
                                state.message = Some(format!("Tab {}/{n}", idx + 1));
                                state.clear_selection();
                                state.remote_workspace.disable();
                                state.show_diff = false;
                                schedule_active_pane_load(&pane_loader, &mut state);
                            }
                        }
                        // Ctrl+T: new tab in active pane
                        KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            state.active_pane_mut().new_tab();
                            let tabs = state.active_pane().tabs.len() + 1;
                            state.clear_selection();
                            state.remote_workspace.disable();
                            state.show_diff = false;
                            schedule_active_pane_load(&pane_loader, &mut state);
                            state.message = Some(format!("Tab {tabs}/{tabs}"));
                        }
                        // Ctrl+W: close tab in active pane
                        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            state.active_pane_mut().close_tab();
                            let tabs = state.active_pane().tabs.len() + 1;
                            state.clear_selection();
                            state.remote_workspace.disable();
                            state.show_diff = false;
                            schedule_active_pane_load(&pane_loader, &mut state);
                            state.message = Some(format!("Tab {}/{}", tabs.min(1), tabs));
                        }
                        // Ctrl+Left: previous tab
                        KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            let tabs_len = state.active_pane().tabs.len();
                            if tabs_len > 0 {
                                state.active_pane_mut().switch_tab(tabs_len - 1);
                                state.clear_selection();
                                state.remote_workspace.disable();
                                state.show_diff = false;
                                schedule_active_pane_load(&pane_loader, &mut state);
                                state.message = Some("Tab ←".into());
                            }
                        }
                        // Ctrl+Right: next tab
                        KeyCode::Right
                            if key.modifiers.contains(KeyModifiers::CONTROL)
                                && state.active_pane().tabs.len() >= 2 =>
                        {
                            state.active_pane_mut().switch_tab(0);
                            state.clear_selection();
                            state.remote_workspace.disable();
                            state.show_diff = false;
                            schedule_active_pane_load(&pane_loader, &mut state);
                            state.message = Some("Tab →".into());
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }

        // ── Deferred editor launch: only after terminal input is drained ──
        if state.pending_editor && !state.should_quit {
            state.pending_editor = false;
            if let Some(mut session) = state.pending_remote_edit_session.take() {
                let origin_matches =
                    state
                        .pending_remote_edit_origin
                        .as_ref()
                        .is_some_and(|(pane, location)| {
                            location == &session.location
                                && pane_still_at_location(&state, *pane, location)
                        });
                if session.state != RemoteEditState::ReadyToEdit {
                    state.pending_remote_edit_origin = None;
                    state.message = Some("Remote edit session invalid".into());
                    continue;
                }
                if !origin_matches {
                    state.pending_remote_edit_origin = None;
                    state.message =
                        Some("Remote edit cancelled: originating pane navigated away".into());
                    continue;
                }

                let editor_cmd = if let Some(cfg_editor) = &editor {
                    cfg_editor.clone()
                } else {
                    session.editor.clone()
                };
                let working_path = session.temp_dir.path().join("working");
                session.state = RemoteEditState::Editing;
                let editor_result = terminal_session
                    .suspend_while(|| DesktopService::open_editor(&editor_cmd, &working_path))
                    .await?;
                if let Some(effect) = finish_remote_editor(session, editor_result, &mut state) {
                    let location = match &effect {
                        Effect::WriteBackRemoteFile { session } => session.location.clone(),
                        _ => unreachable!("remote editor can only schedule write-back"),
                    };
                    let id = effect_dispatcher.dispatch(
                        EffectLane::RemoteEdit,
                        EffectScope::Location(location),
                        effect,
                    );
                    state.register_effect(EffectLane::RemoteEdit, id);
                }
            }
        }
    }
    Ok(())
}

fn normalize_entries(
    mut entries: Vec<Entry>,
    show_hidden: bool,
    sort_mode: SortMode,
) -> Vec<Entry> {
    if !show_hidden {
        entries.retain(|e| !e.name.starts_with('.'));
    }
    sort_entries(&mut entries, sort_mode);
    entries
}

fn schedule_pane_load(loader: &PaneLoader, state: &mut AppState, pane: Pane) {
    let location = match pane {
        Pane::Left => state.left.location.clone(),
        Pane::Right => state.right.location.clone(),
    };
    let id = loader.load(pane, location.clone(), PaneLoadPurpose::Refresh);
    state.register_pane_load(pane, id, location, PaneLoadPurpose::Refresh);
}

fn schedule_pane_navigation(
    loader: &PaneLoader,
    state: &mut AppState,
    pane: Pane,
    location: Location,
    purpose: PaneLoadPurpose,
) {
    let id = loader.load(pane, location.clone(), purpose);
    state.register_pane_load(pane, id, location, purpose);
}

fn schedule_active_pane_load(loader: &PaneLoader, state: &mut AppState) {
    schedule_pane_load(loader, state, state.active);
}

fn schedule_both_pane_loads(loader: &PaneLoader, state: &mut AppState) {
    schedule_pane_load(loader, state, Pane::Left);
    schedule_pane_load(loader, state, Pane::Right);
}

fn apply_pane_load_response(
    response: PaneLoadResponse,
    state: &mut AppState,
    left_entries: &mut Vec<Entry>,
    right_entries: &mut Vec<Entry>,
) {
    if !state.accepts_pane_load(response.pane, response.id, &response.location) {
        return;
    }
    state.finish_pane_load(response.pane, response.id);

    match response.result {
        Ok(entries) => {
            state.pane_load_errors.remove(&response.pane);
            let entries = normalize_entries(entries, state.show_hidden, state.sort_mode);
            let active = state.active == response.pane;
            match response.pane {
                Pane::Left => {
                    if response.purpose != PaneLoadPurpose::Refresh {
                        let old = state.left.location.clone();
                        match response.purpose {
                            PaneLoadPurpose::Navigate {
                                remember_current: true,
                            } => state.left.dir_history.push(old),
                            PaneLoadPurpose::HistoryBack => {
                                let _ = state.left.dir_history.pop();
                            }
                            _ => {}
                        }
                        state.left.location = response.location.clone();
                        state.left.cursor = 0;
                    }
                    *left_entries = entries;
                    state.left.cursor = state.left.cursor.min(left_entries.len().saturating_sub(1));
                }
                Pane::Right => {
                    if response.purpose != PaneLoadPurpose::Refresh {
                        let old = state.right.location.clone();
                        match response.purpose {
                            PaneLoadPurpose::Navigate {
                                remember_current: true,
                            } => state.right.dir_history.push(old),
                            PaneLoadPurpose::HistoryBack => {
                                let _ = state.right.dir_history.pop();
                            }
                            _ => {}
                        }
                        state.right.location = response.location.clone();
                        state.right.cursor = 0;
                    }
                    *right_entries = entries;
                    state.right.cursor = state
                        .right
                        .cursor
                        .min(right_entries.len().saturating_sub(1));
                }
            }
            if response.purpose != PaneLoadPurpose::Refresh {
                state.clear_selection_for_pane(response.pane);
            }
            if active && response.purpose != PaneLoadPurpose::Refresh {
                state.remote_workspace.disable();
                state.show_diff = false;
            }
        }
        Err(error) => {
            // Transactional navigation: current pane location is intentionally
            // untouched on error. Persist the accepted failure so the pane can
            // explain what failed after the one-shot status message is gone.
            let message = error.to_string();
            state.pane_load_errors.insert(
                response.pane,
                PaneLoadUiError {
                    attempted: response.location.clone(),
                    message: message.clone(),
                },
            );
            state.message = Some(format!("{}: {message}", response.location));
        }
    }
}

fn sort_entries(entries: &mut [Entry], mode: SortMode) {
    match mode {
        SortMode::NameAsc => entries.sort_by_key(|a| a.name.to_lowercase()),
        SortMode::NameDesc => entries.sort_by_key(|b| std::cmp::Reverse(b.name.to_lowercase())),
        SortMode::SizeAsc => entries.sort_by_key(|a| a.size.unwrap_or(0)),
        SortMode::SizeDesc => entries.sort_by_key(|b| std::cmp::Reverse(b.size.unwrap_or(0))),
        SortMode::Kind => entries.sort_by_key(|a| (kind_order(a.kind), a.name.to_lowercase())),
    }
}

fn kind_order(k: EntryKind) -> u8 {
    match k {
        EntryKind::Directory => 0,
        EntryKind::Symlink => 1,
        EntryKind::File => 2,
        EntryKind::Other => 3,
    }
}

fn apply_filter<'a>(entries: &'a [Entry], filter: &str) -> Vec<&'a Entry> {
    if filter.is_empty() {
        entries.iter().collect()
    } else {
        let (name_filter, size_min, size_max) = parse_filter(filter);
        entries
            .iter()
            .filter(|e| {
                if !name_filter.is_empty() && !e.name.to_lowercase().contains(&name_filter) {
                    return false;
                }
                match (size_min, size_max) {
                    (Some(min), Some(max)) => e.size.is_some_and(|s| s >= min && s <= max),
                    (Some(min), None) => e.size.is_some_and(|s| s >= min),
                    (None, Some(max)) => e.size.is_some_and(|s| s <= max),
                    (None, None) => true,
                }
            })
            .collect()
    }
}

const VIRTUAL_PARENT_NAME: &str = "..";

fn virtual_parent_entry() -> Entry {
    Entry {
        name: VIRTUAL_PARENT_NAME.into(),
        kind: EntryKind::Directory,
        size: None,
        modified_unix_ms: None,
    }
}

fn apply_filter_with_parent<'a>(
    entries: &'a [Entry],
    filter: &str,
    location: &Location,
    parent_entry: &'a Entry,
) -> Vec<&'a Entry> {
    let mut visible = apply_filter(entries, filter);
    visible.retain(|entry| entry.name != VIRTUAL_PARENT_NAME);
    if location.parent().is_some() {
        visible.insert(0, parent_entry);
    }
    visible
}

fn directory_navigation_target(location: &Location, entry: &Entry) -> Option<Location> {
    if entry.name == VIRTUAL_PARENT_NAME {
        location.parent()
    } else if entry.kind == EntryKind::Directory {
        Some(location.child(&entry.name))
    } else {
        None
    }
}

fn focused_entry<'a>(
    state: &AppState,
    left_entries: &[&'a Entry],
    right_entries: &[&'a Entry],
) -> Option<&'a Entry> {
    let pane = state.active_pane();
    let cursor = if pane.split && pane.split_active {
        pane.split_cursor
    } else {
        pane.cursor
    };
    match state.active {
        Pane::Left => left_entries.get(cursor).copied(),
        Pane::Right => right_entries.get(cursor).copied(),
    }
}

fn toggle_selection_and_advance(
    state: &mut AppState,
    focused: Option<&Entry>,
    visible_count: usize,
) {
    let Some(entry) = focused.filter(|entry| entry.name != VIRTUAL_PARENT_NAME) else {
        return;
    };
    let active = state.active;
    let location = state.active_pane().location.clone();
    state.toggle_selection(active, &location, &entry.name);

    let pane = state.active_pane_mut();
    let cursor = if pane.split && pane.split_active {
        &mut pane.split_cursor
    } else {
        &mut pane.cursor
    };
    if cursor.saturating_add(1) < visible_count {
        *cursor += 1;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaneSurfaceState<'a> {
    Loading {
        target: &'a Location,
        transactional: bool,
    },
    LoadError {
        attempted: &'a Location,
        message: &'a str,
    },
    Empty,
    NoMatches {
        filter: &'a str,
    },
    Entries,
}

fn pane_surface_state<'a>(
    state: &'a AppState,
    pane: Pane,
    total_entries: usize,
    visible_entries: usize,
) -> PaneSurfaceState<'a> {
    if let Some((target, purpose)) = state.pending_pane_targets.get(&pane) {
        let transactional = *purpose != PaneLoadPurpose::Refresh;
        if transactional || total_entries == 0 {
            return PaneSurfaceState::Loading {
                target,
                transactional,
            };
        }
    }

    if let Some(error) = state.pane_load_errors.get(&pane) {
        return PaneSurfaceState::LoadError {
            attempted: &error.attempted,
            message: &error.message,
        };
    }

    if visible_entries == 0 {
        if total_entries > 0 && !state.filter.is_empty() {
            PaneSurfaceState::NoMatches {
                filter: &state.filter,
            }
        } else {
            PaneSurfaceState::Empty
        }
    } else {
        PaneSurfaceState::Entries
    }
}

// --- Commander core action helpers (ponytail: 7-variant match, covers hitbox set) ---

fn action_id_to_action(id: ActionId) -> Option<Action> {
    Some(match id {
        ActionId::ViewFile => Action::ViewFile,
        ActionId::EditFile => Action::EditFile,
        ActionId::Copy => Action::Copy,
        ActionId::Move => Action::Move,
        ActionId::Mkdir => Action::Mkdir,
        ActionId::Delete => Action::Delete,
        ActionId::OpenHosts => Action::OpenHosts,
        _ => return None,
    })
}

fn action_to_id(a: Action) -> ActionId {
    match a {
        Action::ViewFile => ActionId::ViewFile,
        Action::EditFile => ActionId::EditFile,
        Action::Copy => ActionId::Copy,
        Action::Move => ActionId::Move,
        Action::Mkdir => ActionId::Mkdir,
        Action::Delete => ActionId::Delete,
        Action::OpenHosts => ActionId::OpenHosts,
        _ => unreachable!("only commander core actions reach hitboxes"),
    }
}

fn compact_action_label(action: ActionId) -> &'static str {
    match action {
        ActionId::ViewFile => "View",
        ActionId::EditFile => "Edit",
        ActionId::Copy => "Copy",
        ActionId::Move => "Move",
        ActionId::Mkdir => "MkDir",
        ActionId::Delete => "Del",
        ActionId::OpenHosts => "Hosts",
        _ => "",
    }
}

/// Format one command-bar row from hints, respecting width.
fn format_command_row(hints: &[ContextHint], width: u16) -> String {
    let mut text = String::new();
    for hint in hints {
        let item = format!("{} {}", hint.binding, hint.label);
        let candidate = if text.is_empty() {
            item
        } else {
            format!("{text}    {item}")
        };
        if Line::from(candidate.as_str()).width() > usize::from(width) {
            break;
        }
        text = candidate;
    }
    text
}

fn render_command_bar(
    frame: &mut ratatui::Frame,
    row_a_area: Rect,
    row_b_area: Rect,
    hitboxes: &mut Vec<arx::app::CommandHitbox>,
    row_a: &[ContextHint],
    row_b: &[ContextHint],
) {
    hitboxes.clear();

    // Row A — Commander core (always visible, dimmed if unavailable).
    if !row_a.is_empty() && row_a_area.width > 0 {
        let mut spans: Vec<Span> = Vec::new();
        let compact = row_a_area.width < 90;
        let mut col = row_a_area.x;
        for (i, hint) in row_a.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw("  "));
                col += 2;
            }
            let label = if compact {
                compact_action_label(hint.action)
            } else {
                hint.label
            };
            let chip_text = format!("{} {}", hint.binding, label);
            let chip_width = chip_text.len() as u16;
            if let Some(action) = action_id_to_action(hint.action) {
                hitboxes.push(arx::app::CommandHitbox {
                    row: 0,
                    col,
                    width: chip_width,
                    action,
                    available: hint.available,
                });
            }
            col += chip_width;
            let style = if !hint.available {
                Style::default()
                    .fg(Color::Gray)
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::DIM)
            } else {
                Style::default().fg(Color::Black).bg(Color::DarkGray)
            };
            spans.push(Span::styled(chip_text, style));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), row_a_area);
    }

    // Row B — Discovery (responsive, priority-based).
    if !row_b.is_empty() && row_b_area.width > 0 {
        let text = format_command_row(row_b, row_b_area.width);
        if !text.is_empty() {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    text,
                    Style::default().fg(Color::Black).bg(Color::DarkGray),
                )),
                row_b_area,
            );
        }
    }
}

/// Test-only wrapper — resolves the old `contextual_footer_text` signature
/// using the new `command_bar_rows` + `format_command_row` machinery.
#[cfg(test)]
fn command_bar_text_wrapper(
    state: &AppState,
    key_router: &KeyRouter,
    focused_kind: Option<EntryKind>,
    editor_available: bool,
    width: u16,
) -> Option<String> {
    if !key_router.pending().is_empty() || width == 0 {
        return None;
    }
    let (row_a, row_b) =
        command_bar_rows(state, key_router.keymap(), focused_kind, editor_available);
    // If row_a has content (browser), return it. Otherwise return row_b.
    let mut text = format_command_row(&row_a, width);
    if text.is_empty() {
        text = format_command_row(&row_b, width);
    }
    // If both rows are empty, fall back to raw contextual hints for non-browser contexts
    if text.is_empty() {
        let hints = contextual_hints_with_file_context(
            state,
            key_router.keymap(),
            focused_kind,
            editor_available,
        );
        text = format_command_row(&hints, width);
    }
    (!text.is_empty()).then_some(text)
}

fn session_callout_text(state: &AppState, key_router: &KeyRouter) -> Option<String> {
    let callout = state.session_callout.as_ref()?;
    if session_callout_is_embedded(state, callout) {
        return None;
    }

    match callout {
        SessionCallout::CompareCompleted {
            differences,
            bytes_to_transfer,
        } => {
            if *differences == 0 {
                return Some("✓ Workspace compared · No proven differences found.".into());
            }

            let changes = if *differences == 1 {
                "1 change found".to_string()
            } else {
                format!("{differences} changes found")
            };
            let transfer = if *bytes_to_transfer > 0 {
                format!(" · {} planned", format_size(*bytes_to_transfer))
            } else {
                String::new()
            };
            let next = if state.remote_workspace.preview_open {
                String::new()
            } else {
                contextual_hints(state, key_router.keymap())
                    .into_iter()
                    .find(|hint| hint.action == Action::PreviewWorkspaceSync.id())
                    .map(|hint| format!(" · {} {}", hint.binding, hint.label))
                    .unwrap_or_default()
            };
            Some(format!("✓ Workspace compared · {changes}{transfer}{next}"))
        }
        SessionCallout::WorkspaceSyncVerified { .. } => Some(
            "✓ First workspace sync verified this session · Both workspace roots are synchronized."
                .into(),
        ),
    }
}

fn session_callout_is_embedded(state: &AppState, callout: &SessionCallout) -> bool {
    let SessionCallout::WorkspaceSyncVerified { job_id } = callout else {
        return false;
    };
    state.active_overlay() == Some(OverlayKind::SyncPreview)
        && state.remote_workspace.ux.job_id() == Some(job_id.as_str())
}

fn render_session_callout(frame: &mut ratatui::Frame, area: Rect, text: &str) {
    frame.render_widget(
        Paragraph::new(Span::styled(
            text.to_string(),
            Style::default().fg(Color::Green),
        )),
        area,
    );
}

/// Check if a filename looks like an archive (tar, tgz, zip).
fn is_archive(name: &str) -> bool {
    name.ends_with(".tar")
        || name.ends_with(".tar.gz")
        || name.ends_with(".tgz")
        || name.ends_with(".tar.bz2")
        || name.ends_with(".tar.xz")
        || name.ends_with(".zip")
}

#[allow(clippy::too_many_arguments)]
fn render(
    frame: &mut ratatui::Frame,
    state: &mut AppState,
    left_total_entries: usize,
    right_total_entries: usize,
    left_entries: &[&Entry],
    right_entries: &[&Entry],
    left_list: &mut ListState,
    right_list: &mut ListState,
    split_left_list: &mut ListState,
    split_right_list: &mut ListState,
    key_router: &KeyRouter,
    editor_available: bool,
    message: Option<&str>,
) {
    let area = frame.area();
    let session_callout = session_callout_text(state, key_router);
    let constraints = if session_callout.is_some() {
        vec![
            Constraint::Min(1),
            Constraint::Length(1),  // workspace ribbon
            Constraint::Length(1),  // status line
            Constraint::Length(1),  // session callout
            Constraint::Length(1),  // Row A
            Constraint::Length(1),  // Row B
        ]
    } else {
        vec![
            Constraint::Min(1),
            Constraint::Length(1),  // workspace ribbon
            Constraint::Length(1),  // status line
            Constraint::Length(1),  // Row A
            Constraint::Length(1),  // Row B
        ]
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Ratio(state.panel_ratio as u32, 100),
            Constraint::Ratio(100 - state.panel_ratio as u32, 100),
        ])
        .split(chunks[0]);

    state.left_area = Some(panes[0]);
    state.right_area = Some(panes[1]);

    // Diff sets: files unique to each pane (only computed when show_diff is on)
    let (left_only, right_only): (
        std::collections::BTreeSet<&str>,
        std::collections::BTreeSet<&str>,
    ) = if state.show_diff {
        let left_names: std::collections::BTreeSet<&str> =
            left_entries.iter().map(|e| e.name.as_str()).collect();
        let right_names: std::collections::BTreeSet<&str> =
            right_entries.iter().map(|e| e.name.as_str()).collect();
        let lo: std::collections::BTreeSet<&str> =
            left_names.difference(&right_names).copied().collect();
        let ro: std::collections::BTreeSet<&str> =
            right_names.difference(&left_names).copied().collect();
        (lo, ro)
    } else {
        (Default::default(), Default::default())
    };

    let empty_selection = std::collections::BTreeSet::new();
    let left_selection = state
        .selection_names(Pane::Left, &state.left.location)
        .unwrap_or(&empty_selection);
    let right_selection = state
        .selection_names(Pane::Right, &state.right.location)
        .unwrap_or(&empty_selection);

    if state.left.split {
        let mid = panes[0].width / 2;
        let a1 = ratatui::layout::Rect::new(panes[0].x, panes[0].y, mid, panes[0].height);
        let a2 = ratatui::layout::Rect::new(
            panes[0].x + mid,
            panes[0].y,
            panes[0].width - mid,
            panes[0].height,
        );
        let act1 = state.active == Pane::Left && !state.left.split_active;
        let act2 = state.active == Pane::Left && state.left.split_active;
        left_list.select(Some(state.left.cursor));
        split_left_list.select(Some(state.left.split_cursor));
        render_pane(
            frame,
            a1,
            &state.left,
            left_entries,
            pane_surface_state(state, Pane::Left, left_total_entries, left_entries.len()),
            left_list,
            act1,
            left_selection,
            &left_only,
            state.panel_mode,
        );
        render_pane(
            frame,
            a2,
            &state.left,
            left_entries,
            pane_surface_state(state, Pane::Left, left_total_entries, left_entries.len()),
            split_left_list,
            act2,
            left_selection,
            &left_only,
            state.panel_mode,
        );
    } else {
        render_pane(
            frame,
            panes[0],
            &state.left,
            left_entries,
            pane_surface_state(state, Pane::Left, left_total_entries, left_entries.len()),
            left_list,
            state.active == Pane::Left,
            left_selection,
            &left_only,
            state.panel_mode,
        );
    }
    if state.show_terminal {
        if let Some(ref term) = state.term {
            // Render terminal buffer in right pane
            let border_style = if state.active == Pane::Right {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let lines: Vec<Line<'_>> = term
                .buffer
                .iter()
                .skip(term.scroll)
                .take(panes[1].height.saturating_sub(2) as usize)
                .map(|s| Line::from(s.as_str()))
                .collect();
            frame.render_widget(
                Paragraph::new(lines).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Terminal ")
                        .border_style(border_style),
                ),
                panes[1],
            );
        }
    } else {
        if state.right.split {
            let mid = panes[1].width / 2;
            let a1 = ratatui::layout::Rect::new(panes[1].x, panes[1].y, mid, panes[1].height);
            let a2 = ratatui::layout::Rect::new(
                panes[1].x + mid,
                panes[1].y,
                panes[1].width - mid,
                panes[1].height,
            );
            let act1 = state.active == Pane::Right && !state.right.split_active;
            let act2 = state.active == Pane::Right && state.right.split_active;
            right_list.select(Some(state.right.cursor));
            split_right_list.select(Some(state.right.split_cursor));
            render_pane(
                frame,
                a1,
                &state.right,
                right_entries,
                pane_surface_state(state, Pane::Right, right_total_entries, right_entries.len()),
                right_list,
                act1,
                right_selection,
                &right_only,
                state.panel_mode,
            );
            render_pane(
                frame,
                a2,
                &state.right,
                right_entries,
                pane_surface_state(state, Pane::Right, right_total_entries, right_entries.len()),
                split_right_list,
                act2,
                right_selection,
                &right_only,
                state.panel_mode,
            );
        } else {
            render_pane(
                frame,
                panes[1],
                &state.right,
                right_entries,
                pane_surface_state(state, Pane::Right, right_total_entries, right_entries.len()),
                right_list,
                state.active == Pane::Right,
                right_selection,
                &right_only,
                state.panel_mode,
            );
        }
    }

    // Help overlay
    if state.show_help {
        render_help(frame, area);
    }

    // Viewer overlay
    if !state.viewer_content.is_empty() {
        render_viewer(frame, area, state);
    }

    // Which-Key is derived from the active KeyRouter prefix and the shared
    // Action Catalog. There is intentionally no second shortcut table here.
    let input_context = state.input_context();
    let continuations = key_router.continuations(input_context);
    if !continuations.is_empty() {
        let prefix = key_router
            .pending()
            .iter()
            .map(|stroke| stroke.label())
            .collect::<Vec<_>>()
            .join(" ");

        let items: Vec<ListItem> = continuations
            .iter()
            .filter_map(|continuation| {
                action_meta(continuation.action).map(|meta| {
                    ListItem::new(format!(
                        "{:<10} {}",
                        continuation.stroke.label(),
                        meta.label
                    ))
                })
            })
            .collect();

        if !items.is_empty() {
            let height = (items.len() as u16 + 2).min(area.height.max(1));
            let width = area.width.saturating_mul(70).saturating_div(100).max(30);
            let width = width.min(area.width);
            let x = area.x + area.width.saturating_sub(width) / 2;
            let y = area
                .y
                .saturating_add(area.height.saturating_sub(height).saturating_sub(1));
            let popup = Rect::new(x, y, width, height);

            frame.render_widget(Clear, popup);
            frame.render_widget(
                List::new(items).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(format!(" {prefix} … "))
                        .border_style(Style::default().fg(Color::Cyan)),
                ),
                popup,
            );
        }
    }

    // Infrastructure Center overlay (Ctrl+I)
    if state.show_infra {
        let lines = &state.infrastructure_lines;
        let h = (lines.len().max(1) + 3).min(30) as u16;
        let popup = centered_rect(80, h, area);
        frame.render_widget(Clear, popup);
        let items: Vec<ListItem> = lines.iter().map(|l| ListItem::new(l.as_str())).collect();
        let list = ratatui::widgets::List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Infrastructure Center — Ctrl+I toggle "),
            )
            .highlight_style(Style::default().fg(Color::Cyan));
        frame.render_stateful_widget(list, popup, &mut state.overlay_list_state);
    }
    // Smart Tree overlay (Ctrl+T)
    if state.show_tree {
        let tl = &state.tree_lines;
        let h = (tl.len().max(1) + 3).min(30) as u16;
        let popup = centered_rect(80, h, area);
        frame.render_widget(Clear, popup);
        let items: Vec<ListItem> = tl.iter().map(|l| ListItem::new(l.as_str())).collect();
        let title = format!(
            " ARX Smart Tree — :{}_ | Ctrl+T toggle, Esc close ",
            state.tree_filter
        );
        let list = ratatui::widgets::List::new(items)
            .block(Block::default().borders(Borders::ALL).title(title))
            .highlight_style(Style::default().fg(Color::Green));
        frame.render_stateful_widget(list, popup, &mut state.overlay_list_state);
    }
    // Command Center overlay (Ctrl+P)
    if state.show_command_center {
        let h = (state.command_matches.len().max(1) + 3).min(20) as u16;
        let popup = centered_rect(70, h, area);
        frame.render_widget(Clear, popup);
        let items: Vec<ListItem> = state
            .command_matches
            .iter()
            .map(|item| {
                let style = if !item.availability.is_available() {
                    Style::default().fg(Color::DarkGray)
                } else {
                    match item.kind {
                        CommandKind::Action => Style::default().fg(Color::Cyan),
                        CommandKind::Host => Style::default().fg(Color::Green),
                        CommandKind::Bookmark => Style::default().fg(Color::Magenta),
                        CommandKind::History => Style::default(),
                        CommandKind::Session => Style::default().fg(Color::Yellow),
                        CommandKind::UserCommand => Style::default().fg(Color::Blue),
                    }
                };
                let line = match item.availability.reason() {
                    Some(reason) => format!("{}  —  unavailable: {reason}", item.display_line()),
                    None => item.display_line(),
                };
                ListItem::new(line).style(style)
            })
            .collect();

        let list = ratatui::widgets::List::new(items)
            .block(Block::default().borders(Borders::ALL).title(format!(
                " ARX Command Center — :{}_ | bat chafa pdftotext ffprobe 7z ",
                state.filter
            )))
            .highlight_style(Style::default().fg(Color::Yellow));
        frame.render_stateful_widget(list, popup, &mut state.overlay_list_state);
    }

    // Command Center overlay (Ctrl+P)
    // Context menu (right-click)
    if state.show_context_menu {
        let popup = centered_rect(18, 7, area);
        frame.render_widget(Clear, popup);
        let items: Vec<ListItem> = [
            "Copy   F5",
            "Move   F6",
            "Mkdir  F7",
            "Delete F8",
            "View   F3",
            "Edit   F4",
        ]
        .iter()
        .map(|s| ListItem::new(*s))
        .collect();
        let list = ratatui::widgets::List::new(items)
            .block(Block::default().borders(Borders::ALL).title(" Menu "))
            .highlight_style(Style::default().fg(Color::Yellow));
        frame.render_widget(list, popup);
    }

    // Bookmarks overlay
    if state.show_bookmarks {
        render_bookmarks(frame, area, state);
    }

    // Directory history overlay (Alt+H)
    if state.show_history {
        let h = (state.dir_history.len() + 2).min(20) as u16;
        let popup = centered_rect(60, h, area);
        frame.render_widget(Clear, popup);
        let mut items: Vec<ListItem> = state
            .dir_history
            .iter()
            .rev()
            .enumerate()
            .map(|(i, p)| {
                ListItem::new(format!(
                    "{:2}  {}",
                    state.dir_history.len() - i,
                    p.display()
                ))
            })
            .collect();
        if items.is_empty() {
            items.push(ListItem::new("(empty)"));
        }
        frame.render_widget(
            List::new(items).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Directory History (Alt+H) ")
                    .border_style(Style::default().fg(Color::Cyan)),
            ),
            popup,
        );
    }

    // Hotlist overlay
    if state.show_hotlist {
        let hl = arx::app::AppState::load_hotlist();
        let h = (hl.len() + 2).min(20) as u16;
        let popup = centered_rect(60, h, area);
        frame.render_widget(Clear, popup);
        let items: Vec<ListItem> = if hl.is_empty() {
            vec![ListItem::new("(empty - create ~/.config/arx/hotlist)")]
        } else {
            hl.iter()
                .enumerate()
                .map(|(i, p)| {
                    let pre = if i == state.hotlist_cursor {
                        "> "
                    } else {
                        "  "
                    };
                    ListItem::new(format!("{pre}{}", p.display()))
                })
                .collect()
        };
        frame.render_widget(
            List::new(items).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Hotlist (Ctrl+\\) ")
                    .border_style(Style::default().fg(Color::Magenta)),
            ),
            popup,
        );
    }
    // Tab switcher overlay
    if state.show_tab_switcher {
        let mut items: Vec<ListItem> = vec![ListItem::new("── Left pane ──")];
        for (i, tab) in state.left.tabs.iter().enumerate() {
            let idx = items.len();
            let pre = if idx == state.tab_switcher_cursor {
                "> "
            } else {
                "  "
            };
            let loc = match &tab.0 {
                Location::Local(p) => p.display().to_string(),
                o => o.to_string(),
            };
            items.push(ListItem::new(format!("{pre}L{i}: {loc}")));
        }
        items.push(ListItem::new("── Right pane ──"));
        for (i, tab) in state.right.tabs.iter().enumerate() {
            let idx = items.len();
            let pre = if idx == state.tab_switcher_cursor {
                "> "
            } else {
                "  "
            };
            let loc = match &tab.0 {
                Location::Local(p) => p.display().to_string(),
                o => o.to_string(),
            };
            items.push(ListItem::new(format!("{pre}R{i}: {loc}")));
        }
        let h = (items.len() + 2) as u16;
        let popup = centered_rect(60, h, area);
        frame.render_widget(Clear, popup);
        frame.render_widget(
            List::new(items).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Tabs (Alt+`) ")
                    .border_style(Style::default().fg(Color::Cyan)),
            ),
            popup,
        );
    }
    // Rename input bar
    if state.rename_input {
        frame.render_widget(
            Paragraph::new(format!(" Rename: {}_", state.rename_pattern))
                .style(Style::default().fg(Color::Yellow).bg(Color::DarkGray)),
            Rect {
                x: area.x,
                y: area.y + area.height.saturating_sub(2),
                width: area.width,
                height: 1,
            },
        );
    }
    // File search bar (/)
    if state.file_search {
        frame.render_widget(
            Paragraph::new(format!(
                " /{}_  ({})",
                state.search_query,
                state.search_matches.len()
            ))
            .style(Style::default().fg(Color::Yellow).bg(Color::DarkGray)),
            Rect {
                x: area.x,
                y: area.y + area.height.saturating_sub(2),
                width: area.width,
                height: 1,
            },
        );
    }

    // Hosts overlay
    if state.show_hosts {
        render_hosts(frame, area, state);
    }

    // Jobs overlay
    if state.show_jobs {
        render_jobs(frame, area, state);
    }

    // User menu overlay
    if state.show_menu {
        render_menu(frame, area, state);
    }

    // Remote delete confirmation overlay
    if let Some(plan) = &state.pending_delete {
        let file_count = plan
            .targets
            .iter()
            .filter(|t| t.kind == arx::vfs::EntryKind::File)
            .count();
        let symlink_count = plan
            .targets
            .iter()
            .filter(|t| t.kind == arx::vfs::EntryKind::Symlink)
            .count();
        let dir_count = plan
            .targets
            .iter()
            .filter(|t| t.kind == arx::vfs::EntryKind::Directory)
            .count();

        let name_lines: Vec<String> = {
            let max_show = 10;
            let mut names: Vec<String> = plan
                .targets
                .iter()
                .take(max_show)
                .map(|t| format!("  {}", t.name))
                .collect();
            if plan.targets.len() > max_show {
                names.push(format!("  ...and {} more", plan.targets.len() - max_show));
            }
            names
        };

        let breakdown = {
            let mut parts = Vec::new();
            if file_count > 0 {
                parts.push(format!("{file_count} file(s)"));
            }
            if symlink_count > 0 {
                parts.push(format!("{symlink_count} symlink(s)"));
            }
            if dir_count > 0 {
                parts.push(format!("{dir_count} empty dir(s)"));
            }
            if parts.is_empty() {
                "".into()
            } else {
                parts.join(", ")
            }
        };

        let msg = format!(
            "PERMANENT REMOTE DELETE\n\n{} target(s) at {}\n{}\n\nNo Trash / Undo  Enter=Confirm  Esc=Cancel",
            plan.targets.len(),
            plan.location,
            breakdown,
        );

        // Append name lines
        let body = format!("{msg}\n\n{}", name_lines.join("\n"));

        // ponytail: enough room for msg (6 lines) + 2-separator + name_lines + 2-border
        let height = (name_lines.len() + msg.lines().count() + 4).min(area.height as usize) as u16;
        let popup = centered_rect_lines(60, height, area);
        frame.render_widget(Clear, popup);
        let p = ratatui::widgets::Paragraph::new(body)
            .block(
                ratatui::widgets::Block::default()
                    .borders(ratatui::widgets::Borders::ALL)
                    .border_style(Style::default().fg(Color::Red))
                    .title(" Confirm Remote Delete "),
            )
            .alignment(ratatui::layout::Alignment::Left);
        frame.render_widget(p, popup);
    }

    // Status bar
    let pane = state.active_pane();
    let loc_str = match &pane.location {
        Location::Local(p) => p.display().to_string(),
        other => other.to_string(),
    };
    let selection_count = state.selection_count(state.active, &pane.location);
    let hint = if state.cmd_input {
        format!(" :{}_", state.cmd)
    } else if state.go_input {
        format!(" go: {}_", state.filter)
    } else if state.glob_input {
        format!(" glob: {}_", state.filter)
    } else if state.filtering {
        format!(" filter: {}_", state.filter)
    } else if !state.filter.is_empty() {
        format!(" filter: {}", state.filter)
    } else {
        String::new()
    };
    let git_info = state.git_status.as_str();
    let msg_hint = message.map(|m| format!(" | {m}")).unwrap_or_default();

    // Workspace Ribbon — Local ⇄ Remote identity at a glance
    let ribbon_text = if state.remote_workspace.enabled {
        let left_label = state.left.location.label();
        let right_label = state.right.location.label();
        let summary = state.remote_workspace.summary();
        format!("WORKSPACE [LOCAL] {} ⇄ {} · {}", left_label, right_label, summary)
    } else {
        let left_label = state.left.location.label();
        let right_label = state.right.location.label();
        format!("WORKSPACE [LOCAL] {} ⇄ {} · Not compared · Ctrl+D Compare", left_label, right_label)
    };
    let ribbon = Paragraph::new(Line::from(Span::styled(
        ribbon_text,
        Style::default().fg(Color::Cyan),
    )));
    frame.render_widget(ribbon, chunks[1]);

    // Status line — lean, no duplicate path info
    let status = Paragraph::new(Line::from(format!(
        "ARX v{} | sel: {} |{hint}{msg_hint}{git_info}",
        env!("CARGO_PKG_VERSION"),
        selection_count,
    )))
    .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(status, chunks[2]);

    // Session milestones are passive presentation. They never own backend
    // state and disappear on the next user interaction.
    let (footer_row_a, footer_row_b) = if let Some(callout) = session_callout.as_deref() {
        render_session_callout(frame, chunks[2], callout);
        (chunks[3], chunks[4])
    } else {
        (chunks[2], chunks[3])
    };

    // Two-row command bar: Row A = Commander core, Row B = Discovery.
    // Derived from the same runtime Keymap that owns keyboard routing.
    let focused_kind = focused_entry(state, left_entries, right_entries).map(|entry| entry.kind);
    let (row_a, row_b) = command_bar_rows(state, key_router.keymap(), focused_kind, editor_available);
    render_command_bar(
        frame,
        footer_row_a,
        footer_row_b,
        &mut state.command_hitboxes,
        &row_a,
        &row_b,
    );

    if state.active_overlay() == Some(OverlayKind::SyncPreview) {
        render_sync_preview(frame, area, state);
    }
}

fn sync_heading(label: &'static str, color: Color) -> Line<'static> {
    Line::from(Span::styled(label, Style::default().fg(color)))
}

fn render_sync_preview(frame: &mut ratatui::Frame, area: Rect, state: &AppState) {
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
                        "✓ Execution completed",
                        Style::default().fg(Color::Green),
                    )));
                }
                arx::workspace_sync_executor::SyncTerminalState::Cancelled { .. } => {
                    lines.push(Line::from(Span::styled(
                        "Sync cancelled",
                        Style::default().fg(Color::Yellow),
                    )));
                    lines.push(Line::from(format!(
                        "✓ {} physical step(s) completed",
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
                lines.push(Line::from(error.to_string()));
                lines.push(Line::from("The physical result was preserved."));
            }
        }
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
                "✓ Remote Workspace workflow completed end-to-end.",
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
                    "✓ VERIFIED",
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
                    "⚠ DIFFERENCES REMAIN",
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
                "⚠ VERIFICATION FAILED",
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
fn render_help(frame: &mut ratatui::Frame, area: Rect) {
    let popup_area = centered_rect(68, 90, area);
    frame.render_widget(Clear, popup_area);

    let text = vec![
        Line::from(Span::styled("ARX Help", Style::default().fg(Color::Yellow))),
        Line::from(Span::styled(
            "F1/? or Esc to close",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(Span::styled("Navigation", Style::default().fg(Color::Cyan))),
        Line::from("  j/↓ k/↑       Move cursor"),
        Line::from("  Enter          Enter directory / content diff"),
        Line::from("  Backspace      Parent directory"),
        Line::from("  Tab            Switch pane (left ↔ right)"),
        Line::from("  Ctrl+G         Go to path"),
        Line::from("  Ctrl+U         Swap panes"),
        Line::from("  Alt+O          Sync other pane to active"),
        Line::from("  Alt+Down       Go back in directory history"),
        Line::from("  Alt+/          Recursive file search (find)"),
        Line::from("  Ctrl+\\        Open in file manager"),
        Line::from(""),
        Line::from(Span::styled("Tabs", Style::default().fg(Color::Cyan))),
        Line::from("  Ctrl+T         New tab"),
        Line::from("  Ctrl+W         Close tab"),
        Line::from("  Ctrl+←/→       Previous / next tab"),
        Line::from("  Alt+1 … 9      Switch to tab N"),
        Line::from(""),
        Line::from(Span::styled(
            "Selection & Filter",
            Style::default().fg(Color::Cyan),
        )),
        Line::from("  Space          Toggle selection"),
        Line::from("  *              Invert selection"),
        Line::from("  +              Select by glob pattern"),
        Line::from("  /              Quick filter by name"),
        Line::from("  Ctrl+H         Toggle hidden (dot) files"),
        Line::from("  Alt+T          Toggle panel mode (Full/Brief)"),
        Line::from(""),
        Line::from(Span::styled(
            "File Operations",
            Style::default().fg(Color::Cyan),
        )),
        Line::from("  F5             Copy to other pane"),
        Line::from("  F6             Move to other pane"),
        Line::from("  F7             Create directory"),
        Line::from("  F8             Delete"),
        Line::from("  Shift+F6       Rename file"),
        Line::from("  Ctrl+I         File info (stat)"),
        Line::from("  Ctrl+Space     Directory size / free space"),
        Line::from(""),
        Line::from(Span::styled(
            "View & Edit",
            Style::default().fg(Color::Cyan),
        )),
        Line::from("  F3             View file (built-in)"),
        Line::from("  Shift+F3       View with bat (syntax highlight)"),
        Line::from("  F4             Edit (configurable editor)"),
        Line::from(""),
        Line::from(Span::styled(
            "Ctrl+X Prefix (MC-style)",
            Style::default().fg(Color::Cyan),
        )),
        Line::from("  Ctrl+X S       Symlink (ln -s)"),
        Line::from("  Ctrl+X L       Hard link (ln)"),
        Line::from("  Ctrl+X C       chmod"),
        Line::from("  Ctrl+X O       chown"),
        Line::from(""),
        Line::from(Span::styled(
            "Panels & Tools",
            Style::default().fg(Color::Cyan),
        )),
        Line::from("  Ctrl+D         Toggle directory diff"),
        Line::from("  F2             User menu (arx.menu)"),
        Line::from("  F9             Hosts / SFTP"),
        Line::from("  Ctrl+B         Bookmarks"),
        Line::from("  Ctrl+J         Background jobs"),
        Line::from("  Ctrl+O         Shell (drop to subshell)"),
        Line::from("  :              Command line"),
        Line::from(""),
        Line::from(Span::styled("Other", Style::default().fg(Color::Cyan))),
        Line::from("  Ctrl+R         Refresh"),
        Line::from("  q              Quit"),
        Line::from("  F1 / ?         This help"),
    ];

    let help = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).title(" Help "))
        .style(Style::default().fg(Color::White))
        .wrap(Wrap { trim: false });
    frame.render_widget(help, popup_area);
}

fn render_viewer(frame: &mut ratatui::Frame, area: Rect, state: &AppState) {
    let popup_area = centered_rect(80, 90, area);
    frame.render_widget(Clear, popup_area);

    let total = state.viewer_content.len();
    let pct = if total > 0 {
        ((state.viewer_scroll.min(total.saturating_sub(1)) as f64 / total as f64) * 100.0) as usize
    } else {
        0
    };
    let title = format!(" View ({} lines, {}%) ", total, pct);
    let max_scroll = state.viewer_content.len().saturating_sub(1);
    let visible: Vec<Line> = state
        .viewer_content
        .iter()
        .skip(state.viewer_scroll)
        .take(popup_area.height.saturating_sub(2) as usize)
        .map(|l| Line::from(l.as_str()))
        .collect();

    let scroll_hint = if max_scroll > 0 {
        format!(
            " {}/{} | j/k:scroll q/Esc:close ",
            state.viewer_scroll, max_scroll
        )
    } else {
        " q/Esc:close ".into()
    };

    let viewer = Paragraph::new(visible)
        .block(Block::default().borders(Borders::ALL).title(title.as_str()))
        .style(Style::default().fg(Color::White));
    frame.render_widget(viewer, popup_area);

    let hint_area = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(popup_area);
    let hint = Paragraph::new(Line::from(scroll_hint)).style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hint, hint_area[1]);
}

fn render_bookmarks(frame: &mut ratatui::Frame, area: Rect, state: &AppState) {
    let popup_area = centered_rect(60, 70, area);
    frame.render_widget(Clear, popup_area);

    let items: Vec<ListItem> = state
        .bookmarks
        .iter()
        .enumerate()
        .map(|(i, loc)| {
            let prefix = if i == state.bookmark_cursor {
                "> "
            } else {
                "  "
            };
            ListItem::new(Line::from(format!("{prefix}{loc}")))
        })
        .collect();

    let mut list_state = ListState::default();
    list_state.select(Some(state.bookmark_cursor));

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Bookmarks (Ctrl+B: close, Enter: go) "),
        )
        .highlight_style(Style::default().fg(Color::Black).bg(Color::White));
    frame.render_stateful_widget(list, popup_area, &mut list_state);
}

const HOSTS_CONFIG_PATH: &str = "~/.config/arx/hosts.toml";

fn empty_hosts_text() -> String {
    format!("No hosts configured\n\nAdd hosts to:\n{HOSTS_CONFIG_PATH}\n\nEsc Close")
}

fn render_hosts(frame: &mut ratatui::Frame, area: Rect, state: &AppState) {
    if state.hosts.is_empty() {
        let popup_area = centered_rect(60, 40, area);
        frame.render_widget(Clear, popup_area);
        frame.render_widget(
            Paragraph::new(empty_hosts_text())
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Remote Hosts ")
                        .border_style(Style::default().fg(Color::Cyan)),
                )
                .wrap(Wrap { trim: false }),
            popup_area,
        );
        return;
    }
    let popup_area = centered_rect(60, 70, area);
    frame.render_widget(Clear, popup_area);

    let items: Vec<ListItem> = state
        .hosts
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let prefix = if i == state.host_cursor { "> " } else { "  " };
            let line = format!("{prefix}{} ({})", h.name, h.hostname);
            ListItem::new(Line::from(line))
        })
        .collect();

    let mut list_state = ListState::default();
    list_state.select(Some(state.host_cursor));

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Hosts (F9: close, Enter: open) "),
        )
        .highlight_style(Style::default().fg(Color::Black).bg(Color::White));
    frame.render_stateful_widget(list, popup_area, &mut list_state);
}

fn render_jobs(frame: &mut ratatui::Frame, area: Rect, state: &AppState) {
    let popup_area = centered_rect(70, 70, area);
    frame.render_widget(Clear, popup_area);

    let items: Vec<ListItem> = if state.jobs.is_empty() {
        vec![ListItem::new(Line::from(
            "  No jobs yet — start a copy/move to see it here.",
        ))]
    } else {
        state
            .jobs
            .iter()
            .enumerate()
            .map(|(i, j)| {
                let prefix = if i == state.job_cursor { "> " } else { "  " };
                let bar = if j.status == arx::jobs::JobStatus::Running {
                    let filled = j.progress.percent().unwrap_or(0) as usize / 5; // 0-20 chars
                    let empty = 20 - filled;
                    format!(
                        " [{}{}] {}%",
                        "=".repeat(filled),
                        " ".repeat(empty),
                        j.progress
                    )
                } else {
                    String::new()
                };
                ListItem::new(Line::from(format!("{prefix}{j} {bar}")))
            })
            .collect()
    };

    let mut list_state = ListState::default();
    list_state.select(Some(state.job_cursor));

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Jobs (Ctrl+J: close) "),
        )
        .highlight_style(Style::default().fg(Color::Black).bg(Color::White));
    frame.render_stateful_widget(list, popup_area, &mut list_state);
}

fn render_menu(frame: &mut ratatui::Frame, area: Rect, state: &AppState) {
    let popup_area = centered_rect(60, 70, area);
    frame.render_widget(Clear, popup_area);

    let items: Vec<ListItem> = state
        .menu
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let prefix = if i == state.menu_cursor { "> " } else { "  " };
            ListItem::new(Line::from(format!("{prefix}{}", m.label)))
        })
        .collect();

    let mut list_state = ListState::default();
    list_state.select(Some(state.menu_cursor));

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" User Menu (F2: close, Enter: run) "),
        )
        .highlight_style(Style::default().fg(Color::Black).bg(Color::White));
    frame.render_stateful_widget(list, popup_area, &mut list_state);
}

/// ponytail: simple centered rect helper; add flexible sizing when needed
fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

/// Like centered_rect but height is in terminal lines instead of percent.
/// Clamps to available area so the popup never exceeds terminal height.
fn centered_rect_lines(percent_x: u16, lines: u16, area: Rect) -> Rect {
    // ponytail: minimum 8 lines so deletion confirmation never collapses
    let desired = lines.max(8);
    let max_h = area.height.saturating_sub(2);
    let h = desired.min(max_h);
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length((area.height.saturating_sub(h)) / 2),
            Constraint::Length(h),
            Constraint::Length(0),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

#[allow(clippy::too_many_arguments)]
fn render_pane(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    pane: &PaneState,
    entries: &[&Entry],
    surface: PaneSurfaceState<'_>,
    list_state: &mut ListState,
    active: bool,
    selected: &std::collections::BTreeSet<String>,
    unique_set: &std::collections::BTreeSet<&str>,
    panel_mode: PanelMode,
) {
    let border_style = if active {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let title = format!(" {} ", pane.location.label());

    let banner = match surface {
        PaneSurfaceState::Loading {
            target,
            transactional,
        } => {
            let action = if matches!(target, Location::Sftp { .. }) {
                "Connecting…"
            } else {
                "Opening…"
            };
            let detail = if transactional {
                "Current location stays open until this succeeds."
            } else {
                "Loading this location…"
            };
            if entries.is_empty() {
                render_pane_state_message(
                    frame,
                    area,
                    &title,
                    border_style,
                    vec![
                        action.to_string(),
                        target.to_string(),
                        String::new(),
                        detail.to_string(),
                    ],
                );
                return;
            }
            Some((
                format!("{action} {target}"),
                detail.to_string(),
                Style::default().fg(Color::Cyan).bg(Color::DarkGray),
            ))
        }
        PaneSurfaceState::LoadError { attempted, message } => {
            if entries.is_empty() {
                render_pane_state_message(
                    frame,
                    area,
                    &title,
                    border_style,
                    vec![
                        "Could not open".into(),
                        attempted.to_string(),
                        String::new(),
                        message.to_string(),
                        String::new(),
                        "Current location was not changed.".into(),
                    ],
                );
                return;
            }
            Some((
                format!("Could not open {attempted}"),
                format!("{message} · Current location was not changed."),
                Style::default().fg(Color::Yellow).bg(Color::DarkGray),
            ))
        }
        PaneSurfaceState::Empty => {
            render_pane_state_message(
                frame,
                area,
                &title,
                border_style,
                vec![
                    "This folder is empty.".into(),
                    String::new(),
                    "Available actions are shown in the footer.".into(),
                ],
            );
            return;
        }
        PaneSurfaceState::NoMatches { filter } => {
            render_pane_state_message(
                frame,
                area,
                &title,
                border_style,
                vec![
                    format!("No files match \"{filter}\"."),
                    String::new(),
                    "Change or clear the filter to see files.".into(),
                ],
            );
            return;
        }
        PaneSurfaceState::Entries => None,
    };

    if panel_mode == PanelMode::Brief {
        // ponytail: brief mode — filenames in columns, no list state
        let names: Vec<String> = entries
            .iter()
            .map(|e| {
                let icon = match e.kind {
                    EntryKind::Directory => "📁 ",
                    EntryKind::Symlink => "🔗 ",
                    _ => "📄 ",
                };
                let sel_mark = if selected.contains(&e.name) {
                    "* "
                } else {
                    "  "
                };
                format!("{sel_mark}{icon}{}", e.name)
            })
            .collect();
        let text = names.join("  ");
        frame.render_widget(
            Paragraph::new(text)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(title)
                        .border_style(border_style),
                )
                .wrap(Wrap { trim: false }),
            area,
        );
        if let Some((headline, detail, style)) = &banner {
            render_pane_banner(frame, area, headline, detail, *style);
        }
        return;
    }

    let items: Vec<ListItem> = entries
        .iter()
        .map(|e| {
            let sel_mark = if selected.contains(&e.name) {
                "* "
            } else {
                "  "
            };
            let icon = match e.kind {
                EntryKind::Directory => "📁 ",
                EntryKind::Symlink => "🔗 ",
                _ => "📄 ",
            };
            let size_str = e.size.map(format_size).unwrap_or_default();
            let style = if selected.contains(&e.name) {
                Style::default().fg(Color::Yellow)
            } else if !unique_set.is_empty() && unique_set.contains(e.name.as_str()) {
                // ponytail: green for unique entries in diff mode
                Style::default().fg(Color::Green)
            } else {
                let ext = e.name.rsplit('.').next().unwrap_or("");
                match ext {
                    "rs" | "toml" | "lock" => Style::default().fg(Color::LightRed),
                    "py" => Style::default().fg(Color::Blue),
                    "sh" | "bash" | "zsh" => Style::default().fg(Color::Green),
                    "md" | "txt" | "log" => Style::default().fg(Color::White),
                    "json" | "yaml" | "yml" | "xml" | "html" => Style::default().fg(Color::Yellow),
                    "js" | "ts" | "jsx" | "tsx" => Style::default().fg(Color::LightYellow),
                    "c" | "h" | "cpp" | "hpp" => Style::default().fg(Color::LightCyan),
                    "go" => Style::default().fg(Color::Cyan),
                    "pdf" | "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" => {
                        Style::default().fg(Color::Magenta)
                    }
                    "mp4" | "mkv" | "avi" | "mov" => Style::default().fg(Color::LightMagenta),
                    "mp3" | "flac" | "ogg" | "wav" => Style::default().fg(Color::LightBlue),
                    "zip" | "tar" | "gz" | "xz" | "bz2" | "7z" | "rar" => {
                        Style::default().fg(Color::Red)
                    }
                    _ => Style::default(),
                }
            };
            let line = Line::from(vec![
                Span::styled(sel_mark, style),
                Span::styled(icon, style),
                Span::styled(&e.name, style),
                Span::styled(
                    format!("  {size_str}"),
                    Style::default().fg(Color::DarkGray),
                ),
            ]);
            ListItem::new(line)
        })
        .collect();

    let title = format!(" {} ", pane.location.label());

    if panel_mode == PanelMode::Brief {
        let names: Vec<String> = entries
            .iter()
            .map(|e| {
                let icon = match e.kind {
                    EntryKind::Directory => "📁 ",
                    EntryKind::Symlink => "🔗 ",
                    _ => "📄 ",
                };
                let sel_mark = if selected.contains(&e.name) {
                    "* "
                } else {
                    "  "
                };
                format!("{sel_mark}{icon}{}", e.name)
            })
            .collect();
        let text = names.join("  ");
        frame.render_widget(
            Paragraph::new(text)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(title)
                        .border_style(border_style),
                )
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(border_style),
        )
        .highlight_style(Style::default().fg(Color::Black).bg(if active {
            Color::White
        } else {
            Color::DarkGray
        }))
        .highlight_symbol(if active { ">> " } else { "   " });
    frame.render_stateful_widget(list, area, list_state);
    if let Some((headline, detail, style)) = &banner {
        render_pane_banner(frame, area, headline, detail, *style);
    }
}

fn render_pane_state_message(
    frame: &mut ratatui::Frame,
    area: Rect,
    title: &str,
    border_style: Style,
    lines: Vec<String>,
) {
    let lines = lines.into_iter().map(Line::from).collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title.to_string())
                    .border_style(border_style),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_pane_banner(
    frame: &mut ratatui::Frame,
    area: Rect,
    headline: &str,
    detail: &str,
    style: Style,
) {
    if area.width <= 2 || area.height <= 3 {
        return;
    }
    let height = 2.min(area.height.saturating_sub(2));
    let banner = Rect {
        x: area.x + 1,
        y: area.y + area.height.saturating_sub(height + 1),
        width: area.width.saturating_sub(2),
        height,
    };
    frame.render_widget(Clear, banner);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(headline.to_string()),
            Line::from(detail.to_string()),
        ])
        .style(style)
        .wrap(Wrap { trim: false }),
        banner,
    );
}

fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "K", "M", "G", "T"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// Parse filter string for size:>1G, size:<100M modifiers.
fn parse_filter(raw: &str) -> (String, Option<u64>, Option<u64>) {
    let mut name_part = String::new();
    let mut min_size: Option<u64> = None;
    let mut max_size: Option<u64> = None;
    for word in raw.split_whitespace() {
        if let Some(val) = word.strip_prefix("size:>") {
            if let Ok(bytes) = parse_size(val) {
                min_size = Some(bytes);
            }
        } else if let Some(val) = word.strip_prefix("size:<") {
            if let Ok(bytes) = parse_size(val) {
                max_size = Some(bytes);
            }
        } else if let Some(val) = word.strip_prefix("size:") {
            if let Ok(bytes) = parse_size(val) {
                min_size = Some(bytes);
                max_size = Some(bytes);
            }
        } else {
            if !name_part.is_empty() {
                name_part.push(' ');
            }
            name_part.push_str(word);
        }
    }
    (name_part.to_lowercase(), min_size, max_size)
}

fn parse_size(s: &str) -> Result<u64, ()> {
    let s = s.trim();
    let (num_str, mult) = if s.ends_with('G') || s.ends_with('g') {
        (&s[..s.len() - 1], 1_073_741_824)
    } else if s.ends_with('M') || s.ends_with('m') {
        (&s[..s.len() - 1], 1_048_576)
    } else if s.ends_with('K') || s.ends_with('k') {
        (&s[..s.len() - 1], 1024)
    } else {
        (s, 1)
    };
    num_str.parse::<u64>().map(|n| n * mult).map_err(|_| ())
}

fn job_event_id(event: &arx::jobs::JobEvent) -> &str {
    match event {
        arx::jobs::JobEvent::Running { id }
        | arx::jobs::JobEvent::Paused { id }
        | arx::jobs::JobEvent::Progress { id, .. }
        | arx::jobs::JobEvent::Completed { id, .. }
        | arx::jobs::JobEvent::Failed { id, .. }
        | arx::jobs::JobEvent::Cancelled { id, .. } => id,
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

fn is_verified_sync_success(job: &arx::jobs::Job) -> bool {
    if job.status != arx::jobs::JobStatus::Completed {
        return false;
    }
    let execution_completed = matches!(
        &job.result,
        Some(arx::jobs::JobResult::WorkspaceSync(outcome))
            if matches!(
                &outcome.terminal,
                arx::workspace_sync_executor::SyncTerminalState::Completed
            )
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

fn observe_verified_sync_success(state: &mut AppState, job: &arx::jobs::Job) {
    if !is_verified_sync_success(job) {
        return;
    }
    if state.milestones.take_verified_sync_success() {
        state.session_callout = Some(SessionCallout::WorkspaceSyncVerified {
            job_id: job.id.clone(),
        });
    }
}

/// Present an already-accepted JobManager event. Lifecycle state lives in JobManager.
fn handle_job_event(ev: &arx::jobs::JobEvent, state: &mut AppState) -> bool {
    match ev {
        arx::jobs::JobEvent::Completed { id, result } => {
            state.message = Some(match result {
                arx::jobs::JobResult::Generic { message, .. } => message
                    .clone()
                    .unwrap_or_else(|| format!("Job {id} completed")),
                arx::jobs::JobResult::WorkspaceSync(outcome) => format!(
                    "Sync completed: {} physical step(s), {} bytes",
                    outcome.completed.len(),
                    outcome.transferred_bytes
                ),
            });
            true
        }
        arx::jobs::JobEvent::Failed { error, .. } => {
            state.message = Some(error.clone());
            true
        }
        arx::jobs::JobEvent::Cancelled { id, result } => {
            state.message = Some(match result {
                arx::jobs::JobResult::Generic { message, .. } => message
                    .clone()
                    .unwrap_or_else(|| format!("Job {id} cancelled")),
                arx::jobs::JobResult::WorkspaceSync(outcome) => format!(
                    "Sync cancelled after {} completed physical step(s)",
                    outcome.completed.len()
                ),
            });
            true
        }
        arx::jobs::JobEvent::Running { .. }
        | arx::jobs::JobEvent::Progress { .. }
        | arx::jobs::JobEvent::Paused { .. } => false,
    }
}

fn toggle_hosts_overlay(state: &mut AppState) {
    state.toggle_overlay(OverlayKind::Hosts);
}

fn request_quit(state: &mut AppState, effect_dispatcher: &EffectDispatcher) {
    let cancellation_requested = state
        .pending_effect(EffectLane::RemoteEdit)
        .is_some_and(|id| effect_dispatcher.cancel(id));
    state.apply(Action::Quit);
    if cancellation_requested {
        state.message =
            Some("Remote edit cancellation requested — waiting for safe cleanup".into());
    }
}

// ponytail: keep the one action seam instead of wrapping runtime services in a one-use context.
#[allow(clippy::too_many_arguments)]
async fn dispatch_ui_action(
    state: &mut AppState,
    action: Action,
    focused: Option<&Entry>,
    active_entries: &[&Entry],
    visible_count: usize,
    workspace_scanner: &WorkspaceScanner,
    sync: &SyncUiRuntime,
    effect_dispatcher: &EffectDispatcher,
    pane_loader: &PaneLoader,
    terminal_session: &mut TuiTerminalSession,
    configured_editor: Option<&str>,
) -> io::Result<()> {
    let focused = focused.filter(|entry| entry.name != VIRTUAL_PARENT_NAME);
    if matches!(
        action,
        Action::ToggleWorkspaceComparison
            | Action::PreviewWorkspaceSync
            | Action::ReverseWorkspaceDirection
            | Action::ToggleWorkspaceSyncMode
    ) && !supersede_workspace_launch_for_new_action(state, sync)
    {
        return Ok(());
    }

    match action {
        Action::Quit => request_quit(state, effect_dispatcher),
        Action::OpenCommandCenter => {
            state.open_overlay(OverlayKind::CommandCenter);
            state.filter.clear();
            state.command_matches = build_command_items_with_file_context(
                "",
                state,
                focused.map(|entry| entry.kind),
                configured_editor.is_some(),
            );
            state
                .overlay_list_state
                .select((!state.command_matches.is_empty()).then_some(0));
        }
        Action::OpenBookmarks => state.toggle_overlay(OverlayKind::Bookmarks),
        Action::OpenJobs => state.toggle_overlay(OverlayKind::Jobs),
        Action::OpenHosts => toggle_hosts_overlay(state),
        Action::OpenHelp => state.toggle_overlay(OverlayKind::Help),
        Action::ToggleSelect => {
            toggle_selection_and_advance(state, focused, visible_count);
        }
        Action::ViewFile => {
            let Some(entry) = focused.filter(|entry| entry.kind == EntryKind::File) else {
                state.message = Some("Select a regular file to view".into());
                return Ok(());
            };
            let location = state.active_pane().location.clone();
            // SFTP: dispatch preview intent, network I/O runs inside effect lane
            if matches!(location.provider_id(), arx::vfs::ProviderId::Sftp) {
                let name = entry.name.clone();
                let total_size = entry.size;
                let id = effect_dispatcher.dispatch(
                    EffectLane::Preview,
                    EffectScope::Location(location.clone()),
                    Effect::PreviewLocation {
                        location,
                        name: name.clone(),
                        total_size,
                    },
                );
                state.register_effect(EffectLane::Preview, id);
                state.message = Some(format!("Loading preview: {name}"));
                return Ok(());
            }
            let Location::Local(base) = &location else {
                state.message = Some("File preview is currently local-only".into());
                return Ok(());
            };
            let path = base.join(&entry.name);
            let id = effect_dispatcher.dispatch(
                EffectLane::Preview,
                EffectScope::Location(location),
                Effect::PreviewFile { path },
            );
            state.register_effect(EffectLane::Preview, id);
            state.message = Some(format!("Loading preview: {}", entry.name));
        }
        Action::EditFile => {
            let Some(entry) = focused.filter(|entry| entry.kind == EntryKind::File) else {
                state.message = Some("Select a regular file to edit".into());
                return Ok(());
            };
            let Some(editor) = configured_editor else {
                state.message =
                    Some("No editor configured (config.ui.editor, VISUAL, or EDITOR)".into());
                return Ok(());
            };

            match &state.active_pane().location {
                Location::Local(base) => {
                    // Local: direct editor on original file
                    let path = base.join(&entry.name);
                    let editor_result = terminal_session
                        .suspend_while(|| DesktopService::open_editor(editor, &path))
                        .await?;
                    if let Err(error) = editor_result {
                        state.message = Some(format!("Editor failed: {error}"));
                    }
                    schedule_active_pane_load(pane_loader, state);
                }
                Location::Sftp { .. } => {
                    // Remote: download → edit → write-back
                    if state.pending_effects.contains_key(&EffectLane::RemoteEdit)
                        || state.pending_remote_edit_origin.is_some()
                    {
                        state.message = Some("Another remote edit is still in progress".into());
                        return Ok(());
                    }
                    let location = state.active_pane().location.clone();
                    let name = entry.name.clone();

                    state.pending_remote_edit_session = None;
                    state.pending_remote_edit_origin = Some((state.active, location.clone()));
                    let id = effect_dispatcher.dispatch(
                        EffectLane::RemoteEdit,
                        EffectScope::Location(location.clone()),
                        Effect::DownloadRemoteFile {
                            location: location.clone(),
                            name: name.clone(),
                            editor: editor.to_string(),
                        },
                    );
                    state.register_effect(EffectLane::RemoteEdit, id);
                    state.message = Some(format!("Downloading: {name}..."));

                    // ponytail: Phase 2+3 handled in select! when Downloaded arrives
                }
                _ => {
                    state.message = Some("File editing is not supported for this location".into());
                }
            }
        }
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
        Action::Copy => {
            let names: Vec<String> = state
                .selection_names(state.active, &state.active_pane().location)
                .map(|names| names.iter().cloned().collect())
                .or_else(|| focused.map(|entry| vec![entry.name.clone()]))
                .unwrap_or_default();
            if names.is_empty() {
                state.message = Some("Select a file or directory to copy".into());
                return Ok(());
            }
            let src_loc = state.active_pane().location.clone();
            let dst_loc = state.other_pane().location.clone();
            let src_provider = src_loc.provider_id();
            let dst_provider = dst_loc.provider_id();
            let src_caps = state
                .registry
                .capabilities(&src_provider)
                .unwrap_or_default();
            let dst_caps = state
                .registry
                .capabilities(&dst_provider)
                .unwrap_or_default();
            let executors =
                arx::transfer::probe::local_executors(arx::transfer::probe::detect_local_tools());
            let request = arx::transfer::TransferRequest {
                source: src_loc.clone(),
                destination: dst_loc.clone(),
                source_provider: src_provider,
                destination_provider: dst_provider,
                source_capabilities: src_caps,
                destination_capabilities: dst_caps,
                intent: arx::transfer::TransferIntent::Copy,
                executors,
                delete_extraneous: false,
            };
            let plan = match arx::transfer::TransferPlanner::plan(request) {
                Ok(p) => p,
                Err(e) => {
                    state.message = Some(e.to_string());
                    return Ok(());
                }
            };
            let job = sync.jobs.create_job(
                "copy",
                arx::jobs::JobKind::Copy,
                format!("Copy {} → {}", names.join(", "), dst_loc.label()),
                Some(src_loc.clone()),
                Some(dst_loc.clone()),
            );
            let id = job.id.clone();
            let cancel = job.cancel.clone();
            state.jobs = sync.jobs.snapshot();
            let jobs = sync.jobs.clone();
            let tx = sync.job_events.clone();
            let names2 = names.clone();
            let plan2 = plan.clone();
            let job_id = id.clone();
            tokio::spawn(async move {
                if !jobs.publish_event(&tx, arx::jobs::JobEvent::Running { id: job_id.clone() }) {
                    return;
                }
                let tx2 = tx.clone();
                let jid = job_id.clone();
                let result =
                    arx::transfer::executor::execute_transfer(&plan2, &names2, cancel, |p| {
                        let pct = p.completed.saturating_mul(100) / p.total.max(1);
                        let _ = jobs.publish_event(
                            &tx2,
                            arx::jobs::JobEvent::Progress {
                                id: jid.clone(),
                                progress: arx::jobs::Progress::Percent(pct as u8).into(),
                            },
                        );
                    })
                    .await;
                match result {
                    Ok(outcome) => {
                        let _ = jobs.publish_event(
                            &tx,
                            arx::jobs::JobEvent::Completed {
                                id: job_id,
                                result: arx::jobs::JobResult::generic(
                                    format!("Copied {} item(s)", outcome.completed),
                                    outcome.completed,
                                ),
                            },
                        );
                    }
                    Err(arx::transfer::executor::TransferExecutionError::Cancelled {
                        completed,
                    }) => {
                        let _ = jobs.publish_event(
                            &tx,
                            arx::jobs::JobEvent::Cancelled {
                                id: job_id,
                                result: arx::jobs::JobResult::generic(
                                    format!("Cancelled after {completed} item(s)"),
                                    completed,
                                ),
                            },
                        );
                    }
                    Err(e) => {
                        let _ = jobs.publish_event(
                            &tx,
                            arx::jobs::JobEvent::Failed {
                                id: job_id,
                                error: e.to_string(),
                                result: None,
                            },
                        );
                    }
                }
            });
            state.message = Some(format!("Copy queued ({id})"));
            state.clear_selection();
        }
        Action::Move => {
            let names: Vec<String> = state
                .selection_names(state.active, &state.active_pane().location)
                .map(|names| names.iter().cloned().collect())
                .or_else(|| focused.map(|entry| vec![entry.name.clone()]))
                .unwrap_or_default();
            if names.is_empty() {
                state.message = Some("Select a file or directory to move".into());
                return Ok(());
            }
            let src_loc = state.active_pane().location.clone();
            let dst_loc = state.other_pane().location.clone();
            let src_provider = src_loc.provider_id();
            let dst_provider = dst_loc.provider_id();
            let src_caps = state
                .registry
                .capabilities(&src_provider)
                .unwrap_or_default();
            let dst_caps = state
                .registry
                .capabilities(&dst_provider)
                .unwrap_or_default();
            let executors =
                arx::transfer::probe::local_executors(arx::transfer::probe::detect_local_tools());
            let request = arx::transfer::TransferRequest {
                source: src_loc.clone(),
                destination: dst_loc.clone(),
                source_provider: src_provider,
                destination_provider: dst_provider,
                source_capabilities: src_caps,
                destination_capabilities: dst_caps,
                intent: arx::transfer::TransferIntent::Move,
                executors,
                delete_extraneous: false,
            };
            let plan = match arx::transfer::TransferPlanner::plan(request) {
                Ok(p) => p,
                Err(e) => {
                    state.message = Some(e.to_string());
                    return Ok(());
                }
            };
            let job = sync.jobs.create_job(
                "move",
                arx::jobs::JobKind::Move,
                format!("Move {} → {}", names.join(", "), dst_loc.label()),
                Some(src_loc.clone()),
                Some(dst_loc.clone()),
            );
            let id = job.id.clone();
            let cancel = job.cancel.clone();
            state.jobs = sync.jobs.snapshot();
            let jobs = sync.jobs.clone();
            let tx = sync.job_events.clone();
            let names2 = names.clone();
            let plan2 = plan.clone();
            let job_id = id.clone();
            tokio::spawn(async move {
                if !jobs.publish_event(&tx, arx::jobs::JobEvent::Running { id: job_id.clone() }) {
                    return;
                }
                let tx2 = tx.clone();
                let jid = job_id.clone();
                let result =
                    arx::transfer::executor::execute_transfer(&plan2, &names2, cancel, |p| {
                        let pct = p.completed.saturating_mul(100) / p.total.max(1);
                        let _ = jobs.publish_event(
                            &tx2,
                            arx::jobs::JobEvent::Progress {
                                id: jid.clone(),
                                progress: arx::jobs::Progress::Percent(pct as u8).into(),
                            },
                        );
                    })
                    .await;
                match result {
                    Ok(outcome) => {
                        let _ = jobs.publish_event(
                            &tx,
                            arx::jobs::JobEvent::Completed {
                                id: job_id,
                                result: arx::jobs::JobResult::generic(
                                    format!("Moved {} item(s)", outcome.completed),
                                    outcome.completed,
                                ),
                            },
                        );
                    }
                    Err(arx::transfer::executor::TransferExecutionError::Cancelled {
                        completed,
                    }) => {
                        let _ = jobs.publish_event(
                            &tx,
                            arx::jobs::JobEvent::Cancelled {
                                id: job_id,
                                result: arx::jobs::JobResult::generic(
                                    format!("Cancelled after {completed} item(s)"),
                                    completed,
                                ),
                            },
                        );
                    }
                    Err(e) => {
                        let _ = jobs.publish_event(
                            &tx,
                            arx::jobs::JobEvent::Failed {
                                id: job_id,
                                error: e.to_string(),
                                result: None,
                            },
                        );
                    }
                }
            });
            state.message = Some(format!("Move queued ({id})"));
            state.clear_selection();
        }
        Action::Mkdir => {
            // SFTP: use provider-backed mkdir via frozen location
            if state.active_pane().location.provider_id() == arx::vfs::ProviderId::Sftp {
                state.pending_mkdir_location = Some(state.active_pane().location.clone());
                state.cmd = String::new();
                state.cmd_input = true;
            } else {
                // Local: keep existing shell-based mkdir (no regression)
                state.cmd = "mkdir ".into();
                state.cmd_input = true;
            }
        }
        Action::Delete => {
            let names: Vec<String> = state
                .selection_names(state.active, &state.active_pane().location)
                .map(|names| {
                    names
                        .iter()
                        .filter(|n| *n != VIRTUAL_PARENT_NAME)
                        .cloned()
                        .collect()
                })
                .or_else(|| {
                    focused
                        .filter(|e| e.name != VIRTUAL_PARENT_NAME)
                        .map(|entry| vec![entry.name.clone()])
                })
                .unwrap_or_default();
            if names.is_empty() {
                state.message = Some("Select a file or directory to delete".into());
                return Ok(());
            }

            // SFTP: freeze plan for confirmation (no mutation yet)
            if state.active_pane().location.provider_id() == arx::vfs::ProviderId::Sftp {
                let targets: Vec<arx::vfs::RemoteDeleteTarget> = names
                    .iter()
                    .filter_map(|name| {
                        // Resolve real EntryKind from active pane listing
                        let entry = active_entries.iter().find(|e| e.name == *name)?;
                        let path = match &state.active_pane().location {
                            arx::vfs::Location::Sftp { path: p, .. } => {
                                format!("{p}/{name}")
                            }
                            _ => unreachable!(),
                        };
                        Some(arx::vfs::RemoteDeleteTarget {
                            name: name.clone(),
                            kind: entry.kind,
                            path,
                        })
                    })
                    .collect();
                if targets.len() != names.len() {
                    state.message = Some("Selection no longer matches directory contents".into());
                    return Ok(());
                }
                state.pending_delete = Some(arx::vfs::RemoteDeletePlan {
                    location: state.active_pane().location.clone(),
                    targets,
                    created_at: std::time::Instant::now(),
                });
                state.message =
                    Some("Press Enter to confirm permanent deletion, Escape to cancel".into());
                return Ok(());
            }

            // Local: existing trash path
            let Location::Local(dir) = state.active_pane().location.clone() else {
                state.message = Some("Trash is currently available for local files only".into());
                return Ok(());
            };
            let job = sync.jobs.create_job(
                "trash",
                arx::jobs::JobKind::Delete,
                format!("Trash {}", names.join(", ")),
                Some(Location::Local(dir.clone())),
                None,
            );
            let id = job.id.clone();
            let cancel = job.cancel.clone();
            state.jobs = sync.jobs.snapshot();
            let jobs = sync.jobs.clone();
            let tx = sync.job_events.clone();
            let job_id = id.clone();
            tokio::spawn(async move {
                if !jobs.publish_event(&tx, arx::jobs::JobEvent::Running { id: job_id.clone() }) {
                    return;
                }
                let tx_progress = tx.clone();
                let progress_id = job_id.clone();
                let progress_jobs = jobs.clone();
                let result = MutationService::trash_local(dir, names, cancel, move |progress| {
                    let percent = progress.completed.saturating_mul(100) / progress.total.max(1);
                    let _ = progress_jobs.publish_event(
                        &tx_progress,
                        arx::jobs::JobEvent::Progress {
                            id: progress_id.clone(),
                            progress: arx::jobs::Progress::Percent(percent as u8).into(),
                        },
                    );
                })
                .await;
                match result {
                    Ok(outcome) => {
                        let _ = jobs.publish_event(
                            &tx,
                            arx::jobs::JobEvent::Completed {
                                id: job_id,
                                result: arx::jobs::JobResult::generic(
                                    format!("Trashed {} item(s)", outcome.completed),
                                    outcome.completed,
                                ),
                            },
                        );
                    }
                    Err(MutationError::Cancelled { completed }) => {
                        let _ = jobs.publish_event(
                            &tx,
                            arx::jobs::JobEvent::Cancelled {
                                id: job_id,
                                result: arx::jobs::JobResult::generic(
                                    format!("Cancelled after {completed} item(s)"),
                                    completed,
                                ),
                            },
                        );
                    }
                    Err(error) => {
                        let _ = jobs.publish_event(
                            &tx,
                            arx::jobs::JobEvent::Failed {
                                id: job_id,
                                error: error.to_string(),
                                result: None,
                            },
                        );
                    }
                }
            });
            state.clear_selection();
            state.message = Some(format!("Trash queued ({id})"));
        }
        Action::ListTmuxSessions => {
            let id = effect_dispatcher.dispatch(
                EffectLane::TmuxDiscovery,
                EffectScope::Global,
                Effect::ListTmuxSessions,
            );
            state.register_effect(EffectLane::TmuxDiscovery, id);
            state.message = Some("Discovering tmux sessions…".into());
        }
        Action::ConfirmRemoteDelete => {
            let Some(plan) = state.pending_delete.take() else {
                return Ok(());
            };
            let registry = state.registry.clone();
            let pane = state.active;
            let loader = pane_loader.clone();
            let location = plan.location.clone();
            let targets = plan.targets;
            let target_count = targets.len();
            let jobs = sync.jobs.clone();
            let tx = sync.job_events.clone();

            let job = jobs.create_job(
                "remote-delete",
                arx::jobs::JobKind::Delete,
                format!("Permanent delete {} target(s)", targets.len()),
                Some(location.clone()),
                None,
            );

            let _ = jobs.publish_event(&tx, arx::jobs::JobEvent::Running { id: job.id.clone() });

            tokio::spawn(async move {
                let mut completed: usize = 0;
                let mut failed: usize = 0;
                let mut cancelled = false;

                // ── Preflight: revalidate all frozen targets ──────────────
                let (provider, parent_path) = match registry.provider_for_location(&location) {
                    Ok(p) => p,
                    Err(e) => {
                        let _ = jobs.publish_event(
                            &tx,
                            arx::jobs::JobEvent::Failed {
                                id: job.id.clone(),
                                error: format!("Cannot access location: {e}"),
                                result: None,
                            },
                        );
                        return;
                    }
                };

                let fresh_listing = match provider.list_async(&parent_path).await {
                    Ok(entries) => entries,
                    Err(e) => {
                        let _ = jobs.publish_event(
                            &tx,
                            arx::jobs::JobEvent::Failed {
                                id: job.id.clone(),
                                error: format!("Cannot re-list directory: {e}"),
                                result: None,
                            },
                        );
                        return;
                    }
                };

                for target in &targets {
                    match fresh_listing.iter().find(|e| e.name == target.name) {
                        None => {
                            let _ = jobs.publish_event(
                                &tx,
                                arx::jobs::JobEvent::Failed {
                                    id: job.id.clone(),
                                    error: format!(
                                        "Remote contents changed: '{}' no longer exists. Review selection.",
                                        target.name
                                    ),
                                    result: None,
                                },
                            );
                            return;
                        }
                        Some(entry) if entry.kind != target.kind => {
                            let _ = jobs.publish_event(
                                &tx,
                                arx::jobs::JobEvent::Failed {
                                    id: job.id.clone(),
                                    error: format!(
                                        "Remote contents changed: '{}' type changed. Review selection.",
                                        target.name
                                    ),
                                    result: None,
                                },
                            );
                            return;
                        }
                        Some(entry) if entry.kind == arx::vfs::EntryKind::Directory => {
                            match provider.list_async(&target.path).await {
                                Ok(children) if !children.is_empty() => {
                                    let _ = jobs.publish_event(
                                        &tx,
                                        arx::jobs::JobEvent::Failed {
                                            id: job.id.clone(),
                                            error: format!(
                                                "Recursive remote delete is not supported: '{}' is not empty",
                                                target.name
                                            ),
                                            result: None,
                                        },
                                    );
                                    return;
                                }
                                Ok(_) => {} // empty directory — allowed
                                Err(e) => {
                                    let _ = jobs.publish_event(
                                        &tx,
                                        arx::jobs::JobEvent::Failed {
                                            id: job.id.clone(),
                                            error: format!(
                                                "Cannot verify that remote directory '{}' is empty: {}. Nothing was deleted.",
                                                target.name, e
                                            ),
                                            result: None,
                                        },
                                    );
                                    return;
                                }
                            }
                        }
                        _ => {}
                    }
                }
                // ── All targets validated — proceed with deletion ────────

                for target in &targets {
                    if let Some(j) = jobs.get(&job.id)
                        && j.cancel.load(std::sync::atomic::Ordering::Relaxed)
                    {
                        cancelled = true;
                        break;
                    }

                    let result = match target.kind {
                        arx::vfs::EntryKind::Directory => {
                            registry.remove_dir_at(&location, &target.path).await
                        }
                        _ => registry.remove_file_at(&location, &target.path).await,
                    };

                    match result {
                        Ok(()) => completed += 1,
                        Err(_e) => {
                            failed += 1;
                        }
                    }
                }

                // Refresh pane after any physical mutations
                if completed > 0 || failed > 0 {
                    let _ = loader.load(pane, location, PaneLoadPurpose::Refresh);
                }

                if cancelled {
                    let _ = jobs.publish_event(
                        &tx,
                        arx::jobs::JobEvent::Cancelled {
                            id: job.id,
                            result: arx::jobs::JobResult::generic(
                                format!("Cancelled after {completed} deleted, {failed} failed"),
                                completed,
                            ),
                        },
                    );
                } else if failed > 0 {
                    let _ = jobs.publish_event(
                        &tx,
                        arx::jobs::JobEvent::Failed {
                            id: job.id,
                            error: format!("{completed} deleted, {failed} failed"),
                            result: Some(arx::jobs::JobResult::generic(
                                format!("Partial: {completed} deleted, {failed} failed"),
                                completed,
                            )),
                        },
                    );
                } else {
                    let _ = jobs.publish_event(
                        &tx,
                        arx::jobs::JobEvent::Completed {
                            id: job.id,
                            result: arx::jobs::JobResult::generic(
                                format!("{completed} deleted"),
                                completed,
                            ),
                        },
                    );
                }
            });
            state.message = Some(format!("Remote delete: {target_count} target(s) queued"));
        }
        Action::CancelRemoteDelete => {
            state.pending_delete = None;
            state.message = Some("Remote delete cancelled".into());
        }
        Action::ToggleEmbeddedTerminal => {
            if state.show_terminal {
                state.show_terminal = false;
                if let Some(ref mut t) = state.term {
                    t.kill();
                }
                state.term = None;
                state.message = Some("Terminal closed".into());
            } else if let Location::Local(dir) = &state.right.location {
                match arx::terminal::TermPane::spawn(dir) {
                    Ok(t) => {
                        state.term = Some(t);
                        state.show_terminal = true;
                        state.active = Pane::Right;
                        state.message = Some("Terminal started — Esc to close".into());
                    }
                    Err(e) => {
                        state.message = Some(format!("Terminal error: {e}"));
                    }
                }
            }
        }
        _ => state.apply(action),
    }
    Ok(())
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

// ponytail: a context struct would only hide these already-scoped runtime services.
#[allow(clippy::too_many_arguments)]
async fn execute_command_target(
    state: &mut AppState,
    target: CommandTarget,
    focused: Option<&Entry>,
    active_entries: &[&Entry],
    visible_count: usize,
    workspace_scanner: &WorkspaceScanner,
    pane_loader: &PaneLoader,
    sync: &SyncUiRuntime,
    effect_dispatcher: &EffectDispatcher,
    terminal_session: &mut TuiTerminalSession,
    configured_editor: Option<&str>,
) -> io::Result<Option<Effect>> {
    let effect = match target {
        CommandTarget::Action(action) => {
            dispatch_ui_action(
                state,
                action,
                focused,
                active_entries,
                visible_count,
                workspace_scanner,
                sync,
                effect_dispatcher,
                pane_loader,
                terminal_session,
                configured_editor,
            )
            .await?;
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
    };
    Ok(effect)
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

fn handle_workspace_scan_response(response: WorkspaceScanResponse, state: &mut AppState) {
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

fn finalize_received_effect(dispatcher: &EffectDispatcher, response: &mut EffectResponse) {
    let was_cancelled = dispatcher
        .finish(response.id)
        .is_some_and(|cancellation| cancellation.is_cancelled());
    if was_cancelled && matches!(&response.event, EffectEvent::Downloaded { .. }) {
        response.event = EffectEvent::Failed {
            label: "remote edit download".into(),
            error: "cancelled before editor launch".into(),
        };
    }
}

fn finish_remote_editor(
    mut session: RemoteEditSession,
    editor_result: io::Result<()>,
    state: &mut AppState,
) -> Option<Effect> {
    if let Err(error) = editor_result {
        session.state = RemoteEditState::Failed;
        state.pending_remote_edit_origin = None;
        state.message = Some(format!("Editor failed: {error}"));
        return None;
    }

    session.state = RemoteEditState::WritingBack;
    Some(Effect::WriteBackRemoteFile { session })
}

fn apply_effect_event(state: &mut AppState, event: EffectEvent) {
    match event {
        EffectEvent::ShellCaptured {
            command,
            success,
            stdout,
            stderr,
        } => {
            let stdout = stdout.trim();
            let stderr = stderr.trim();
            let text = if !stdout.is_empty() {
                stdout
            } else if !stderr.is_empty() {
                stderr
            } else if success {
                "ok"
            } else {
                "failed"
            };
            state.message = Some(format!(": {command} — {}", truncate_message(text, 80)));
        }
        EffectEvent::ProcessExited { label, success } => {
            state.message = Some(format!(
                "{label} — {}",
                if success { "done" } else { "failed" }
            ));
        }
        EffectEvent::Spawned { label } => {
            state.message = Some(format!("{label} — started"));
        }
        EffectEvent::TmuxSessions { sessions } => {
            if sessions.is_empty() {
                state.message = Some("No tmux sessions found".into());
                return;
            }
            state.command_matches = sessions
                .into_iter()
                .map(|name| CommandItem {
                    title: name.clone(),
                    subtitle: Some("Attach tmux session".into()),
                    kind: CommandKind::Session,
                    target: CommandTarget::TmuxSession(name),
                    score: 0,
                    availability: ActionAvailability::Available,
                })
                .collect();
            state.open_overlay(OverlayKind::CommandCenter);
            state.overlay_list_state.select(Some(0));
        }
        EffectEvent::ViewerLines { title, lines } => {
            state.viewer_content = lines;
            state.viewer_scroll = 0;
            state.message = Some(title);
        }
        EffectEvent::InfrastructureLines { lines } => {
            state.infrastructure_lines = if lines.is_empty() {
                vec!["No SSH hosts discovered".into()]
            } else {
                lines
            };
        }
        EffectEvent::TreeLines { lines } => {
            state.tree_lines = if lines.is_empty() {
                vec!["(empty)".into()]
            } else {
                lines
            };
        }
        EffectEvent::PathOpened { path } => {
            state.message = Some(format!("Opened {}", path.display()));
        }
        EffectEvent::Downloaded { session } => {
            state.message = Some(format!("Downloaded: {}", session.name));
            state.pending_remote_edit_session = Some(session);
        }
        EffectEvent::WrittenBack { name } => {
            state.message = Some(format!("Uploaded: {name}"));
            state.pending_remote_edit_session = None;
        }
        EffectEvent::NoChange { name } => {
            state.message = Some(format!("No changes: {name}"));
            state.pending_remote_edit_session = None;
        }
        EffectEvent::RemoteConflict { name, reason } => {
            state.message = Some(format!(
                "{name} changed on remote — write-back refused: {reason}"
            ));
            state.pending_remote_edit_session = None;
        }
        EffectEvent::RecoveryRequired { name, details } => {
            state.message = Some(format!("{name}: RECOVERY REQUIRED — {details}"));
            state.pending_remote_edit_session = None;
        }
        EffectEvent::WrittenBackWarning { name, warning } => {
            state.message = Some(format!("Uploaded {name} with warning: {warning}"));
            state.pending_remote_edit_session = None;
        }
        EffectEvent::Failed { label, error } => {
            state.message = Some(format!("{label} failed: {error}"));
        }
    }
}

fn handle_effect_response(
    response: EffectResponse,
    state: &mut AppState,
    _left_entries: &mut Vec<Entry>,
    _right_entries: &mut Vec<Entry>,
    pane_loader: &PaneLoader,
) {
    if !state.accepts_effect(response.id, response.lane, &response.scope) {
        return;
    }
    let refresh_origin = if matches!(
        &response.event,
        EffectEvent::WrittenBack { .. } | EffectEvent::WrittenBackWarning { .. }
    ) {
        state.pending_remote_edit_origin.clone()
    } else {
        None
    };
    let remote_terminal = response.lane == EffectLane::RemoteEdit
        && !matches!(&response.event, EffectEvent::Downloaded { .. });

    state.finish_effect(response.lane, response.id);
    apply_effect_event(state, response.event);
    if remote_terminal {
        state.pending_remote_edit_origin = None;
    }

    if let Some((pane, location)) = refresh_origin
        && pane_still_at_location(state, pane, &location)
    {
        schedule_pane_load(pane_loader, state, pane);
    }

    match response.lane {
        EffectLane::LeftPane => {
            schedule_pane_load(pane_loader, state, Pane::Left);
        }
        EffectLane::RightPane => {
            schedule_pane_load(pane_loader, state, Pane::Right);
        }
        _ => {}
    }
}

fn pane_still_at_location(state: &AppState, pane: Pane, location: &Location) -> bool {
    match pane {
        Pane::Left => &state.left.location == location,
        Pane::Right => &state.right.location == location,
    }
}

fn truncate_message(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let prefix: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{prefix}...")
    } else {
        prefix
    }
}

// ponytail: keep for test visibility; selection logic lives in dispatch_ui_action.
#[allow(dead_code)]
fn selection_or_cursor(state: &AppState, entries: &[&Entry], cursor: usize) -> Vec<String> {
    let pane = state.active_pane();
    if let Some(selected) = state.selection_names(state.active, &pane.location) {
        selected.iter().cloned().collect()
    } else if let Some(entry) = entries
        .get(cursor)
        .filter(|entry| entry.name != VIRTUAL_PARENT_NAME)
    {
        vec![entry.name.clone()]
    } else {
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arx::app::{Action, InputContext};
    use arx::input::{KeyBinding, KeyStroke, Keymap};
    use arx::jobs::{Job, JobKind, JobResult, JobStatus};
    use arx::process::ProcessService;
    use arx::services::{PaneLoadId, WorkspaceScanId};
    use arx::vfs::{Capability, CapabilitySet, ProviderRegistry};
    use arx::workspace_sync::{
        WorkspaceDiff, WorkspaceEntry, WorkspaceFingerprint, WorkspaceSyncPlan,
    };
    use arx::workspace_sync_execution::SyncPlanValidator;
    use arx::workspace_sync_executor::{
        SyncExecutionOutcome, SyncJournalFinalization, SyncTerminalState,
    };
    use arx::workspace_sync_verification::{
        SyncVerificationId, SyncVerificationResult, SyncVerificationSnapshot,
    };

    fn shell_quote(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }

    async fn remote_script(host: &str, script: &str) -> std::process::ExitStatus {
        ProcessService::status(
            "ssh",
            &[host.to_string(), format!("sh -c {}", shell_quote(script))],
            None,
        )
        .await
        .expect("run physical SFTP fixture command")
    }

    struct PhysicalRemoteEditFixture {
        host: String,
        base: String,
    }

    impl PhysicalRemoteEditFixture {
        async fn new(host: &str, case: &str) -> Self {
            use std::time::{SystemTime, UNIX_EPOCH};

            arx::remote::validate_ssh_alias(host).unwrap();
            let token = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let base = format!("/tmp/arx-demo/arx-remote-edit-{case}-{token}");
            let quoted_base = shell_quote(&base);
            let path = shell_quote(&format!("{base}/note.txt"));
            let script = format!(
                "set -eu; rm -rf -- {quoted_base}; mkdir -m 700 -- {quoted_base}; printf '%s' 'original text\n' > {path}; chmod 600 -- {path}"
            );
            assert!(remote_script(host, &script).await.success());
            Self {
                host: host.to_string(),
                base,
            }
        }

        fn location(&self) -> Location {
            Location::Sftp {
                host: self.host.clone(),
                path: self.base.clone(),
            }
        }

        async fn cleanup(&self) {
            let script = format!("rm -rf -- {}", shell_quote(&self.base));
            assert!(remote_script(&self.host, &script).await.success());
        }
    }

    fn physical_sftp_registry(host: &str) -> ProviderRegistry {
        let registry = ProviderRegistry::new();
        registry.insert_sftp(
            host,
            Box::new(arx::vfs::sftp::SftpProvider::new(
                arx::remote::Host::from_alias(host),
            )),
            CapabilitySet::NONE
                .with(Capability::List)
                .with(Capability::Read)
                .with(Capability::Write),
        );
        registry
    }

    async fn assert_remote_original(registry: &ProviderRegistry, location: &Location) {
        let bytes = registry
            .read_all_capped_at(location, "note.txt", 64)
            .await
            .unwrap()
            .bytes;
        assert_eq!(bytes, b"original text\n");
    }

    #[tokio::test]
    #[ignore = "requires ARX_SFTP_SMOKE_HOST pointing at a disposable SSH/SFTP host"]
    async fn physical_editor_nonzero_never_schedules_writeback() {
        let host = std::env::var("ARX_SFTP_SMOKE_HOST").unwrap();
        let fixture = PhysicalRemoteEditFixture::new(&host, "editor-failure").await;
        let location = fixture.location();
        let registry = physical_sftp_registry(&host);
        let (dispatcher, mut responses) = EffectDispatcher::channel(registry.clone());
        let (pane_loader, _pane_responses) = PaneLoader::channel(registry.clone());
        let mut state = AppState {
            registry: registry.clone(),
            ..AppState::default()
        };
        state.left.location = location.clone();
        state.pending_remote_edit_origin = Some((Pane::Left, location.clone()));
        let id = dispatcher.dispatch(
            EffectLane::RemoteEdit,
            EffectScope::Location(location.clone()),
            Effect::DownloadRemoteFile {
                location: location.clone(),
                name: "note.txt".into(),
                editor: "false".into(),
            },
        );
        state.register_effect(EffectLane::RemoteEdit, id);
        let mut response =
            tokio::time::timeout(std::time::Duration::from_secs(20), responses.recv())
                .await
                .unwrap()
                .unwrap();
        assert!(matches!(&response.event, EffectEvent::Downloaded { .. }));
        finalize_received_effect(&dispatcher, &mut response);
        handle_effect_response(
            response,
            &mut state,
            &mut Vec::new(),
            &mut Vec::new(),
            &pane_loader,
        );

        let session = state.pending_remote_edit_session.take().unwrap();
        let temp_path = session.temp_dir.path().to_path_buf();
        let working_path = temp_path.join("working");
        let editor_result = DesktopService::open_editor("false", &working_path).await;
        assert!(editor_result.is_err());
        assert!(finish_remote_editor(session, editor_result, &mut state).is_none());
        assert!(state.pending_remote_edit_origin.is_none());
        assert!(
            !temp_path.exists(),
            "secure temporary directory must be dropped"
        );
        assert_remote_original(&registry, &location).await;
        fixture.cleanup().await;
    }

    #[tokio::test]
    #[ignore = "requires ARX_SFTP_SMOKE_HOST pointing at a disposable SSH/SFTP host"]
    async fn physical_queued_download_cancel_never_reaches_editor() {
        let host = std::env::var("ARX_SFTP_SMOKE_HOST").unwrap();
        let fixture = PhysicalRemoteEditFixture::new(&host, "queued-cancel").await;
        let location = fixture.location();
        let registry = physical_sftp_registry(&host);
        let (dispatcher, mut responses) = EffectDispatcher::channel(registry.clone());
        let (pane_loader, _pane_responses) = PaneLoader::channel(registry.clone());
        let mut state = AppState {
            registry: registry.clone(),
            ..AppState::default()
        };
        state.left.location = location.clone();
        state.pending_remote_edit_origin = Some((Pane::Left, location.clone()));
        let id = dispatcher.dispatch(
            EffectLane::RemoteEdit,
            EffectScope::Location(location.clone()),
            Effect::DownloadRemoteFile {
                location: location.clone(),
                name: "note.txt".into(),
                editor: "false".into(),
            },
        );
        state.register_effect(EffectLane::RemoteEdit, id);
        let mut response =
            tokio::time::timeout(std::time::Duration::from_secs(20), responses.recv())
                .await
                .unwrap()
                .unwrap();
        let temp_path = match &response.event {
            EffectEvent::Downloaded { session } => session.temp_dir.path().to_path_buf(),
            event => panic!("expected queued download, got {event:?}"),
        };
        assert!(
            dispatcher.cancel(id),
            "queued response must remain cancellable"
        );
        finalize_received_effect(&dispatcher, &mut response);
        assert!(matches!(&response.event, EffectEvent::Failed { .. }));
        handle_effect_response(
            response,
            &mut state,
            &mut Vec::new(),
            &mut Vec::new(),
            &pane_loader,
        );

        assert!(state.pending_remote_edit_session.is_none());
        assert!(state.pending_remote_edit_origin.is_none());
        assert!(
            !temp_path.exists(),
            "queued session must be dropped before editor launch"
        );
        assert_remote_original(&registry, &location).await;
        fixture.cleanup().await;
    }

    #[test]
    fn remote_edit_origin_is_bound_to_one_exact_pane_and_location() {
        let mut state = AppState::default();
        let origin = Location::Sftp {
            host: "test".into(),
            path: "/srv".into(),
        };
        state.left.location = origin.clone();
        state.right.location = origin.clone();

        assert!(pane_still_at_location(&state, Pane::Left, &origin));
        assert!(pane_still_at_location(&state, Pane::Right, &origin));

        state.left.location = Location::Local("/elsewhere".into());
        assert!(!pane_still_at_location(&state, Pane::Left, &origin));
        assert!(pane_still_at_location(&state, Pane::Right, &origin));
    }

    #[test]
    fn footer_uses_remapped_file_action_bindings() {
        let keymap = Keymap::new(vec![
            KeyBinding::new(
                InputContext::Browser,
                vec![KeyStroke::new(KeyCode::F(12), KeyModifiers::NONE)],
                Action::ViewFile,
            ),
            KeyBinding::new(
                InputContext::Browser,
                vec![KeyStroke::new(KeyCode::F(11), KeyModifiers::NONE)],
                Action::EditFile,
            ),
            KeyBinding::new(
                InputContext::Browser,
                vec![
                    KeyStroke::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
                    KeyStroke::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                ],
                Action::BeginChmod,
            ),
        ]);
        let mut router = KeyRouter::new(keymap);

        assert_eq!(
            command_bar_text_wrapper(
                &AppState::default(),
                &router,
                Some(EntryKind::File),
                true,
                u16::MAX,
            )
            .as_deref(),
            Some("F12 View file    F11 Edit file")
        );

        assert_eq!(
            router.resolve_stroke(
                InputContext::Browser,
                KeyStroke::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
            ),
            KeyResolution::Pending
        );
        assert!(
            command_bar_text_wrapper(
                &AppState::default(),
                &router,
                Some(EntryKind::File),
                true,
                u16::MAX,
            )
            .is_none()
        );
    }

    #[test]
    fn footer_fits_priority_prefix_to_real_width() {
        let state = AppState::default();
        let router = KeyRouter::default();
        let wide = command_bar_text_wrapper(&state, &router, Some(EntryKind::File), true, u16::MAX)
            .unwrap();
        // Row A has 7 chips (F3-F9)
        assert_eq!(wide.split("    ").count(), 7);
        assert!(wide.contains("F3 View file"));
        assert!(wide.contains("F4 Edit file"));
        assert!(wide.contains("F5 Copy"));
        assert!(wide.contains("F6 Move"));
        assert!(wide.contains("F7 New directory"));
        assert!(wide.contains("F8 Delete"));
        assert!(wide.contains("F9 Hosts"));

        let first = wide.split("    ").next().unwrap();
        let first_width = u16::try_from(Line::from(first).width()).unwrap();
        assert_eq!(
            command_bar_text_wrapper(&state, &router, Some(EntryKind::File), true, first_width,)
                .as_deref(),
            Some(first)
        );
        assert!(
            command_bar_text_wrapper(
                &state,
                &router,
                Some(EntryKind::File),
                true,
                first_width - 1,
            )
            .is_none()
        );
        // On directory focus, ViewFile is present but unavailable (dimmed)
        let dir_text =
            command_bar_text_wrapper(&state, &router, Some(EntryKind::Directory), true, u16::MAX)
                .unwrap();
        assert!(
            dir_text.contains("F3 View file"),
            "ViewFile should be visible even on directory focus"
        );
    }

    #[test]
    fn footer_derives_file_action_from_keymap_not_hardcoded() {
        // Remap Copy to F10; footer must follow runtime Keymap, not hardcoded F5.
        let state = AppState::default();
        let base = Keymap::default();
        let mut bindings: Vec<_> = base
            .bindings()
            .iter()
            .filter(|b| {
                !(b.context == InputContext::Browser
                    && b.action == Action::Copy
                    && b.sequence.len() == 1
                    && matches!(b.sequence[0].code, KeyCode::F(5)))
            })
            .cloned()
            .collect();
        bindings.push(KeyBinding::new(
            InputContext::Browser,
            vec![KeyStroke::new(KeyCode::F(10), KeyModifiers::NONE)],
            Action::Copy,
        ));
        let router = KeyRouter::new(Keymap::new(bindings));
        let wide = command_bar_text_wrapper(&state, &router, Some(EntryKind::File), true, u16::MAX)
            .unwrap();
        assert!(
            wide.contains("F10 Copy"),
            "footer must derive copy key from remapped Keymap: {wide}"
        );
        assert!(
            !wide.contains("F5 Copy"),
            "footer must not show old F5 for Copy after remap: {wide}"
        );
    }

    #[test]
    fn pending_chord_leaves_discovery_to_which_key() {
        let mut router = KeyRouter::default();
        let resolution = router.resolve_stroke(
            InputContext::Browser,
            KeyStroke::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
        );

        assert_eq!(resolution, KeyResolution::Pending);
        assert!(
            command_bar_text_wrapper(&AppState::default(), &router, None, false, u16::MAX)
                .is_none()
        );
    }

    fn file(name: &str) -> Entry {
        Entry {
            name: name.into(),
            kind: EntryKind::File,
            size: Some(1),
            modified_unix_ms: None,
        }
    }

    #[test]
    fn selection_from_other_pane_is_not_a_mutation_target() {
        let mut state = AppState::default();
        let left_location = state.left.location.clone();
        state.toggle_selection(Pane::Left, &left_location, "left-only.txt");
        state.active = Pane::Right;
        let right_entries = [file("right.txt")];
        let visible = right_entries.iter().collect::<Vec<_>>();

        assert_eq!(selection_or_cursor(&state, &visible, 0), vec!["right.txt"]);
    }

    #[test]
    fn selection_from_previous_location_is_not_a_mutation_target() {
        let mut state = AppState::default();
        state.left.location = Location::Local("/a".into());
        let original = state.left.location.clone();
        state.toggle_selection(Pane::Left, &original, "foo.txt");
        state.left.location = Location::Local("/b".into());
        let current_entries = [file("bar.txt")];
        let visible = current_entries.iter().collect::<Vec<_>>();

        assert_eq!(selection_or_cursor(&state, &visible, 0), vec!["bar.txt"]);
        assert_eq!(state.selection_count(Pane::Left, &state.left.location), 0);
    }

    #[test]
    fn selection_toggle_advances_first_and_middle_but_stops_at_last() {
        let entries = [file("first"), file("middle"), file("last")];

        for (start, expected_cursor) in [(0, 1), (1, 2), (2, 2)] {
            let mut state = AppState::default();
            state.left.cursor = start;
            let location = state.left.location.clone();

            toggle_selection_and_advance(&mut state, entries.get(start), entries.len());

            assert!(state.is_selected(Pane::Left, &location, &entries[start].name));
            assert_eq!(state.left.cursor, expected_cursor);
        }
    }

    #[test]
    fn selection_toggle_advances_within_filtered_entries_only() {
        let entries = [file("first"), file("hidden"), file("last")];
        let visible = [&entries[0], &entries[2]];
        let mut state = AppState::default();
        let location = state.left.location.clone();

        toggle_selection_and_advance(&mut state, visible.first().copied(), visible.len());
        assert_eq!(state.left.cursor, 1);
        toggle_selection_and_advance(&mut state, visible.get(1).copied(), visible.len());

        assert_eq!(state.left.cursor, 1);
        assert!(state.is_selected(Pane::Left, &location, "first"));
        assert!(!state.is_selected(Pane::Left, &location, "hidden"));
        assert!(state.is_selected(Pane::Left, &location, "last"));
    }

    #[test]
    fn selection_toggle_is_a_noop_for_an_empty_list() {
        let mut state = AppState::default();
        let location = state.left.location.clone();

        toggle_selection_and_advance(&mut state, None, 0);

        assert_eq!(state.left.cursor, 0);
        assert_eq!(state.selection_count(Pane::Left, &location), 0);
    }

    #[test]
    fn virtual_parent_is_always_visible_exactly_once_away_from_root() {
        let entries = [
            Entry {
                name: "..".into(),
                kind: EntryKind::Directory,
                size: None,
                modified_unix_ms: None,
            },
            file("child.txt"),
        ];
        let parent = virtual_parent_entry();

        let non_roots = [
            Location::Local("/tmp/work".into()),
            Location::Sftp {
                host: "prod".into(),
                path: "/srv".into(),
            },
            Location::Archive {
                archive: "/tmp/data.zip".into(),
                inner_path: String::new(),
            },
            Location::Archive {
                archive: "/tmp/data.zip".into(),
                inner_path: "nested".into(),
            },
        ];
        for location in non_roots {
            let visible = apply_filter_with_parent(&entries, "does-not-match", &location, &parent);
            assert_eq!(
                visible
                    .iter()
                    .map(|entry| entry.name.as_str())
                    .collect::<Vec<_>>(),
                vec![".."]
            );
        }

        let roots = [
            Location::Local("/".into()),
            Location::Sftp {
                host: "prod".into(),
                path: "/".into(),
            },
        ];
        for location in roots {
            let visible = apply_filter_with_parent(&entries, "", &location, &parent);
            assert!(!visible.iter().any(|entry| entry.name == ".."));
        }
    }

    #[test]
    fn virtual_parent_navigates_but_is_not_a_mutation_target() {
        let parent = virtual_parent_entry();
        let location = Location::Local("/tmp/work".into());
        let mut state = AppState::default();
        state.left.location = location.clone();

        assert_eq!(
            directory_navigation_target(&location, &parent),
            Some(Location::Local("/tmp".into()))
        );
        assert!(selection_or_cursor(&state, &[&parent], 0).is_empty());
        toggle_selection_and_advance(&mut state, Some(&parent), 1);
        assert_eq!(state.selection_count(Pane::Left, &location), 0);
    }

    #[test]
    fn footer_tracks_sync_preview_confirmation_and_running_contexts() {
        let left = Location::Local("/left".into());
        let right = Location::Local("/right".into());
        let router = KeyRouter::default();

        let mut preview = AppState::default();
        preview.remote_workspace.preview_open = true;
        preview.remote_workspace.refresh_visible(
            left.clone(),
            right.clone(),
            &[file("source.txt")],
            &[],
        );
        let preview_footer =
            command_bar_text_wrapper(&preview, &router, None, false, u16::MAX).unwrap_or_default();
        assert!(preview_footer.contains("Enter Execute workspace sync"));
        assert!(preview_footer.contains("D Reverse sync direction"));
        assert!(preview_footer.contains("M Toggle update/mirror"));

        let mut confirmation = AppState::default();
        confirmation.left.location = left.clone();
        confirmation.right.location = right.clone();
        confirmation.remote_workspace.preview_open = true;
        confirmation.remote_workspace.refresh_visible(
            left,
            right,
            &[],
            &[file("destination-only.txt")],
        );
        confirmation.remote_workspace.toggle_mode();
        let plan = confirmation
            .remote_workspace
            .plan
            .clone()
            .expect("mirror preview plan");
        let diff = confirmation
            .remote_workspace
            .diff
            .clone()
            .expect("mirror preview diff");
        let frozen = arx::workspace_sync_execution::SyncPlanValidator::freeze(
            &plan,
            &diff,
            &arx::vfs::default_registry(),
        )
        .expect("freeze mirror preview");
        confirmation.remote_workspace.set_frozen_plan(frozen);
        let confirmation_footer =
            command_bar_text_wrapper(&confirmation, &router, None, false, u16::MAX)
                .unwrap_or_default();
        assert!(confirmation_footer.contains("Enter Confirm workspace sync"));
        assert!(confirmation_footer.contains("Esc Back in workspace sync"));

        let mut running = AppState::default();
        running.remote_workspace.preview_open = true;
        running.remote_workspace.ux = WorkspaceSyncUxState::Running {
            job_id: "sync-1".into(),
        };
        let running_footer =
            command_bar_text_wrapper(&running, &router, None, false, u16::MAX).unwrap_or_default();
        assert!(running_footer.contains("C Cancel workspace sync"));
        assert!(running_footer.contains("Esc Hide workspace sync"));
    }

    #[test]
    fn pane_surface_distinguishes_empty_filtered_loading_and_error_states() {
        let state = AppState::default();
        assert_eq!(
            pane_surface_state(&state, Pane::Left, 0, 0),
            PaneSurfaceState::Empty
        );

        let filtered = AppState {
            filter: "xyz".into(),
            ..AppState::default()
        };
        assert!(matches!(
            pane_surface_state(&filtered, Pane::Left, 5, 0),
            PaneSurfaceState::NoMatches { filter: "xyz" }
        ));

        let target = Location::Sftp {
            host: "prod".into(),
            path: "/srv/app".into(),
        };
        let mut loading = AppState::default();
        loading.register_pane_load(
            Pane::Left,
            PaneLoadId(1),
            target.clone(),
            PaneLoadPurpose::Navigate {
                remember_current: true,
            },
        );
        assert!(matches!(
            pane_surface_state(&loading, Pane::Left, 4, 4),
            PaneSurfaceState::Loading {
                target: pending,
                transactional: true,
            } if pending == &target
        ));

        loading.finish_pane_load(Pane::Left, PaneLoadId(1));
        loading.pane_load_errors.insert(
            Pane::Left,
            PaneLoadUiError {
                attempted: target.clone(),
                message: "SSH connection failed".into(),
            },
        );
        assert!(matches!(
            pane_surface_state(&loading, Pane::Left, 4, 4),
            PaneSurfaceState::LoadError { attempted, message }
                if attempted == &target && message == "SSH connection failed"
        ));
    }

    #[test]
    fn navigation_completion_clears_selection_even_if_pane_became_inactive() {
        let mut state = AppState::default();
        let original = state.left.location.clone();
        let target = Location::Local("/target".into());
        state.toggle_selection(Pane::Left, &original, "foo.txt");
        state.register_pane_load(
            Pane::Left,
            PaneLoadId(1),
            target.clone(),
            PaneLoadPurpose::Navigate {
                remember_current: true,
            },
        );
        state.active = Pane::Right;
        let mut left_entries = vec![file("current.txt")];
        let mut right_entries = Vec::new();

        apply_pane_load_response(
            PaneLoadResponse {
                id: PaneLoadId(1),
                pane: Pane::Left,
                location: target.clone(),
                purpose: PaneLoadPurpose::Navigate {
                    remember_current: true,
                },
                result: Ok(vec![file("foo.txt")]),
            },
            &mut state,
            &mut left_entries,
            &mut right_entries,
        );

        assert_eq!(state.left.location, target);
        assert_eq!(state.selection_count(Pane::Left, &original), 0);
        assert!(state.selected.is_empty());
    }

    #[test]
    fn failed_navigation_keeps_location_and_success_clears_error() {
        let mut state = AppState::default();
        let original = state.left.location.clone();
        let target = Location::Local("/target".into());
        let mut left_entries = vec![file("current.txt")];
        let mut right_entries = Vec::new();

        state.register_pane_load(
            Pane::Left,
            PaneLoadId(1),
            target.clone(),
            PaneLoadPurpose::Navigate {
                remember_current: true,
            },
        );
        apply_pane_load_response(
            PaneLoadResponse {
                id: PaneLoadId(1),
                pane: Pane::Left,
                location: target.clone(),
                purpose: PaneLoadPurpose::Navigate {
                    remember_current: true,
                },
                result: Err(std::io::Error::other("permission denied")),
            },
            &mut state,
            &mut left_entries,
            &mut right_entries,
        );

        assert_eq!(state.left.location, original);
        assert_eq!(left_entries, vec![file("current.txt")]);
        assert_eq!(
            state
                .pane_load_errors
                .get(&Pane::Left)
                .map(|error| &error.attempted),
            Some(&target)
        );

        state.register_pane_load(
            Pane::Left,
            PaneLoadId(2),
            target.clone(),
            PaneLoadPurpose::Navigate {
                remember_current: true,
            },
        );
        apply_pane_load_response(
            PaneLoadResponse {
                id: PaneLoadId(2),
                pane: Pane::Left,
                location: target.clone(),
                purpose: PaneLoadPurpose::Navigate {
                    remember_current: true,
                },
                result: Ok(Vec::new()),
            },
            &mut state,
            &mut left_entries,
            &mut right_entries,
        );

        assert_eq!(state.left.location, target);
        assert!(!state.pane_load_errors.contains_key(&Pane::Left));
    }

    #[test]
    fn empty_hosts_overlay_is_real_and_truthful() {
        let mut state = AppState::default();
        state.hosts.clear();

        toggle_hosts_overlay(&mut state);

        assert_eq!(state.active_overlay(), Some(OverlayKind::Hosts));
        let text = empty_hosts_text();
        assert!(text.contains("~/.config/arx/hosts.toml"));
        assert!(!text.contains("~/.ssh/config"));
    }

    fn workspace_entry(
        name: &str,
        size: u64,
        modified_unix_ms: Option<u64>,
        content_hash: Option<&str>,
    ) -> WorkspaceEntry {
        WorkspaceEntry {
            relative_path: name.into(),
            fingerprint: WorkspaceFingerprint {
                kind: EntryKind::File,
                size: Some(size),
                modified_unix_ms,
                content_hash: content_hash.map(str::to_string),
            },
        }
    }

    fn accept_workspace_compare(
        state: &mut AppState,
        left_entries: Vec<WorkspaceEntry>,
        right_entries: Vec<WorkspaceEntry>,
        generation: u64,
    ) {
        let left = Location::Local("/left".into());
        let right = Location::Local("/right".into());
        state.left.location = left.clone();
        state.right.location = right.clone();
        state.remote_workspace.enabled = true;
        let _ = state.remote_workspace.begin_recursive_scan();
        let left_id = WorkspaceScanId(generation * 2 + 1);
        let right_id = WorkspaceScanId(generation * 2 + 2);
        state
            .remote_workspace
            .register_scan(WorkspaceSide::Left, left_id);
        state
            .remote_workspace
            .register_scan(WorkspaceSide::Right, right_id);

        handle_workspace_scan_response(
            WorkspaceScanResponse {
                id: left_id,
                side: WorkspaceSide::Left,
                root: left,
                result: Ok(left_entries),
            },
            state,
        );
        handle_workspace_scan_response(
            WorkspaceScanResponse {
                id: right_id,
                side: WorkspaceSide::Right,
                root: right,
                result: Ok(right_entries),
            },
            state,
        );
    }

    #[test]
    fn compare_milestone_is_one_per_session_and_uses_runtime_binding() {
        let mut state = AppState::default();
        accept_workspace_compare(
            &mut state,
            vec![workspace_entry("payload.bin", 84, None, None)],
            Vec::new(),
            1,
        );
        assert_eq!(
            state.session_callout,
            Some(SessionCallout::CompareCompleted {
                differences: 1,
                bytes_to_transfer: 84,
            })
        );

        let keymap = Keymap::new(vec![KeyBinding::new(
            InputContext::Browser,
            vec![KeyStroke::new(KeyCode::F(12), KeyModifiers::NONE)],
            Action::PreviewWorkspaceSync,
        )]);
        let text = session_callout_text(&state, &KeyRouter::new(keymap)).unwrap_or_default();
        assert!(text.contains("1 change found"));
        assert!(text.contains("F12 Preview workspace sync"));

        state.dismiss_session_callout();
        accept_workspace_compare(
            &mut state,
            vec![workspace_entry("second.bin", 21, None, None)],
            Vec::new(),
            2,
        );
        assert!(state.session_callout.is_none());
    }

    #[test]
    fn compare_milestone_never_claims_unproven_equality() {
        let mut equal = AppState::default();
        accept_workspace_compare(
            &mut equal,
            vec![workspace_entry("same.bin", 10, None, Some("same"))],
            vec![workspace_entry("same.bin", 10, None, Some("same"))],
            1,
        );
        let equal_text = session_callout_text(&equal, &KeyRouter::default()).unwrap_or_default();
        assert!(equal_text.contains("No proven differences found."));
        assert!(!equal_text.to_lowercase().contains("identical"));
        assert!(!equal_text.contains("changes found"));

        let mut unproven = AppState::default();
        accept_workspace_compare(
            &mut unproven,
            vec![workspace_entry("unknown.bin", 10, None, None)],
            vec![workspace_entry("unknown.bin", 10, None, None)],
            1,
        );
        let unproven_text =
            session_callout_text(&unproven, &KeyRouter::default()).unwrap_or_default();
        assert!(unproven_text.contains("1 change found"));
        assert!(!unproven_text.to_lowercase().contains("identical"));
    }

    fn test_plan_id() -> arx::workspace_sync_execution::SyncPlanId {
        let diff = WorkspaceDiff::compare(
            Location::Local("/left".into()),
            Location::Local("/right".into()),
            vec![workspace_entry("a.txt", 1, None, None)],
            Vec::new(),
        );
        let plan = WorkspaceSyncPlan::build(&diff, arx::workspace_sync::SyncPolicy::default());
        SyncPlanValidator::freeze(&plan, &diff, &arx::vfs::default_registry())
            .expect("freeze test plan")
            .id()
    }

    fn verification_snapshot(
        plan_id: arx::workspace_sync_execution::SyncPlanId,
        status: SyncVerificationStatus,
    ) -> SyncVerificationSnapshot {
        SyncVerificationSnapshot {
            id: SyncVerificationId(1),
            plan_id,
            left_root: Location::Local("/left".into()),
            right_root: Location::Local("/right".into()),
            status,
        }
    }

    fn synchronized_result(
        plan_id: arx::workspace_sync_execution::SyncPlanId,
    ) -> SyncVerificationResult {
        SyncVerificationResult::from_diff(
            plan_id,
            WorkspaceDiff::compare(
                Location::Local("/left".into()),
                Location::Local("/right".into()),
                Vec::new(),
                Vec::new(),
            ),
        )
    }

    fn differences_result(
        plan_id: arx::workspace_sync_execution::SyncPlanId,
    ) -> SyncVerificationResult {
        SyncVerificationResult::from_diff(
            plan_id,
            WorkspaceDiff::compare(
                Location::Local("/left".into()),
                Location::Local("/right".into()),
                vec![workspace_entry("left-only", 1, None, Some("left"))],
                Vec::new(),
            ),
        )
    }

    fn inconclusive_result(
        plan_id: arx::workspace_sync_execution::SyncPlanId,
    ) -> SyncVerificationResult {
        SyncVerificationResult::from_diff(
            plan_id,
            WorkspaceDiff::compare(
                Location::Local("/left".into()),
                Location::Local("/right".into()),
                vec![workspace_entry("unknown", 1, None, None)],
                vec![workspace_entry("unknown", 1, None, None)],
            ),
        )
    }

    fn sync_job(
        plan_id: arx::workspace_sync_execution::SyncPlanId,
        id: &str,
        job_status: JobStatus,
        terminal: SyncTerminalState,
        verification_status: SyncVerificationStatus,
        journal: SyncJournalFinalization,
    ) -> Job {
        let manager = arx::jobs::JobManager::new();
        let mut job = manager.create_job(
            id,
            JobKind::Synchronize,
            "test sync",
            Some(Location::Local("/left".into())),
            Some(Location::Local("/right".into())),
        );
        job.status = job_status;
        job.result = Some(JobResult::WorkspaceSync(SyncExecutionOutcome {
            plan_id,
            completed: Vec::new(),
            terminal,
            remaining: Vec::new(),
            transferred_bytes: 0,
            workspace_may_have_changed: true,
            journal,
        }));
        job.verification = Some(verification_snapshot(plan_id, verification_status));
        job
    }

    #[test]
    fn verified_sync_milestone_requires_completed_and_synchronized() {
        let plan_id = test_plan_id();
        let cases = [
            sync_job(
                plan_id,
                "diff",
                JobStatus::Completed,
                SyncTerminalState::Completed,
                SyncVerificationStatus::Finished(Box::new(differences_result(plan_id))),
                SyncJournalFinalization::Recorded,
            ),
            sync_job(
                plan_id,
                "inconclusive",
                JobStatus::Completed,
                SyncTerminalState::Completed,
                SyncVerificationStatus::Finished(Box::new(inconclusive_result(plan_id))),
                SyncJournalFinalization::Failed {
                    error: "audit warning".into(),
                },
            ),
            sync_job(
                plan_id,
                "verification-failed",
                JobStatus::Completed,
                SyncTerminalState::Completed,
                SyncVerificationStatus::Failed {
                    side: None,
                    error: "scan failed".into(),
                },
                SyncJournalFinalization::Recorded,
            ),
            sync_job(
                plan_id,
                "cancelled",
                JobStatus::Cancelled,
                SyncTerminalState::Cancelled { completed_steps: 0 },
                SyncVerificationStatus::Finished(Box::new(synchronized_result(plan_id))),
                SyncJournalFinalization::Recorded,
            ),
            sync_job(
                plan_id,
                "failed",
                JobStatus::Failed,
                SyncTerminalState::Failed {
                    step: arx::workspace_sync_executor::PhysicalStepId(1),
                    error: arx::workspace_sync_executor::SyncExecutionError::Mutation {
                        path: "a".into(),
                        error: "boom".into(),
                    },
                },
                SyncVerificationStatus::Finished(Box::new(synchronized_result(plan_id))),
                SyncJournalFinalization::Recorded,
            ),
        ];

        for job in cases {
            let mut state = AppState::default();
            observe_verified_sync_success(&mut state, &job);
            assert!(
                state.session_callout.is_none(),
                "unexpected milestone for {}",
                job.id
            );
            assert!(!state.milestones.verified_sync_success_seen);
        }
    }

    #[test]
    fn verified_sync_milestone_is_one_per_session_and_does_not_steal_focus() {
        let plan_id = test_plan_id();
        let first = sync_job(
            plan_id,
            "first",
            JobStatus::Completed,
            SyncTerminalState::Completed,
            SyncVerificationStatus::Finished(Box::new(synchronized_result(plan_id))),
            SyncJournalFinalization::Recorded,
        );
        let second = sync_job(
            plan_id,
            "second",
            JobStatus::Completed,
            SyncTerminalState::Completed,
            SyncVerificationStatus::Finished(Box::new(synchronized_result(plan_id))),
            SyncJournalFinalization::Recorded,
        );
        let mut state = AppState::default();
        let previous_ux = state.remote_workspace.ux.clone();

        observe_verified_sync_success(&mut state, &first);
        assert_eq!(state.remote_workspace.ux, previous_ux);
        assert_eq!(state.active_overlay(), None);
        assert!(matches!(
            state.session_callout,
            Some(SessionCallout::WorkspaceSyncVerified { ref job_id }) if job_id == &first.id
        ));

        state.dismiss_session_callout();
        observe_verified_sync_success(&mut state, &second);
        assert!(state.session_callout.is_none());
    }

    #[test]
    fn verified_sync_callout_embeds_only_in_its_open_finished_overlay() {
        let mut state = AppState::default();
        state.remote_workspace.preview_open = true;
        state.remote_workspace.ux = WorkspaceSyncUxState::Finished {
            job_id: "sync-1".into(),
        };
        state.session_callout = Some(SessionCallout::WorkspaceSyncVerified {
            job_id: "sync-1".into(),
        });

        assert!(session_callout_text(&state, &KeyRouter::default()).is_none());
        state.remote_workspace.ux = WorkspaceSyncUxState::Finished {
            job_id: "sync-other".into(),
        };
        assert!(session_callout_text(&state, &KeyRouter::default()).is_some());
    }
}
