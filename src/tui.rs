use arx::app::{
    Action, ActionAvailability, ActionContext, AppState, CommandItem, CommandKind, CommandTarget,
    OverlayKind, Pane, PaneState, PanelMode, SortMode, WorkspaceSyncUxState, action_availability,
    action_meta, build_command_items,
};
use arx::effect_dispatcher::{EffectDispatcher, EffectLane, EffectResponse, EffectScope};
use arx::effects::{Effect, EffectEvent};
use arx::input::{KeyResolution, KeyRouter};
use arx::services::{
    DesktopService, FileInfoService, GitService, MutationError, MutationService, PaneLoadPurpose,
    PaneLoadResponse, PaneLoader, PreviewService, SyncLaunchId, WorkspaceScanError,
    WorkspaceScanOptions, WorkspaceScanResponse, WorkspaceScanner, WorkspaceSyncController,
};
use arx::vfs::{Entry, EntryKind, Location};
use arx::workspace_sync::{
    DiffState, SyncDirection, SyncMode, WorkspaceSide, WorkspaceSyncOperation,
};
use arx::workspace_sync_execution::SyncPlanId;
use arx::workspace_sync_verification::{
    SyncVerificationEvent, SyncVerificationStatus, SyncVerificationVerdict,
};
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers, MouseButton, MouseEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    DefaultTerminal,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
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
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        crossterm::event::EnableMouseCapture
    )?;
    let mut terminal = ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(stdout))?;

    let result = event_loop(&mut terminal, config).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

#[allow(clippy::collapsible_if)]
async fn event_loop(
    terminal: &mut DefaultTerminal,
    config: arx::config::ArxConfig,
) -> io::Result<()> {
    let editor = config.ui.editor.clone();
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
    let (effect_dispatcher, mut effect_rx) = EffectDispatcher::channel();
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
        let left_filtered = apply_filter(&left_entries, &state.filter);
        let right_filtered = apply_filter(&right_entries, &state.filter);
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
                &left_filtered,
                &right_filtered,
                &mut left_list,
                &mut right_list,
                &mut split_left_list,
                &mut split_right_list,
                &key_router,
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
                handle_effect_response(
                    response,
                    &mut state,
                    &mut left_entries,
                    &mut right_entries,
                    &pane_loader,
                );
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
                    continue;
                }
            }
        };
        if let Some(event) = next_input {
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
                            if row < filt.len() {
                                let name = &filt[row].name;
                                if state.selected.contains(name) {
                                    state.selected.remove(name);
                                } else {
                                    state.selected.insert(name.clone());
                                }
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
                                    let focused_entry = if state.active == Pane::Left {
                                        left_filtered.get(state.left.cursor)
                                    } else {
                                        right_filtered.get(state.right.cursor)
                                    };
                                    if let Some(effect) = execute_command_target(
                                        &mut state,
                                        item.target,
                                        focused_entry.copied(),
                                        &workspace_scanner,
                                        &pane_loader,
                                        &sync_runtime,
                                    ) {
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
                                state.command_matches = build_command_items(&state.filter, &state);
                                state
                                    .overlay_list_state
                                    .select((!state.command_matches.is_empty()).then_some(0));
                            }
                            KeyCode::Char(c)
                                if !key.modifiers.contains(KeyModifiers::CONTROL)
                                    && !key.modifiers.contains(KeyModifiers::ALT) =>
                            {
                                state.filter.push(c);
                                state.command_matches = build_command_items(&state.filter, &state);
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
                                    for e in filt {
                                        state.selected.insert(e.name.clone());
                                    }
                                    state.message =
                                        Some(format!("Selected {}", state.selected.len()));
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
                                    state.cmd_input = false;
                                    if command.is_empty() {
                                        state.message = Some(": command cancelled".into());
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
                                            build_command_items(&state.filter, &state);
                                    }
                                    if state.show_command_center {
                                        state.command_matches =
                                            build_command_items(&state.filter, &state);
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

                    // Hosts panel: F9
                    if state.show_hosts {
                        match key.code {
                            KeyCode::Esc | KeyCode::F(9) => {
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
                            let context = ActionContext::from_state(&state);
                            match action_availability(action.id(), &context) {
                                ActionAvailability::Available => dispatch_ui_action(
                                    &mut state,
                                    action,
                                    entries.get(cursor).copied(),
                                    &workspace_scanner,
                                    &sync_runtime,
                                ),
                                ActionAvailability::Disabled { reason } => {
                                    state.message = Some(reason);
                                }
                                ActionAvailability::Hidden => {}
                            }
                            continue;
                        }
                        KeyResolution::Unhandled => {}
                    }

                    // F7: tmux session attach (before pane borrow)
                    if key.code == KeyCode::F(7) && !state.show_terminal {
                        let id = effect_dispatcher.dispatch(
                            EffectLane::TmuxDiscovery,
                            EffectScope::Global,
                            Effect::ListTmuxSessions,
                        );
                        state.register_effect(EffectLane::TmuxDiscovery, id);
                        state.message = Some("Discovering tmux sessions…".into());
                        continue;
                    }

                    // Handle tree-filter Backspace before borrowing the active pane.
                    if key.code == KeyCode::Backspace && state.show_tree {
                        state.tree_filter.pop();
                        continue;
                    }

                    let pane = state.active_pane_mut();

                    match key.code {
                        KeyCode::Char('q') => state.apply(Action::Quit),
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
                        KeyCode::Char(' ') => {
                            if let Some(entry) = entries.get(cursor) {
                                if state.selected.contains(&entry.name) {
                                    state.selected.remove(&entry.name);
                                } else {
                                    state.selected.insert(entry.name.clone());
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
                                if entry.kind == EntryKind::Directory {
                                    let new_location = pane.location.child(&entry.name);
                                    let active = state.active;
                                    schedule_pane_navigation(
                                        &pane_loader,
                                        &mut state,
                                        active,
                                        new_location,
                                        PaneLoadPurpose::Navigate {
                                            remember_current: true,
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
                            let go_back = match &pane.location {
                                Location::Local(p) => {
                                    let parent = p
                                        .parent()
                                        .map(|p| p.to_path_buf())
                                        .unwrap_or_else(|| p.to_path_buf());
                                    if parent != *p {
                                        Some(Location::Local(parent))
                                    } else {
                                        None
                                    }
                                }
                                Location::Sftp { host, path } => {
                                    // Go to parent directory
                                    if path == "/" {
                                        None
                                    } else {
                                        let parent = path
                                            .rsplit_once('/')
                                            .map(|(p, _)| p.to_string())
                                            .unwrap_or_else(|| "/".to_string());
                                        Some(Location::Sftp {
                                            host: host.clone(),
                                            path: parent,
                                        })
                                    }
                                }
                                Location::Archive {
                                    archive,
                                    inner_path,
                                } => {
                                    if inner_path.is_empty() {
                                        archive.parent().map(|p| Location::Local(p.to_path_buf()))
                                    } else {
                                        // Go up one level
                                        let parent = inner_path
                                            .rsplit_once('/')
                                            .map(|(p, _)| p.to_string())
                                            .unwrap_or_default();
                                        Some(Location::Archive {
                                            archive: archive.clone(),
                                            inner_path: parent,
                                        })
                                    }
                                }
                            };
                            if let Some(new_loc) = go_back {
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
                            state.selected.clear();
                            state.remote_workspace.disable();
                            state.show_diff = false;
                            state.message = Some("Swapped".into());
                            schedule_both_pane_loads(&pane_loader, &mut state);
                        }
                        // F7: create directory
                        KeyCode::F(7) => {
                            state.cmd = "mkdir ".into();
                            state.cmd_input = true;
                        }
                        // Shift+F6: rename file under cursor
                        KeyCode::F(6) if key.modifiers.contains(KeyModifiers::SHIFT) => {
                            if let Some(entry) = entries.get(cursor) {
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
                        // Ctrl+Shift:T: toggle terminal in right pane
                        KeyCode::Char('t')
                            if key.modifiers.contains(KeyModifiers::CONTROL)
                                && key.modifiers.contains(KeyModifiers::SHIFT) =>
                        {
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
                                        state.message =
                                            Some("Terminal started — Esc to close".into());
                                    }
                                    Err(e) => {
                                        state.message = Some(format!("Terminal error: {e}"));
                                    }
                                }
                            }
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
                            disable_raw_mode()?;
                            execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
                            let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
                            let _ = DesktopService::run_interactive_shell(&shell).await;
                            execute!(terminal.backend_mut(), EnterAlternateScreen)?;
                            enable_raw_mode()?;
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
                        // F5: copy — Detection → Planner → Executor
                        KeyCode::F(5) => {
                            let names = selection_or_cursor(&state, entries, cursor);
                            if names.is_empty() {
                                continue;
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
                            let executors = arx::transfer::probe::local_executors(
                                arx::transfer::probe::detect_local_tools(),
                            );
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
                                    continue;
                                }
                            };
                            let job = job_manager.create_job(
                                "copy",
                                arx::jobs::JobKind::Copy,
                                format!("Copy {} → {}", names.join(", "), dst_loc.label()),
                                Some(src_loc.clone()),
                                Some(dst_loc.clone()),
                            );
                            let id = job.id.clone();
                            let cancel = job.cancel.clone();
                            state.jobs = job_manager.snapshot();
                            let jobs = job_manager.clone();
                            let tx = job_tx.clone();
                            let names2 = names.clone();
                            let plan2 = plan.clone();
                            let job_id = id.clone();
                            tokio::spawn(async move {
                                if !jobs.publish_event(
                                    &tx,
                                    arx::jobs::JobEvent::Running { id: job_id.clone() },
                                ) {
                                    return;
                                }
                                let tx2 = tx.clone();
                                let jid = job_id.clone();
                                let result = arx::transfer::executor::execute_transfer(
                                    &plan2,
                                    &names2,
                                    cancel,
                                    |p| {
                                        let pct = p.completed.saturating_mul(100) / p.total.max(1);
                                        let _ = jobs.publish_event(
                                            &tx2,
                                            arx::jobs::JobEvent::Progress {
                                                id: jid.clone(),
                                                progress: arx::jobs::Progress::Percent(pct as u8)
                                                    .into(),
                                            },
                                        );
                                    },
                                )
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
                                    Err(
                                        arx::transfer::executor::TransferExecutionError::Cancelled {
                                            completed,
                                        },
                                    ) => {
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
                            state.selected.clear();
                        }
                        // F6: move — Detection → Planner → Executor
                        KeyCode::F(6) => {
                            let names = selection_or_cursor(&state, entries, cursor);
                            if names.is_empty() {
                                continue;
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
                            let executors = arx::transfer::probe::local_executors(
                                arx::transfer::probe::detect_local_tools(),
                            );
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
                                    continue;
                                }
                            };
                            let job = job_manager.create_job(
                                "move",
                                arx::jobs::JobKind::Move,
                                format!("Move {} → {}", names.join(", "), dst_loc.label()),
                                Some(src_loc.clone()),
                                Some(dst_loc.clone()),
                            );
                            let id = job.id.clone();
                            let cancel = job.cancel.clone();
                            state.jobs = job_manager.snapshot();
                            let jobs = job_manager.clone();
                            let tx = job_tx.clone();
                            let names2 = names.clone();
                            let plan2 = plan.clone();
                            let job_id = id.clone();
                            tokio::spawn(async move {
                                if !jobs.publish_event(
                                    &tx,
                                    arx::jobs::JobEvent::Running { id: job_id.clone() },
                                ) {
                                    return;
                                }
                                let tx2 = tx.clone();
                                let jid = job_id.clone();
                                let result = arx::transfer::executor::execute_transfer(
                                    &plan2,
                                    &names2,
                                    cancel,
                                    |p| {
                                        let pct = p.completed.saturating_mul(100) / p.total.max(1);
                                        let _ = jobs.publish_event(
                                            &tx2,
                                            arx::jobs::JobEvent::Progress {
                                                id: jid.clone(),
                                                progress: arx::jobs::Progress::Percent(pct as u8)
                                                    .into(),
                                            },
                                        );
                                    },
                                )
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
                                    Err(
                                        arx::transfer::executor::TransferExecutionError::Cancelled {
                                            completed,
                                        },
                                    ) => {
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
                            state.selected.clear();
                        }
                        // F8: delete selected (or cursor) from active pane
                        // F8: delete selected (or cursor) from active pane
                        KeyCode::F(8) => {
                            let names = selection_or_cursor(&state, entries, cursor);
                            if names.is_empty() {
                                continue;
                            }
                            let Location::Local(dir) = state.active_pane().location.clone() else {
                                state.message = Some(
                                    "Trash is currently available for local files only".into(),
                                );
                                continue;
                            };
                            let job = job_manager.create_job(
                                "trash",
                                arx::jobs::JobKind::Delete,
                                format!("Trash {}", names.join(", ")),
                                Some(Location::Local(dir.clone())),
                                None,
                            );
                            let id = job.id.clone();
                            let cancel = job.cancel.clone();
                            state.jobs = job_manager.snapshot();
                            let jobs = job_manager.clone();
                            let tx = job_tx.clone();
                            let job_id = id.clone();
                            tokio::spawn(async move {
                                if !jobs.publish_event(
                                    &tx,
                                    arx::jobs::JobEvent::Running { id: job_id.clone() },
                                ) {
                                    return;
                                }

                                let tx_progress = tx.clone();
                                let progress_id = job_id.clone();
                                let progress_jobs = jobs.clone();
                                let result = MutationService::trash_local(
                                    dir,
                                    names,
                                    cancel,
                                    move |progress| {
                                        let percent = progress.completed.saturating_mul(100)
                                            / progress.total.max(1);
                                        let _ = progress_jobs.publish_event(
                                            &tx_progress,
                                            arx::jobs::JobEvent::Progress {
                                                id: progress_id.clone(),
                                                progress: arx::jobs::Progress::Percent(
                                                    percent as u8,
                                                )
                                                .into(),
                                            },
                                        );
                                    },
                                )
                                .await;

                                match result {
                                    Ok(outcome) => {
                                        let _ = jobs.publish_event(
                                            &tx,
                                            arx::jobs::JobEvent::Completed {
                                                id: job_id,
                                                result: arx::jobs::JobResult::generic(
                                                    format!(
                                                        "Trashed {} item(s)",
                                                        outcome.completed
                                                    ),
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

                            state.selected.clear();
                            state.message = Some(format!("Trash queued ({id})"));
                        }

                        // *: invert selection on visible entries
                        KeyCode::Char('*') => {
                            let filt = if state.active == Pane::Left {
                                &left_filtered
                            } else {
                                &right_filtered
                            };
                            for e in filt {
                                if state.selected.contains(&e.name) {
                                    state.selected.remove(&e.name);
                                } else {
                                    state.selected.insert(e.name.clone());
                                }
                            }
                            state.message = Some(format!("Selected {}", state.selected.len()));
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
                        // F3: view file
                        KeyCode::F(3) => {
                            if let Some(entry) = entries.get(cursor) {
                                if entry.kind != EntryKind::Directory {
                                    let path = match &pane.location {
                                        Location::Local(dir) => dir.join(&entry.name),
                                        _ => continue,
                                    };
                                    if key.modifiers.contains(KeyModifiers::SHIFT) {
                                        let _ = DesktopService::page_with_bat(&path).await;
                                    } else {
                                        state.viewer_content = PreviewService::preview(&path).await;
                                        state.viewer_scroll = 0;
                                    }
                                }
                            }
                        }
                        // F4: edit file in $EDITOR
                        KeyCode::F(4) => {
                            if let Some(entry) = entries.get(cursor) {
                                let path = match &pane.location {
                                    Location::Local(dir) => dir.join(&entry.name),
                                    _ => continue,
                                };
                                let editor_cmd = editor
                                    .clone()
                                    .or_else(|| std::env::var("EDITOR").ok())
                                    .or_else(|| std::env::var("VISUAL").ok())
                                    .unwrap_or_else(|| "vi".into());
                                // Leave raw mode, spawn editor, restore
                                disable_raw_mode()?;
                                execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
                                let _ = DesktopService::open_editor(&editor_cmd, &path).await;
                                execute!(terminal.backend_mut(), EnterAlternateScreen)?;
                                enable_raw_mode()?;
                                // Refresh after edit
                                schedule_active_pane_load(&pane_loader, &mut state);
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
                        // F9: host panel
                        KeyCode::F(9) => {
                            if !state.hosts.is_empty() {
                                state.show_hosts = !state.show_hosts;
                                state.host_cursor = 0;
                            } else {
                                state.message = Some(
                                    "No hosts configured — add ~/.config/arx/hosts.toml".into(),
                                );
                            }
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
                                state.selected.clear();
                                state.remote_workspace.disable();
                                state.show_diff = false;
                                schedule_active_pane_load(&pane_loader, &mut state);
                            }
                        }
                        // Ctrl+T: new tab in active pane
                        KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            state.active_pane_mut().new_tab();
                            let tabs = state.active_pane().tabs.len() + 1;
                            state.selected.clear();
                            state.remote_workspace.disable();
                            state.show_diff = false;
                            schedule_active_pane_load(&pane_loader, &mut state);
                            state.message = Some(format!("Tab {tabs}/{tabs}"));
                        }
                        // Ctrl+W: close tab in active pane
                        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            state.active_pane_mut().close_tab();
                            let tabs = state.active_pane().tabs.len() + 1;
                            state.selected.clear();
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
                                state.selected.clear();
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
                            state.selected.clear();
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
            if active && response.purpose != PaneLoadPurpose::Refresh {
                state.selected.clear();
                state.remote_workspace.disable();
                state.show_diff = false;
            }
        }
        Err(error) => {
            // Transactional navigation: current pane location is intentionally
            // untouched on error.
            state.message = Some(format!("{}: {error}", response.location));
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

fn selection_or_cursor(state: &AppState, entries: &[&Entry], cursor: usize) -> Vec<String> {
    if !state.selected.is_empty() {
        state.selected.iter().cloned().collect()
    } else if let Some(entry) = entries.get(cursor) {
        vec![entry.name.clone()]
    } else {
        vec![]
    }
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
    left_entries: &[&Entry],
    right_entries: &[&Entry],
    left_list: &mut ListState,
    right_list: &mut ListState,
    split_left_list: &mut ListState,
    split_right_list: &mut ListState,
    key_router: &KeyRouter,
    message: Option<&str>,
) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
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
            left_list,
            act1,
            &state.selected,
            &left_only,
            state.panel_mode,
        );
        render_pane(
            frame,
            a2,
            &state.left,
            left_entries,
            split_left_list,
            act2,
            &state.selected,
            &left_only,
            state.panel_mode,
        );
    } else {
        render_pane(
            frame,
            panes[0],
            &state.left,
            left_entries,
            left_list,
            state.active == Pane::Left,
            &state.selected,
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
                right_list,
                act1,
                &state.selected,
                &right_only,
                state.panel_mode,
            );
            render_pane(
                frame,
                a2,
                &state.right,
                right_entries,
                split_right_list,
                act2,
                &state.selected,
                &right_only,
                state.panel_mode,
            );
        } else {
            render_pane(
                frame,
                panes[1],
                &state.right,
                right_entries,
                right_list,
                state.active == Pane::Right,
                &state.selected,
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

    // Status bar
    let pane = state.active_pane();
    let loc_str = match &pane.location {
        Location::Local(p) => p.display().to_string(),
        other => other.to_string(),
    };
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
    let hidden = if state.show_hidden { " [dot]" } else { "" };
    let tab_info = format!(
        " | LT:{} RT:{}",
        state.left.tabs.len() + 1,
        state.right.tabs.len() + 1
    );
    let git_info = state.git_status.as_str();
    let msg_hint = message.map(|m| format!(" | {m}")).unwrap_or_default();
    let workspace_hint = if state.remote_workspace.enabled {
        format!(" | {}", state.remote_workspace.summary())
    } else {
        String::new()
    };
    let status = Paragraph::new(Line::from(format!(
        "ARX v0.1.0 | {}{hidden}{tab_info} | sel: {} |{hint}{msg_hint}{git_info}{workspace_hint} | ?: help",
        loc_str,
        state.selected.len(),
    )))
    .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(status, chunks[1]);

    // Bottom F-key shortcut bar
    let shortcuts = Span::styled(
        "1Help  2Menu  3View  4Edit  5Copy  6RenMov  7Mkdir  8Delete  9Hosts  10Quit",
        Style::default().fg(Color::Black).bg(Color::DarkGray),
    );
    frame.render_widget(Paragraph::new(shortcuts), chunks[2]);

    if state.active_overlay() == Some(OverlayKind::SyncPreview) {
        render_sync_preview(frame, area, state);
    }
}

fn render_sync_preview(frame: &mut ratatui::Frame, area: Rect, state: &AppState) {
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
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Esc hides this view. A newer compare/direction/mode action supersedes preparation.",
                Style::default().fg(Color::DarkGray),
            )));
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
        WorkspaceSyncUxState::VerificationDiff { job_id } => {
            title = " Post-sync Verification Diff ".into();
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
                let can_return_to_preview =
                    ActionContext::from_state(state).sync_return_preview_ready;
                render_sync_job_lines(
                    job,
                    &state.remote_workspace.ux,
                    can_return_to_preview,
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
    lines.push(Line::from(format!("Create dirs       {create_dirs}")));
    lines.push(Line::from(format!("Delete            {deletes}")));
    lines.push(Line::from(format!("Conflicts         {}", plan.conflicts)));
    lines.push(Line::from(format!(
        "Transfer          {}",
        format_size(plan.bytes_to_transfer)
    )));
    lines.push(Line::from(""));
    if plan.destructive_operations == 0 && plan.policy.mode == SyncMode::Update {
        lines.push(Line::from(
            "Safe update — destination-only entries are preserved.",
        ));
    } else if plan.destructive_operations == 0 {
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
    can_return_to_preview: bool,
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
                lines.push(Line::from(error.to_string()));
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
        WorkspaceSyncUxState::Finished { .. }
            if verification_has_differences(job) && can_return_to_preview =>
        {
            "V verification diff   B current preview   Esc hide"
        }
        WorkspaceSyncUxState::Finished { .. } if verification_has_differences(job) => {
            "V verification diff   Esc hide · current panes moved"
        }
        WorkspaceSyncUxState::Finished { .. } if can_return_to_preview => {
            "B current preview   Esc hide"
        }
        WorkspaceSyncUxState::Finished { .. } => {
            "Esc hide · compare current panes for a new preview"
        }
        _ => "Esc hide",
    };
    lines.push(Line::from(Span::styled(
        footer,
        Style::default().fg(Color::DarkGray),
    )));
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

    lines.push(Line::from(format!("LEFT   {}", result.left_root)));
    lines.push(Line::from(format!("RIGHT  {}", result.right_root)));
    lines.push(Line::from(""));
    lines.push(Line::from(format!(
        "{} proven difference(s) · {} conflict(s) · {} unverified",
        result.changed_entries, result.conflicts, result.unverified_entries
    )));
    lines.push(Line::from(Span::styled(
        "This is the recursive post-sync verification snapshot for this Job.",
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::from(""));

    let visible = result
        .diff
        .entries
        .iter()
        .filter(|entry| entry.state != DiffState::SameFingerprint)
        .collect::<Vec<_>>();
    for entry in visible.iter().take(40) {
        let label = match entry.state {
            DiffState::OnlyLeft => "LEFT ONLY",
            DiffState::OnlyRight => "RIGHT ONLY",
            DiffState::LeftNewer => "LEFT NEWER",
            DiffState::RightNewer => "RIGHT NEWER",
            DiffState::Different => "COMPARE",
            DiffState::SameFingerprint => continue,
        };
        lines.push(Line::from(format!("{label:>11}  {}", entry.relative_path)));
    }
    if visible.len() > 40 {
        lines.push(Line::from(format!(
            "… {} more verification entry/entries",
            visible.len() - 40
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "B back to Job result   Esc hide",
        Style::default().fg(Color::DarkGray),
    )));
}

fn verification_has_differences(job: &arx::jobs::Job) -> bool {
    job.verification.as_ref().is_some_and(|verification| {
        matches!(
            &verification.status,
            SyncVerificationStatus::Finished(result)
                if matches!(
                    &result.verdict,
                    SyncVerificationVerdict::DifferencesRemain { .. }
                )
        )
    })
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
                lines.push(Line::from(
                    "Preview the next sync to resolve current differences.",
                ));
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
            lines.push(Line::from(
                "Verification superseded by a newer workspace state.",
            ));
        }
        SyncVerificationStatus::Pending | SyncVerificationStatus::Running { .. } => {
            lines.push(Line::from("Verifying current workspace…"));
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

fn render_hosts(frame: &mut ratatui::Frame, area: Rect, state: &AppState) {
    if state.hosts.is_empty() {
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

#[allow(clippy::too_many_arguments)]
fn render_pane(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    pane: &PaneState,
    entries: &[&Entry],
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

fn dispatch_ui_action(
    state: &mut AppState,
    action: Action,
    focused: Option<&Entry>,
    workspace_scanner: &WorkspaceScanner,
    sync: &SyncUiRuntime,
) {
    if matches!(
        action,
        Action::ToggleWorkspaceComparison
            | Action::PreviewWorkspaceSync
            | Action::ReverseWorkspaceDirection
            | Action::ToggleWorkspaceSyncMode
    ) && !supersede_workspace_launch_for_new_action(state, sync)
    {
        return;
    }

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
        _ => state.apply(action),
    }
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

fn execute_command_target(
    state: &mut AppState,
    target: CommandTarget,
    focused: Option<&Entry>,
    workspace_scanner: &WorkspaceScanner,
    pane_loader: &PaneLoader,
    sync: &SyncUiRuntime,
) -> Option<Effect> {
    match target {
        CommandTarget::Action(action) => {
            dispatch_ui_action(state, action, focused, workspace_scanner, sync);
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
        state.message = Some(state.remote_workspace.summary());
    } else {
        let waiting = match side {
            WorkspaceSide::Left => "right",
            WorkspaceSide::Right => "left",
        };
        state.message = Some(format!("Remote Workspace: waiting for {waiting} pane…"));
    }
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
    state.finish_effect(response.lane, response.id);
    apply_effect_event(state, response.event);

    // Pure process responses normally do not require directory refresh. Pane
    // and workspace effect lanes added later can opt into targeted refresh.
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

fn truncate_message(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let prefix: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{prefix}...")
    } else {
        prefix
    }
}
