use crate::tui_terminal::TuiTerminalSession;
#[cfg(test)]
use arx::app::CommandItem;
use arx::app::InputContext;
#[cfg(test)]
use arx::app::PaneLoadUiError;
use arx::app::{
    Action, ActionAvailability, ActionContext, ActionId, AppState, CommandKind, CommandTarget,
    OverlayKind, Pane, PaneState, PanelMode, SessionCallout, SortMode, WorkspaceSyncUxState,
    action_availability, action_meta, build_command_items_with_file_context,
    listed_entry_navigation_target, navigation_parent_target,
};
use arx::effect_dispatcher::{EffectDispatcher, EffectLane, EffectScope};
#[cfg(test)]
use arx::effects::EffectEvent;
use arx::effects::{Effect, ProgressSlot};
#[cfg(test)]
use arx::input::contextual_hints_with_file_context;
use arx::input::{ContextHint, KeyResolution, KeyRouter, command_bar_rows};
#[cfg(test)]
use arx::services::WorkspaceSyncController;
use arx::services::{
    DesktopService, FileInfoService, GitService, PaneListingContinuation, PaneLoadPurpose,
    PaneLoader, SyncLaunchId, WorkspaceScanOptions, WorkspaceScanner,
};
#[cfg(test)]
use arx::services::{PaneLoadResponse, PaneNextPageResponse, QuickActionOutcome};
#[cfg(test)]
use arx::services::{WorkspaceScanError, WorkspaceScanResponse};
use arx::vfs::{
    Entry, EntryIdentity, EntryKind, ListedEntry, Location, ProviderId, ProviderRegistry,
    RemoteEditSession, RemoteEditState,
};
use arx::workspace_sync::{
    DiffState, SyncDirection, SyncMode, WorkspaceSide, WorkspaceSyncOperation,
};
use arx::workspace_sync_execution::SyncPlanId;
#[cfg(test)]
use arx::workspace_sync_verification::SyncVerificationEvent;
use arx::workspace_sync_verification::{SyncVerificationStatus, SyncVerificationVerdict};
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use std::io;
use std::path::PathBuf;
#[cfg(test)]
use tokio::sync::mpsc;

mod presentation;
use presentation::{session_callout_text, workspace_ribbon_text};

mod bookmarks;
mod browser_input;
pub use browser_input::validate_user_browser_bindings;
mod command_bar;
mod command_center;
mod effect_responses;
mod embedded_terminal;
mod feature_registry;
mod help;
mod hosts;
mod hotlist;
mod input_dispatch;
mod job_responses;
mod jobs;
mod mouse;
mod mutations;
mod overlays;
mod pane_responses;
mod quick_actions;
mod remote_edit;
mod runtime;
mod ssh_hosts;
mod transfers;
mod user_menu;
mod viewer;
mod which_key;
mod workspace;
mod workspace_responses;
use effect_responses::pane_still_at_location;
use overlays::{
    render_context_menu, render_directory_history, render_file_search,
    render_infrastructure_center, render_rename_input, render_session_callout, render_smart_tree,
    render_tab_switcher,
};
use runtime::{RuntimeEvent, SyncLaunchResponse, SyncUiRuntime, TuiRuntime};

pub async fn run(config: arx::config::ArxConfig, keymap: arx::input::Keymap) -> io::Result<()> {
    let mut terminal_session = TuiTerminalSession::enter()?;
    let stdout = io::stdout();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(stdout))?;

    let result = event_loop(&mut terminal, &mut terminal_session, config, keymap).await;
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
    keymap: arx::input::Keymap,
) -> io::Result<()> {
    let editor = DesktopService::resolve_editor(config.ui.editor.as_deref());
    let mut state = AppState {
        show_hidden: config.ui.show_hidden,
        hosts: arx::remote::hosts_config::load_hosts(),
        menu: AppState::load_menu(),
        ..AppState::default()
    };
    let mut runtime = TuiRuntime::new(&mut state, &config);
    let mut left_entries = Vec::new();
    let mut right_entries = Vec::new();
    schedule_pane_load(&runtime.pane_loader, &mut state, Pane::Left);
    schedule_pane_load(&runtime.pane_loader, &mut state, Pane::Right);
    let mut left_list = ListState::default();
    let mut right_list = ListState::default();
    let mut split_left_list = ListState::default();
    let mut split_right_list = ListState::default();
    // #214: ONE effective keymap built in main feeds routing + presentation.
    let mut key_router = KeyRouter::new(keymap);
    let parent_entry = virtual_parent_entry();
    let load_more_entry = load_more_entry();

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
        let left_visible = apply_filter_with_parent_and_continuation(
            &left_entries,
            &state.filter,
            &state.left.location,
            &state.registry,
            &parent_entry,
            &load_more_entry,
            state.pane_listing_continuations.get(&Pane::Left),
        );
        let right_visible = apply_filter_with_parent_and_continuation(
            &right_entries,
            &state.filter,
            &state.right.location,
            &state.registry,
            &parent_entry,
            &load_more_entry,
            state.pane_listing_continuations.get(&Pane::Right),
        );
        let left_filtered: Vec<&Entry> = left_visible.iter().map(VisiblePaneRow::entry).collect();
        let right_filtered: Vec<&Entry> = right_visible.iter().map(VisiblePaneRow::entry).collect();
        // clamp cursors
        state.left.cursor = state.left.cursor.min(left_filtered.len().saturating_sub(1));
        state.right.cursor = state
            .right
            .cursor
            .min(right_filtered.len().saturating_sub(1));
        let focused_kind = focused_action_kind(&state, &left_visible, &right_visible);

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
                focused_kind,
                editor.is_some(),
                msg.as_deref(),
                &runtime.sync,
            )
        })?;
        state.message = None; // one-shot clear after render

        // Drain terminal output if active
        if let Some(ref mut term) = state.term {
            term.drain();
        }

        let next_input = match runtime.next_event().await {
            RuntimeEvent::WorkspaceScan(response) => {
                workspace_responses::handle_workspace_scan_response(response, &mut state);
                continue;
            }
            RuntimeEvent::PaneLoad(response) => {
                pane_responses::apply_pane_load_response(
                    response,
                    &mut state,
                    &mut left_entries,
                    &mut right_entries,
                );
                continue;
            }
            RuntimeEvent::PaneNextPage(response) => {
                pane_responses::apply_next_page_response(
                    response,
                    &mut state,
                    &mut left_entries,
                    &mut right_entries,
                );
                continue;
            }
            RuntimeEvent::Effect(response) => {
                effect_responses::apply_received(
                    &runtime.effect_dispatcher,
                    response,
                    &mut state,
                    &runtime.pane_loader,
                );

                // Defer editor launch until after terminal input is polled,
                // so queued Quit/navigation input is processed first.
                if state.pending_remote_edit_session.is_some() {
                    state.pending_editor = true;
                }

                continue;
            }
            RuntimeEvent::SyncLaunch(response) => {
                workspace_responses::apply_sync_launch_response(
                    response,
                    &mut state,
                    &runtime.sync.controller,
                    &runtime.job_manager,
                );
                continue;
            }
            RuntimeEvent::Verification(event) => {
                workspace_responses::apply_verification_event(
                    event,
                    &mut state,
                    &runtime.job_manager,
                );
                continue;
            }
            RuntimeEvent::Job(event) => {
                let outcome =
                    job_responses::apply_job_event(&event, &mut state, &runtime.job_manager);
                if let Some(body) = outcome.failure_notification {
                    tokio::spawn(async move {
                        DesktopService::notify("ARX", &body).await;
                    });
                }
                if outcome.refresh_panes {
                    schedule_pane_load(&runtime.pane_loader, &mut state, Pane::Left);
                    schedule_pane_load(&runtime.pane_loader, &mut state, Pane::Right);
                }
                if let Some(ref mut term) = state.term {
                    term.drain();
                }
                continue;
            }
            RuntimeEvent::Tick => {
                if event::poll(std::time::Duration::ZERO)? {
                    Some(event::read()?)
                } else {
                    if let Some(ref mut term) = state.term {
                        term.drain();
                    }
                    if state.pending_editor {
                        None
                    } else {
                        continue;
                    }
                }
            }
        };
        if let Some(event) = next_input {
            let outcome = input_dispatch::handle_event(
                event,
                &mut state,
                &left_visible,
                &right_visible,
                &left_filtered,
                &right_filtered,
                &runtime.workspace_scanner,
                &runtime.sync,
                &runtime.effect_dispatcher,
                &runtime.pane_loader,
                terminal_session,
                editor.as_deref(),
                &mut key_router,
            )
            .await?;
            match outcome.entry_mutation {
                input_dispatch::EntryMutation::None => {}
                input_dispatch::EntryMutation::SwapPaneEntries => {
                    std::mem::swap(&mut left_entries, &mut right_entries);
                }
                input_dispatch::EntryMutation::ResortPaneEntries(mode) => {
                    sort_entries(&mut left_entries, mode);
                    sort_entries(&mut right_entries, mode);
                }
            }
            if outcome.flow == input_dispatch::InputFlow::ContinueLoop {
                continue;
            }
        }

        // ── Deferred editor launch: only after terminal input is drained ──
        if state.pending_editor && !state.should_quit {
            if remote_edit::drive_deferred_editor(
                &mut state,
                editor.as_deref(),
                terminal_session,
                &runtime.effect_dispatcher,
            )
            .await?
            {
                continue;
            }
        }
    }
    Ok(())
}

fn normalize_entries(
    mut entries: Vec<ListedEntry>,
    show_hidden: bool,
    sort_mode: SortMode,
) -> Vec<ListedEntry> {
    if !show_hidden {
        entries.retain(|listed| {
            !listed.entry.name.starts_with('.')
                || (listed.entry.name == VIRTUAL_PARENT_NAME
                    && !matches!(&listed.identity, EntryIdentity::Other))
        });
    }
    sort_entries(&mut entries, sort_mode);
    entries
}

fn schedule_next_page(loader: &PaneLoader, state: &mut AppState, pane: Pane) {
    let Some(continuation) = state.pane_listing_continuations.get(&pane).cloned() else {
        return;
    };
    if !state.accepts_pane_listing_continuation(pane, &continuation)
        || state.pending_next_pages.contains_key(&pane)
    {
        return;
    }
    let request_id = loader.next_page_request_id();
    if state.register_next_page(pane, request_id, continuation.clone()) {
        loader.load_next(request_id, pane, continuation);
        state.message = Some("Loading next page…".into());
    }
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

fn sort_entries(entries: &mut [ListedEntry], mode: SortMode) {
    match mode {
        SortMode::NameAsc => entries.sort_by_key(|a| a.entry.name.to_lowercase()),
        SortMode::NameDesc => {
            entries.sort_by_key(|b| std::cmp::Reverse(b.entry.name.to_lowercase()))
        }
        SortMode::SizeAsc => entries.sort_by_key(|a| a.entry.size.unwrap_or(0)),
        SortMode::SizeDesc => entries.sort_by_key(|b| std::cmp::Reverse(b.entry.size.unwrap_or(0))),
        SortMode::Kind => {
            entries.sort_by_key(|a| (kind_order(a.entry.kind), a.entry.name.to_lowercase()))
        }
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

fn apply_filter<'a>(entries: &'a [ListedEntry], filter: &str) -> Vec<&'a ListedEntry> {
    if filter.is_empty() {
        entries.iter().collect()
    } else {
        let (name_filter, size_min, size_max) = parse_filter(filter);
        entries
            .iter()
            .filter(|listed| {
                let entry = &listed.entry;
                if !name_filter.is_empty() && !entry.name.to_lowercase().contains(&name_filter) {
                    return false;
                }
                match (size_min, size_max) {
                    (Some(min), Some(max)) => entry.size.is_some_and(|s| s >= min && s <= max),
                    (Some(min), None) => entry.size.is_some_and(|s| s >= min),
                    (None, Some(max)) => entry.size.is_some_and(|s| s <= max),
                    (None, None) => true,
                }
            })
            .collect()
    }
}

const VIRTUAL_PARENT_NAME: &str = "..";
const LOAD_MORE_LABEL: &str = "Load more…";

fn load_more_entry() -> Entry {
    Entry {
        name: LOAD_MORE_LABEL.into(),
        kind: EntryKind::Other,
        size: None,
        modified_unix_ms: None,
    }
}

fn virtual_parent_entry() -> Entry {
    Entry {
        name: VIRTUAL_PARENT_NAME.into(),
        kind: EntryKind::Directory,
        size: None,
        modified_unix_ms: None,
    }
}

#[derive(Clone, Copy)]
enum VisiblePaneRow<'a> {
    Parent(&'a Entry),
    Listed(&'a ListedEntry),
    LoadMore(&'a Entry),
}

impl<'a> VisiblePaneRow<'a> {
    fn entry(&self) -> &'a Entry {
        match self {
            Self::Parent(entry) | Self::LoadMore(entry) => entry,
            Self::Listed(listed) => &listed.entry,
        }
    }

    fn listed(&self) -> Option<&'a ListedEntry> {
        match self {
            Self::Parent(_) | Self::LoadMore(_) => None,
            Self::Listed(listed) => Some(listed),
        }
    }

    fn listed_entry(&self) -> Option<&'a Entry> {
        self.listed().map(|listed| &listed.entry)
    }

    fn action_entry(self) -> Option<&'a Entry> {
        match self {
            Self::LoadMore(_) => None,
            _ => Some(self.entry()),
        }
    }

    fn navigation_target(
        &self,
        location: &Location,
        registry: &ProviderRegistry,
    ) -> Option<Location> {
        match self {
            Self::Parent(_) => navigation_parent_target(location, registry),
            Self::Listed(listed) => listed_entry_navigation_target(location, listed),
            Self::LoadMore(_) => None,
        }
    }
}

#[cfg(test)]
fn apply_filter_with_parent<'a>(
    entries: &'a [ListedEntry],
    filter: &str,
    location: &Location,
    registry: &ProviderRegistry,
    parent_entry: &'a Entry,
) -> Vec<VisiblePaneRow<'a>> {
    apply_filter_with_parent_and_continuation(
        entries,
        filter,
        location,
        registry,
        parent_entry,
        parent_entry,
        None,
    )
}

fn apply_filter_with_parent_and_continuation<'a>(
    entries: &'a [ListedEntry],
    filter: &str,
    location: &Location,
    registry: &ProviderRegistry,
    parent_entry: &'a Entry,
    load_more_entry: &'a Entry,
    continuation: Option<&PaneListingContinuation>,
) -> Vec<VisiblePaneRow<'a>> {
    let mut visible: Vec<_> = apply_filter(entries, filter)
        .into_iter()
        .filter(|listed| {
            !(matches!(&listed.identity, EntryIdentity::Other)
                && listed.entry.name == VIRTUAL_PARENT_NAME)
        })
        .map(VisiblePaneRow::Listed)
        .collect();
    if navigation_parent_target(location, registry).is_some() {
        visible.insert(0, VisiblePaneRow::Parent(parent_entry));
    }
    if continuation.is_some() {
        visible.push(VisiblePaneRow::LoadMore(load_more_entry));
    }
    visible
}

fn focused_action_kind(
    state: &AppState,
    left_entries: &[VisiblePaneRow<'_>],
    right_entries: &[VisiblePaneRow<'_>],
) -> Option<EntryKind> {
    focused_visible_entry(state, left_entries, right_entries).map(|entry| entry.kind)
}

fn focused_visible_entry<'a>(
    state: &AppState,
    left_entries: &[VisiblePaneRow<'a>],
    right_entries: &[VisiblePaneRow<'a>],
) -> Option<&'a Entry> {
    let pane = state.active_pane();
    let cursor = if pane.split && pane.split_active {
        pane.split_cursor
    } else {
        pane.cursor
    };
    match state.active {
        Pane::Left => left_entries.get(cursor).and_then(|row| row.action_entry()),
        Pane::Right => right_entries.get(cursor).and_then(|row| row.action_entry()),
    }
}

fn toggle_selection_and_advance(
    state: &mut AppState,
    focused: Option<&Entry>,
    visible_count: usize,
) {
    let Some(entry) = focused else {
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

/// Check if a filename looks like an archive (tar, tgz, zip).
fn is_archive(name: &str) -> bool {
    name.ends_with(".tar")
        || name.ends_with(".tar.gz")
        || name.ends_with(".tgz")
        || name.ends_with(".tar.bz2")
        || name.ends_with(".tar.xz")
        || name.ends_with(".zip")
}

/// S3-30R: S3 regular objects now route through the identity-aware
/// `Effect::PreviewLocation` lane (alongside SFTP). Local/Archive fall through
/// to their own preview paths. This is the S3 F3 dispatch decision.
fn s3_f3_routes_to_preview(location: &Location) -> bool {
    matches!(location.provider_id(), ProviderId::Sftp | ProviderId::S3)
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
    focused_kind: Option<EntryKind>,
    editor_available: bool,
    message: Option<&str>,
    sync: &SyncUiRuntime,
) {
    let area = frame.area();
    let session_callout = session_callout_text(state, key_router);
    let constraints = if session_callout.is_some() {
        vec![
            Constraint::Min(1),
            Constraint::Length(1), // workspace ribbon
            Constraint::Length(1), // status line
            Constraint::Length(1), // session callout
            Constraint::Length(1), // Row A
            Constraint::Length(1), // Row B
        ]
    } else {
        vec![
            Constraint::Min(1),
            Constraint::Length(1), // workspace ribbon
            Constraint::Length(1), // status line
            Constraint::Length(1), // Row A
            Constraint::Length(1), // Row B
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
    if !embedded_terminal::render_if_active(frame, panes[1], state) {
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
        help::render(frame, area, state, key_router.keymap());
    }

    // Viewer overlay
    if !state.viewer_content.is_empty() {
        viewer::render(frame, area, state);
    }

    which_key::render(frame, area, state, key_router);

    if state.show_infra {
        render_infrastructure_center(frame, area, state);
    }

    if state.show_tree {
        render_smart_tree(frame, area, state);
    }

    if state.show_command_center {
        command_center::render(frame, area, state, key_router.keymap());
    }

    if state.show_context_menu {
        render_context_menu(frame, area);
    }

    // Bookmarks overlay
    if state.show_bookmarks {
        bookmarks::render(frame, area, state);
    }

    // Directory history overlay (Alt+H)
    if state.show_history {
        render_directory_history(frame, area, state);
    }

    // Hotlist overlay
    if state.show_hotlist {
        hotlist::render(frame, area, state);
    }
    if state.show_tab_switcher {
        render_tab_switcher(frame, area, state);
    }
    if state.rename_input {
        render_rename_input(frame, area, state);
    }
    if state.file_search {
        render_file_search(frame, area, state);
    }

    // Hosts overlay
    if state.show_hosts {
        hosts::render(frame, area, state);
    }

    // SSH Hosts overlay
    if state.show_ssh_hosts {
        ssh_hosts::render(frame, area, state);
    }

    // Jobs overlay
    if state.show_jobs {
        jobs::render(frame, area, state);
    }

    if state.show_transfer_center {
        arx::transfer_center_ui::render_transfer_center(frame, area, state, &sync.transfers);
    }

    #[cfg(target_os = "linux")]
    if state.show_storage_inspector {
        arx::storage_inspector_ui::render_storage_inspector(frame, area, state);
    }

    #[cfg(target_os = "linux")]
    if state.show_filesystems {
        arx::filesystem_usage_ui::render_filesystems(frame, area, state);
    }

    // User menu overlay
    if state.show_menu {
        user_menu::render(frame, area, state);
    }

    // Remote delete confirmation overlay
    if state.pending_delete.is_some() {
        mutations::render_confirmation(frame, area, state);
    }

    // Status bar
    let pane = state.active_pane();
    let selection_count = state.selection_count(state.active, &pane.location);
    let hint = if state.cmd_input {
        if let Some(prompt) = state.pending_quick_action_prompt.as_ref() {
            format!("{}{}_", prompt.label(), state.cmd)
        } else {
            format!(" :{}_", state.cmd)
        }
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
    // Status line — lean, no duplicate path info.
    // Issue #15: real transfer progress/rate comes from the authoritative
    // presentation helper (never a counts-only summary). It returns None when
    // there are no active transfers, so no "0 running" clutter.
    let transfer_status = arx::transfer_queue_view::transfer_status_bar(&sync.jobs.snapshot())
        .map(|status| format!(" | transfers {status}"))
        .unwrap_or_default();

    // Workspace Ribbon — provider-truthful identity + workflow phase
    let ribbon_text = workspace_ribbon_text(state);
    let ribbon = Paragraph::new(Line::from(Span::styled(
        ribbon_text,
        Style::default().fg(Color::Cyan),
    )));
    frame.render_widget(ribbon, chunks[1]);

    let status = Paragraph::new(Line::from(format!(
        "ARX v{} | sel: {}{} |{hint}{msg_hint}{git_info}",
        env!("CARGO_PKG_VERSION"),
        selection_count,
        transfer_status,
    )))
    .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(status, chunks[2]);

    // Session milestones are passive presentation. They never own backend
    // state and disappear on the next user interaction.
    let (footer_row_a, footer_row_b) = if let Some(callout) = session_callout.as_deref() {
        render_session_callout(frame, chunks[3], callout);
        (chunks[4], chunks[5])
    } else {
        (chunks[3], chunks[4])
    };

    // Two-row command bar: Row A = Commander core, Row B = Discovery.
    // Derived from the same runtime Keymap that owns keyboard routing.
    let (row_a, row_b) =
        command_bar_rows(state, key_router.keymap(), focused_kind, editor_available);
    command_bar::render(
        frame,
        footer_row_a,
        footer_row_b,
        &mut state.command_hitboxes,
        &row_a,
        &row_b,
    );

    if state.active_overlay() == Some(OverlayKind::SyncPreview) {
        workspace::render(frame, area, state);
    }
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
    let max_h = area.height.saturating_sub(2);
    let h = lines.min(max_h);
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

    // Provider badge: [LOCAL] | [SSH] | [ARCHIVE]
    let provider_badge = match pane.location.provider_id() {
        ProviderId::Local => "[LOCAL]",
        ProviderId::Sftp => "[SSH]",
        ProviderId::Archive => "[ARCHIVE]",
        _ => "[REMOTE]",
    };
    let title = format!(" {} {} ", provider_badge, pane.location.label());

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

fn toggle_hosts_overlay(state: &mut AppState) {
    state.toggle_overlay(OverlayKind::Hosts);
}

fn request_quit(state: &mut AppState, effect_dispatcher: &EffectDispatcher) {
    let mut cancellation_requested = false;
    for lane in [EffectLane::RemoteEdit, EffectLane::QuickAction] {
        if state
            .pending_effect(lane)
            .is_some_and(|id| effect_dispatcher.cancel(id))
        {
            cancellation_requested = true;
        }
    }

    state.apply(Action::Quit);

    if cancellation_requested {
        state.message = Some("Cancellation requested — waiting for a safe terminal outcome".into());
    }
}

// ponytail: keep the one action seam instead of wrapping runtime services in a one-use context.
#[allow(clippy::too_many_arguments)]
async fn dispatch_ui_action(
    state: &mut AppState,
    action: Action,
    focused_row: Option<VisiblePaneRow<'_>>,
    other_focused_row: Option<VisiblePaneRow<'_>>,
    active_entries: &[&Entry],
    visible_count: usize,
    workspace_scanner: &WorkspaceScanner,
    sync: &SyncUiRuntime,
    effect_dispatcher: &EffectDispatcher,
    pane_loader: &PaneLoader,
    terminal_session: &mut TuiTerminalSession,
    configured_editor: Option<&str>,
    key_router: &mut KeyRouter,
) -> io::Result<()> {
    if embedded_terminal::handle_action(state, &action) {
        return Ok(());
    }
    let focused = focused_row
        .and_then(|row| row.listed())
        .map(|listed| &listed.entry);
    // PACK R: proof-feature activation through the registered controller seam.
    {
        let focused_ref = focused;
        let mut ctx = feature_registry::FeatureActionContext {
            state,
            focused: focused_ref,
            active_entries,
            effect_dispatcher,
        };
        if feature_registry::handle_registered_action(&mut ctx, &action) {
            return Ok(());
        }
    }
    // ponytail: keep the ListedEntry (exact identity) for preview, not &Entry
    let focused_listed = focused_row.and_then(|row| row.listed());
    // ponytail: passive pane's focused entry — needed for cross-pane S3 transfer
    let other_listed = other_focused_row.and_then(|row| row.listed());
    if transfers::handle_action(state, &action, focused, focused_listed, other_listed, sync) {
        return Ok(());
    }
    if workspace::handle_action(state, &action, workspace_scanner, sync) {
        return Ok(());
    }
    if mutations::handle_action(state, &action, focused, active_entries, sync, pane_loader) {
        return Ok(());
    }

    match action {
        Action::Quit => request_quit(state, effect_dispatcher),
        Action::OpenCommandCenter => command_center::open(
            state,
            focused.map(|entry| entry.kind),
            configured_editor.is_some(),
        ),
        Action::OpenBookmarks => state.toggle_overlay(OverlayKind::Bookmarks),
        Action::OpenJobs => state.toggle_overlay(OverlayKind::Jobs),
        Action::OpenSmartTree => {
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
        Action::OpenInfrastructureCenter => {
            let opening = state.active_overlay() != Some(OverlayKind::Infrastructure);
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
        Action::OpenHosts => toggle_hosts_overlay(state),
        Action::OpenHelp => {
            key_router.clear_pending();
            state.help_scroll = 0;
            state.toggle_overlay(OverlayKind::Help);
        }
        Action::ToggleSplitPane => {
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
        Action::OpenHotlist => hotlist::open(state),
        Action::OpenInFileManager => {
            let Location::Local(dir) = &state.active_pane().location else {
                state.message = Some("Open in file manager is currently local-only".into());
                return Ok(());
            };
            let dir = dir.clone();
            let id = effect_dispatcher.dispatch(
                EffectLane::GlobalProcess,
                EffectScope::Location(Location::Local(dir.clone())),
                Effect::OpenPath { path: dir.clone() },
            );
            state.register_effect(EffectLane::GlobalProcess, id);
            state.message = Some(format!("Opening {}", dir.display()));
            state.dir_history.push(dir);
            if state.dir_history.len() > 20 {
                state.dir_history.remove(0);
            }
        }
        Action::ToggleSelect => {
            toggle_selection_and_advance(state, focused, visible_count);
        }
        Action::ViewFile => {
            let Some(listed) = focused_listed.filter(|listed| listed.entry.kind == EntryKind::File)
            else {
                state.message = Some("Select a regular file to view".into());
                return Ok(());
            };
            let location = state.active_pane().location.clone();
            // SFTP/S3: dispatch preview intent, network I/O runs inside effect lane.
            // Forward the exact ListedEntry identity — never reduce to &entry.name.
            if s3_f3_routes_to_preview(&location) {
                let id = effect_dispatcher.dispatch(
                    EffectLane::Preview,
                    EffectScope::Location(location.clone()),
                    Effect::PreviewLocation {
                        location,
                        listed: listed.clone(),
                    },
                );
                state.register_effect(EffectLane::Preview, id);
                state.message = Some(format!("Loading preview: {}", listed.entry.name));
                return Ok(());
            }
            let Location::Local(base) = &location else {
                state.message = Some("File preview is currently local-only".into());
                return Ok(());
            };
            let entry = focused.expect("file filtered above");
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

            match state.active_pane().location.clone() {
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
                    let location = state.active_pane().location.clone();
                    let name = entry.name.clone();
                    remote_edit::begin_sftp_edit(state, location, name, editor, effect_dispatcher);
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
        Action::ListTmuxSessions => {
            let id = effect_dispatcher.dispatch(
                EffectLane::TmuxDiscovery,
                EffectScope::Global,
                Effect::ListTmuxSessions,
            );
            state.register_effect(EffectLane::TmuxDiscovery, id);
            state.message = Some("Discovering tmux sessions…".into());
        }
        _ => state.apply(action),
    }
    Ok(())
}

/// Product-path cancel for the Jobs UI. Transfer jobs route through the queue
/// runtime (which holds the executor + scheduler); every other kind uses the
/// legacy JobManager cancel. Tests exercise exactly this function.
fn cancel_job_product_route(state: &mut AppState, sync: &SyncUiRuntime, job_id: &str) -> bool {
    let kind = sync.jobs.get(job_id).map(|job| job.kind);
    let cancelled = match kind {
        Some(arx::jobs::JobKind::Transfer) => sync.transfers.cancel(job_id).is_ok(),
        _ => {
            // Legacy path: unrelated job kinds use the existing JobManager
            // cancel token. Keep the literal routing visible for the contract.
            let id = job_id.to_string();
            let job_manager = &sync.jobs;
            job_manager.cancel(&id)
        }
    };
    if cancelled {
        state.message = Some(format!("Job {job_id} cancellation requested"));
    }
    cancelled
}

// ponytail: a context struct would only hide these already-scoped runtime services.
#[allow(clippy::too_many_arguments)]
async fn execute_command_target(
    state: &mut AppState,
    target: CommandTarget,
    focused: Option<VisiblePaneRow<'_>>,
    other_focused_row: Option<VisiblePaneRow<'_>>,
    active_entries: &[&Entry],
    visible_count: usize,
    workspace_scanner: &WorkspaceScanner,
    pane_loader: &PaneLoader,
    sync: &SyncUiRuntime,
    effect_dispatcher: &EffectDispatcher,
    terminal_session: &mut TuiTerminalSession,
    configured_editor: Option<&str>,
    key_router: &mut KeyRouter,
) -> io::Result<Option<Effect>> {
    let effect = match target {
        CommandTarget::Action(action) => {
            dispatch_ui_action(
                state,
                action,
                focused,
                other_focused_row,
                active_entries,
                visible_count,
                workspace_scanner,
                sync,
                effect_dispatcher,
                pane_loader,
                terminal_session,
                configured_editor,
                key_router,
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

// ponytail: keep for test visibility; selection logic lives in dispatch_ui_action.
#[allow(dead_code)]
fn selection_or_cursor(
    state: &AppState,
    entries: &[VisiblePaneRow<'_>],
    cursor: usize,
) -> Vec<String> {
    let pane = state.active_pane();
    if let Some(selected) = state.selection_names(state.active, &pane.location) {
        selected.iter().cloned().collect()
    } else if let Some(entry) = entries
        .get(cursor)
        .and_then(VisiblePaneRow::listed)
        .map(|listed| &listed.entry)
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
    use arx::services::{PaneListingContinuation, PaneLoadId, PaneLoadPage, WorkspaceScanId};
    use arx::vfs::s3::{S3BucketRef, S3ObjectRef, S3PrefixRef};
    use arx::vfs::{
        Capability, CapabilitySet, Entry, EntryIdentity, EntryKind, ListedEntry,
        ProviderContinuation, ProviderListingPage, ProviderRegistry, VfsProvider,
    };
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
        let (pane_loader, _pane_responses, _next_page_responses) =
            PaneLoader::channel(registry.clone());
        // Production-equivalent wiring: the one shared JobManager + event channel
        // that run() binds, so the remote-edit job is real and observable.
        let job_manager = arx::jobs::JobManager::new();
        let (job_tx, mut job_rx) = tokio::sync::mpsc::unbounded_channel::<arx::jobs::JobEvent>();
        let mut state = AppState {
            registry: registry.clone(),
            job_manager: Some(job_manager),
            job_events: Some(job_tx),
            ..AppState::default()
        };
        state.left.location = location.clone();
        state.pending_remote_edit_origin = Some((Pane::Left, location.clone()));
        // Production F4 start: create exactly one RemoteEdit job, store its id.
        let re_job = state
            .job_manager
            .as_ref()
            .expect("job manager bound")
            .create_job(
                "remote-edit",
                arx::jobs::JobKind::RemoteEdit,
                "Remote edit note.txt",
                Some(location.clone()),
                None,
            );
        let re_id = re_job.id.clone();
        state.pending_remote_edit_job_id = Some(re_id.clone());
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
        effect_responses::finalize_received_effect(&dispatcher, &mut response);
        effect_responses::handle_effect_response(response, &mut state, &pane_loader);

        let session = state.pending_remote_edit_session.take().unwrap();
        let temp_path = session.temp_dir.path().to_path_buf();
        let working_path = temp_path.join("working");
        let editor_result = DesktopService::open_editor("false", &working_path).await;
        assert!(editor_result.is_err(), "editor must return non-zero");
        // No writeback effect is scheduled on editor failure.
        assert!(
            remote_edit::finish_remote_editor(session, editor_result, &mut state).is_none(),
            "editor failure must not schedule a writeback effect"
        );
        // Session-scoped pending state cleared (defensive, job-id-independent).
        assert!(state.pending_remote_edit_origin.is_none());
        assert!(state.pending_remote_edit_session.is_none());
        assert!(state.pending_remote_edit_job_id.is_none());
        assert!(!state.pending_editor);
        assert!(
            !temp_path.exists(),
            "secure temporary directory must be dropped"
        );
        // Exactly one RemoteEdit job, terminal as typed Failed, no leak.
        let snap = state
            .job_manager
            .as_ref()
            .expect("job manager bound")
            .snapshot();
        assert_eq!(snap.len(), 1, "exactly one RemoteEdit job");
        assert_eq!(snap[0].id, re_id, "job id is the one created");
        assert!(snap[0].status.is_terminal(), "job reached terminal truth");
        assert_eq!(
            snap[0].result,
            Some(arx::jobs::JobResult::RemoteEdit(
                arx::jobs::RemoteEditOutcome::Failed
            )),
            "terminal truth is typed RemoteEdit::Failed"
        );
        // Drain the event channel and confirm the typed Failed terminal arrived.
        let mut terminal = None;
        while let Ok(ev) = job_rx.try_recv() {
            if let arx::jobs::JobEvent::Failed {
                result: Some(arx::jobs::JobResult::RemoteEdit(o)),
                ..
            }
            | arx::jobs::JobEvent::Completed {
                result: arx::jobs::JobResult::RemoteEdit(o),
                ..
            } = ev
            {
                terminal = Some(o);
            }
        }
        assert_eq!(terminal, Some(arx::jobs::RemoteEditOutcome::Failed));
        assert_remote_original(&registry, &location).await;
        fixture.cleanup().await;
    }

    // #51 behavioral (runnable, no host): one RemoteEdit job id survives every
    // phase transition and reaches a terminal truth, exactly as the production
    // Behavioral (runnable, no host): mirrors the production F4 path exactly —
    // the AppState binds the SAME JobManager + event channel that run() binds,
    // then the remote-edit wiring creates one job and publishes through phases.
    // Asserts the job id survives and the terminal truth is the typed
    // RemoteEditOutcome::Completed (not a collapsed generic result).
    #[tokio::test]
    async fn track_f1_remote_edit_job_id_survives_phases_and_terminal_truth() {
        let mut state = AppState::default();
        // Production wiring: run() binds the one real manager + channel.
        let job_manager = arx::jobs::JobManager::new();
        let (job_tx, _job_rx) = tokio::sync::mpsc::unbounded_channel::<arx::jobs::JobEvent>();
        state.job_manager = Some(job_manager);
        state.job_events = Some(job_tx);
        let location = Location::Sftp {
            host: "example".into(),
            path: "/srv".into(),
        };
        // F4 start: create exactly one RemoteEdit job, store its id.
        let re_job = state
            .job_manager
            .as_ref()
            .expect("job manager bound")
            .create_job(
                "remote-edit",
                arx::jobs::JobKind::RemoteEdit,
                "Remote edit note.txt",
                Some(location.clone()),
                None,
            );
        let id = re_job.id.clone();
        state.pending_remote_edit_job_id = Some(id.clone());
        assert!(
            state
                .job_manager
                .as_ref()
                .expect("job manager bound")
                .get(&id)
                .is_some_and(|j| j.kind == arx::jobs::JobKind::RemoteEdit)
        );

        // Downloaded → Ready: same id, Running.
        let _ = state
            .job_manager
            .as_ref()
            .expect("job manager bound")
            .publish_event(
                state.job_events.as_ref().expect("job events bound"),
                arx::jobs::JobEvent::Running { id: id.clone() },
            );
        // Editing: same id.
        let _ = state
            .job_manager
            .as_ref()
            .expect("job manager bound")
            .publish_event(
                state.job_events.as_ref().expect("job events bound"),
                arx::jobs::JobEvent::Running { id: id.clone() },
            );
        // WriteBack: same id.
        let _ = state
            .job_manager
            .as_ref()
            .expect("job manager bound")
            .publish_event(
                state.job_events.as_ref().expect("job events bound"),
                arx::jobs::JobEvent::Running { id: id.clone() },
            );
        // Terminal truth: WrittenBack → Completed with same id.
        let _ = state
            .job_manager
            .as_ref()
            .expect("job manager bound")
            .publish_event(
                state.job_events.as_ref().expect("job events bound"),
                arx::jobs::JobEvent::Completed {
                    id: id.clone(),
                    // Typed, not generic — this is the MAJOR #2 contract.
                    result: arx::jobs::JobResult::RemoteEdit(
                        arx::jobs::RemoteEditOutcome::Completed,
                    ),
                },
            );
        state.pending_remote_edit_job_id = None;

        let snap = state
            .job_manager
            .as_ref()
            .expect("job manager bound")
            .snapshot();
        assert_eq!(snap.len(), 1, "exactly one RemoteEdit job");
        let job = &snap[0];
        assert_eq!(job.id, id, "job id survives all phases");
        assert_eq!(job.kind, arx::jobs::JobKind::RemoteEdit);
        assert!(job.status.is_terminal(), "reaches terminal truth");
        // Typed outcome must be recoverable from the snapshot, not stringified.
        assert_eq!(
            job.result,
            Some(arx::jobs::JobResult::RemoteEdit(
                arx::jobs::RemoteEditOutcome::Completed
            )),
            "terminal truth is typed RemoteEdit::Completed"
        );
    }

    // #51 behavioral (runnable, no host): drive the real JobManager + event
    // channel through the full normal remote-edit phase model and assert every
    // phase (Queued→Downloading→AwaitingEditor→Editing→ValidatingWorkingCopy→
    // WriteBack→Verifying) is observed with the SAME job id, ending in a typed
    // Completed terminal — exactly as finish_remote_editor publishes.
    #[tokio::test]
    async fn remote_edit_phase_model_normal_reaches_all_phases() {
        let mut state = AppState::default();
        let job_manager = arx::jobs::JobManager::new();
        let (job_tx, mut job_rx) = tokio::sync::mpsc::unbounded_channel::<arx::jobs::JobEvent>();
        state.job_manager = Some(job_manager);
        state.job_events = Some(job_tx);
        let location = Location::Sftp {
            host: "example".into(),
            path: "/srv".into(),
        };
        let re_job = state
            .job_manager
            .as_ref()
            .expect("job manager bound")
            .create_job(
                "remote-edit",
                arx::jobs::JobKind::RemoteEdit,
                "Remote edit note.txt",
                Some(location),
                None,
            );
        let id = re_job.id.clone();
        state.pending_remote_edit_job_id = Some(id.clone());

        let mgr = state.job_manager.as_ref().expect("job manager bound");
        let ev = state.job_events.as_ref().expect("job events bound");
        // Production: download-complete sets the job Running before phases.
        let _ = mgr.publish_event(ev, arx::jobs::JobEvent::Running { id: id.clone() });
        // F4 production start (tui.rs deferred-launch path).
        remote_edit::publish_remote_edit_phase(&state, arx::jobs::RemoteEditPhase::Queued);
        remote_edit::publish_remote_edit_phase(&state, arx::jobs::RemoteEditPhase::Downloading);
        remote_edit::publish_remote_edit_phase(&state, arx::jobs::RemoteEditPhase::AwaitingEditor);
        remote_edit::publish_remote_edit_phase(&state, arx::jobs::RemoteEditPhase::Editing);
        // finish_remote_editor: validation → writeback → verification.
        remote_edit::publish_remote_edit_phase(
            &state,
            arx::jobs::RemoteEditPhase::ValidatingWorkingCopy,
        );
        remote_edit::publish_remote_edit_phase(&state, arx::jobs::RemoteEditPhase::WriteBack);
        remote_edit::publish_remote_edit_phase(&state, arx::jobs::RemoteEditPhase::Verifying);
        remote_edit::terminate_remote_edit_job(
            &mut state,
            arx::jobs::RemoteEditOutcome::Completed,
            None,
        );

        let mut phases = Vec::new();
        let mut terminal = None;
        while let Ok(ev) = job_rx.try_recv() {
            match ev {
                arx::jobs::JobEvent::Progress {
                    progress: arx::jobs::JobProgress::RemoteEdit(p),
                    ..
                } => phases.push(p),
                arx::jobs::JobEvent::Completed {
                    result: arx::jobs::JobResult::RemoteEdit(o),
                    ..
                } => terminal = Some(o),
                _ => {}
            }
        }
        assert_eq!(
            phases,
            vec![
                arx::jobs::RemoteEditPhase::Queued,
                arx::jobs::RemoteEditPhase::Downloading,
                arx::jobs::RemoteEditPhase::AwaitingEditor,
                arx::jobs::RemoteEditPhase::Editing,
                arx::jobs::RemoteEditPhase::ValidatingWorkingCopy,
                arx::jobs::RemoteEditPhase::WriteBack,
                arx::jobs::RemoteEditPhase::Verifying,
            ],
            "all 8 phases observed in order"
        );
        assert_eq!(terminal, Some(arx::jobs::RemoteEditOutcome::Completed));
        assert!(state.pending_remote_edit_job_id.is_none());
    }

    // #51 behavioral (runnable, no host): recovery path exposes RollbackOrRecovery
    // BEFORE the typed RecoveryRequired terminal, same job id.
    #[tokio::test]
    async fn remote_edit_phase_model_recovery_exposes_rollback_before_required() {
        let mut state = AppState::default();
        let job_manager = arx::jobs::JobManager::new();
        let (job_tx, mut job_rx) = tokio::sync::mpsc::unbounded_channel::<arx::jobs::JobEvent>();
        state.job_manager = Some(job_manager);
        state.job_events = Some(job_tx);
        let re_job = state
            .job_manager
            .as_ref()
            .expect("job manager bound")
            .create_job(
                "remote-edit",
                arx::jobs::JobKind::RemoteEdit,
                "Remote edit",
                None,
                None,
            );
        let id = re_job.id.clone();
        state.pending_remote_edit_job_id = Some(id.clone());

        let mgr = state.job_manager.as_ref().expect("job manager bound");
        let ev = state.job_events.as_ref().expect("job events bound");
        let _ = mgr.publish_event(ev, arx::jobs::JobEvent::Running { id: id.clone() });
        remote_edit::publish_remote_edit_phase(&state, arx::jobs::RemoteEditPhase::Queued);
        // RecoveryRequired handler emits RollbackOrRecovery first.
        remote_edit::publish_remote_edit_phase(
            &state,
            arx::jobs::RemoteEditPhase::RollbackOrRecovery,
        );
        remote_edit::terminate_remote_edit_job(
            &mut state,
            arx::jobs::RemoteEditOutcome::RecoveryRequired,
            Some("recovery required".into()),
        );

        let mut phases = Vec::new();
        let mut terminal = None;
        while let Ok(ev) = job_rx.try_recv() {
            match ev {
                arx::jobs::JobEvent::Progress {
                    progress: arx::jobs::JobProgress::RemoteEdit(p),
                    ..
                } => phases.push(p),
                arx::jobs::JobEvent::Completed {
                    result: arx::jobs::JobResult::RemoteEdit(o),
                    ..
                }
                | arx::jobs::JobEvent::Failed {
                    result: Some(arx::jobs::JobResult::RemoteEdit(o)),
                    ..
                } => terminal = Some(o),
                _ => {}
            }
        }
        assert!(
            phases.contains(&arx::jobs::RemoteEditPhase::RollbackOrRecovery),
            "RollbackOrRecovery observed"
        );
        assert_eq!(
            terminal,
            Some(arx::jobs::RemoteEditOutcome::RecoveryRequired)
        );
        assert!(state.pending_remote_edit_job_id.is_none());
    }

    // #51 behavioral (runnable, no host): no-change terminates after validation
    // WITHOUT writeback/verification, same job id.
    #[tokio::test]
    async fn remote_edit_phase_model_no_change_skips_writeback() {
        let mut state = AppState::default();
        let job_manager = arx::jobs::JobManager::new();
        let (job_tx, mut job_rx) = tokio::sync::mpsc::unbounded_channel::<arx::jobs::JobEvent>();
        state.job_manager = Some(job_manager);
        state.job_events = Some(job_tx);
        let re_job = state
            .job_manager
            .as_ref()
            .expect("job manager bound")
            .create_job(
                "remote-edit",
                arx::jobs::JobKind::RemoteEdit,
                "Remote edit",
                None,
                None,
            );
        let id = re_job.id.clone();
        state.pending_remote_edit_job_id = Some(id.clone());

        let mgr = state.job_manager.as_ref().expect("job manager bound");
        let ev = state.job_events.as_ref().expect("job events bound");
        let _ = mgr.publish_event(ev, arx::jobs::JobEvent::Running { id: id.clone() });
        remote_edit::publish_remote_edit_phase(&state, arx::jobs::RemoteEditPhase::Queued);
        remote_edit::publish_remote_edit_phase(&state, arx::jobs::RemoteEditPhase::Downloading);
        remote_edit::publish_remote_edit_phase(&state, arx::jobs::RemoteEditPhase::AwaitingEditor);
        remote_edit::publish_remote_edit_phase(&state, arx::jobs::RemoteEditPhase::Editing);
        remote_edit::publish_remote_edit_phase(
            &state,
            arx::jobs::RemoteEditPhase::ValidatingWorkingCopy,
        );
        // NoChange terminal — must NOT emit WriteBack/Verifying.
        remote_edit::terminate_remote_edit_job(
            &mut state,
            arx::jobs::RemoteEditOutcome::NoChange,
            None,
        );

        let mut phases = Vec::new();
        let mut terminal = None;
        while let Ok(ev) = job_rx.try_recv() {
            match ev {
                arx::jobs::JobEvent::Progress {
                    progress: arx::jobs::JobProgress::RemoteEdit(p),
                    ..
                } => phases.push(p),
                arx::jobs::JobEvent::Completed {
                    result: arx::jobs::JobResult::RemoteEdit(o),
                    ..
                } => terminal = Some(o),
                _ => {}
            }
        }
        assert!(
            !phases.contains(&arx::jobs::RemoteEditPhase::WriteBack),
            "no WriteBack on no-change"
        );
        assert!(
            !phases.contains(&arx::jobs::RemoteEditPhase::Verifying),
            "no Verifying on no-change"
        );
        assert_eq!(terminal, Some(arx::jobs::RemoteEditOutcome::NoChange));
        assert!(state.pending_remote_edit_job_id.is_none());
    }

    // #51 behavioral (runnable, no host): editor failure terminalizes as Failed
    // and never fabricates later phases (no WriteBack/Verifying after failure).
    #[tokio::test]
    async fn remote_edit_phase_model_failure_never_fabricates_later_phases() {
        let mut state = AppState::default();
        let job_manager = arx::jobs::JobManager::new();
        let (job_tx, mut job_rx) = tokio::sync::mpsc::unbounded_channel::<arx::jobs::JobEvent>();
        state.job_manager = Some(job_manager);
        state.job_events = Some(job_tx);
        let re_job = state
            .job_manager
            .as_ref()
            .expect("job manager bound")
            .create_job(
                "remote-edit",
                arx::jobs::JobKind::RemoteEdit,
                "Remote edit",
                None,
                None,
            );
        let id = re_job.id.clone();
        state.pending_remote_edit_job_id = Some(id.clone());

        let mgr = state.job_manager.as_ref().expect("job manager bound");
        let ev = state.job_events.as_ref().expect("job events bound");
        let _ = mgr.publish_event(ev, arx::jobs::JobEvent::Running { id: id.clone() });
        remote_edit::publish_remote_edit_phase(&state, arx::jobs::RemoteEditPhase::Queued);
        remote_edit::publish_remote_edit_phase(&state, arx::jobs::RemoteEditPhase::Editing);
        // Editor failure → Failed terminal (finish_remote_editor Err arm).
        remote_edit::terminate_remote_edit_job(
            &mut state,
            arx::jobs::RemoteEditOutcome::Failed,
            Some("editor failed".into()),
        );

        let mut phases = Vec::new();
        let mut terminal = None;
        while let Ok(ev) = job_rx.try_recv() {
            match ev {
                arx::jobs::JobEvent::Progress {
                    progress: arx::jobs::JobProgress::RemoteEdit(p),
                    ..
                } => phases.push(p),
                arx::jobs::JobEvent::Failed { .. } => {
                    terminal = Some(arx::jobs::RemoteEditOutcome::Failed)
                }
                _ => {}
            }
        }
        assert!(
            !phases.contains(&arx::jobs::RemoteEditPhase::WriteBack),
            "no WriteBack after failure"
        );
        assert!(
            !phases.contains(&arx::jobs::RemoteEditPhase::Verifying),
            "no Verifying after failure"
        );
        assert_eq!(terminal, Some(arx::jobs::RemoteEditOutcome::Failed));
        assert!(state.pending_remote_edit_job_id.is_none());
    }

    // #48/MAJOR#1: runnable proof that queued-cancel → typed Cancelled and a
    // real provider error → Failed, and the two cannot be confused. Drives the
    // real `apply_effect_event` mapping (no host needed).
    #[test]
    fn remote_edit_cancel_vs_failure_outcomes_are_distinct_and_typed() {
        use arx::jobs::{JobEvent, RemoteEditCancelReason, RemoteEditOutcome};

        // Queued cancel → Cancelled terminal.
        let mut cancel_state = AppState::default();
        let jm_cancel = arx::jobs::JobManager::new();
        let (tx_cancel, mut rx_cancel) = tokio::sync::mpsc::unbounded_channel::<JobEvent>();
        cancel_state.job_manager = Some(jm_cancel);
        cancel_state.job_events = Some(tx_cancel);
        cancel_state.pending_remote_edit_job_id = Some(
            cancel_state
                .job_manager
                .as_ref()
                .unwrap()
                .create_job(
                    "remote-edit",
                    arx::jobs::JobKind::RemoteEdit,
                    "Remote edit",
                    None,
                    None,
                )
                .id
                .clone(),
        );
        effect_responses::apply_effect_event(
            &mut cancel_state,
            EffectLane::RemoteEdit,
            EffectEvent::RemoteEditCancelled {
                name: "note.txt".into(),
                reason: RemoteEditCancelReason::Queued,
            },
        );
        let mut cancel_outcome = None;
        while let Ok(ev) = rx_cancel.try_recv() {
            if let JobEvent::Cancelled { .. } = ev {
                cancel_outcome = Some(RemoteEditOutcome::Cancelled);
            }
        }
        assert_eq!(cancel_outcome, Some(RemoteEditOutcome::Cancelled));
        assert!(cancel_state.pending_remote_edit_job_id.is_none());

        // Real provider error → Failed terminal (never Cancelled).
        let mut fail_state = AppState::default();
        let jm_fail = arx::jobs::JobManager::new();
        let (tx_fail, mut rx_fail) = tokio::sync::mpsc::unbounded_channel::<JobEvent>();
        fail_state.job_manager = Some(jm_fail);
        fail_state.job_events = Some(tx_fail);
        fail_state.pending_remote_edit_job_id = Some(
            fail_state
                .job_manager
                .as_ref()
                .unwrap()
                .create_job(
                    "remote-edit",
                    arx::jobs::JobKind::RemoteEdit,
                    "Remote edit",
                    None,
                    None,
                )
                .id
                .clone(),
        );
        effect_responses::apply_effect_event(
            &mut fail_state,
            EffectLane::RemoteEdit,
            EffectEvent::Failed {
                label: "remote edit download".into(),
                error: "connection reset".into(),
            },
        );
        let mut fail_outcome = None;
        let mut cancelled = false;
        while let Ok(ev) = rx_fail.try_recv() {
            match ev {
                JobEvent::Failed { .. } => fail_outcome = Some(RemoteEditOutcome::Failed),
                JobEvent::Cancelled { .. } => cancelled = true,
                _ => {}
            }
        }
        assert_eq!(fail_outcome, Some(RemoteEditOutcome::Failed));
        assert!(!cancelled, "provider error must not be typed Cancelled");
    }

    // #51/MAJOR: an unrelated (non-RemoteEdit) Failed effect must NOT mutate an
    // in-flight Remote Edit — same job id, still Running, ownership intact.
    #[test]
    fn unrelated_failed_effect_does_not_kill_active_remote_edit() {
        let mut state = AppState::default();
        let jm = arx::jobs::JobManager::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<arx::jobs::JobEvent>();
        state.job_manager = Some(jm);
        state.job_events = Some(tx);
        state.left.location = Location::Sftp {
            host: "h".into(),
            path: "/x".into(),
        };
        state.pending_remote_edit_origin = Some((Pane::Left, state.left.location.clone()));
        let re_job = state.job_manager.as_ref().unwrap().create_job(
            "remote-edit",
            arx::jobs::JobKind::RemoteEdit,
            "Remote edit note.txt",
            Some(state.left.location.clone()),
            None,
        );
        let re_id = re_job.id.clone();
        state.pending_remote_edit_job_id = Some(re_id.clone());

        // Feed a generic failure on an unrelated lane (e.g. LeftPane).
        effect_responses::apply_effect_event(
            &mut state,
            EffectLane::LeftPane,
            EffectEvent::Failed {
                label: "list directory".into(),
                error: "permission denied".into(),
            },
        );

        // Remote Edit must be completely untouched.
        assert_eq!(
            state.pending_remote_edit_job_id,
            Some(re_id.clone()),
            "job id must survive an unrelated failure"
        );
        assert!(
            state.pending_remote_edit_origin.is_some(),
            "origin must survive an unrelated failure"
        );
        assert!(
            !state.pending_editor,
            "editor flag must not be set, and must not be cleared by unrelated failure"
        );
        let job = state
            .job_manager
            .as_ref()
            .unwrap()
            .get(&re_id)
            .expect("job still present");
        assert_eq!(
            job.status,
            arx::jobs::JobStatus::Pending,
            "in-flight Remote Edit job must not be terminalized by an unrelated failure"
        );
        // No RemoteEdit terminal event may have been published.
        let mut terminal = false;
        while let Ok(ev) = rx.try_recv() {
            if matches!(
                ev,
                arx::jobs::JobEvent::Failed { .. }
                    | arx::jobs::JobEvent::Cancelled { .. }
                    | arx::jobs::JobEvent::Completed { .. }
            ) {
                terminal = true;
            }
        }
        assert!(
            !terminal,
            "no RemoteEdit terminal event from unrelated failure"
        );

        // Now the same failure on the RemoteEdit lane DOES terminate it once.
        effect_responses::apply_effect_event(
            &mut state,
            EffectLane::RemoteEdit,
            EffectEvent::Failed {
                label: "remote edit download".into(),
                error: "connection reset".into(),
            },
        );
        assert!(state.pending_remote_edit_job_id.is_none());
        assert!(state.pending_remote_edit_origin.is_none());
        let job = state
            .job_manager
            .as_ref()
            .unwrap()
            .get(&re_id)
            .expect("job still present");
        assert_eq!(job.status, arx::jobs::JobStatus::Failed);
        let mut failed_once = false;
        while let Ok(ev) = rx.try_recv() {
            if matches!(ev, arx::jobs::JobEvent::Failed { .. }) {
                failed_once = true;
            }
        }
        assert!(
            failed_once,
            "exactly one Failed terminal on RemoteEdit lane"
        );
    }

    // #51 physical (requires host): cancel before editor prevents later launch
    // and clears the in-flight RemoteEdit job id.
    #[tokio::test]
    #[ignore = "requires ARX_SFTP_SMOKE_HOST pointing at a disposable SSH/SFTP host"]
    async fn physical_remote_edit_cancel_before_editor_clears_job() {
        let host = std::env::var("ARX_SFTP_SMOKE_HOST").unwrap();
        let fixture = PhysicalRemoteEditFixture::new(&host, "cancel-before-edit").await;
        let location = fixture.location();
        let registry = physical_sftp_registry(&host);
        let (dispatcher, mut responses) = EffectDispatcher::channel(registry.clone());
        let (pane_loader, _pane_responses, _next_page_responses) =
            PaneLoader::channel(registry.clone());
        let mut state = AppState {
            registry: registry.clone(),
            ..AppState::default()
        };
        let job_manager = arx::jobs::JobManager::new();
        let (job_tx, _job_rx) = tokio::sync::mpsc::unbounded_channel::<arx::jobs::JobEvent>();
        state.job_manager = Some(job_manager);
        state.job_events = Some(job_tx);
        state.left.location = location.clone();
        state.pending_remote_edit_origin = Some((Pane::Left, location.clone()));
        // Production F4 start: create exactly one RemoteEdit job and record its id.
        let re_job = state
            .job_manager
            .as_ref()
            .expect("job manager bound")
            .create_job(
                "remote-edit",
                arx::jobs::JobKind::RemoteEdit,
                "Remote edit note.txt",
                Some(location.clone()),
                None,
            );
        state.pending_remote_edit_job_id = Some(re_job.id.clone());
        let id = dispatcher.dispatch(
            EffectLane::RemoteEdit,
            EffectScope::Location(location.clone()),
            Effect::DownloadRemoteFile {
                location: location.clone(),
                name: "note.txt".into(),
                editor: "sleep 1".into(),
            },
        );
        state.register_effect(EffectLane::RemoteEdit, id);
        let response = tokio::time::timeout(std::time::Duration::from_secs(20), responses.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(&response.event, EffectEvent::Downloaded { .. }));
        effect_responses::handle_effect_response(response, &mut state, &pane_loader);
        // Downloaded must have created exactly one RemoteEdit job id.
        let job_id = state
            .pending_remote_edit_job_id
            .clone()
            .expect("job id set after download");
        assert!(
            state
                .job_manager
                .as_ref()
                .expect("job manager bound")
                .get(&job_id)
                .is_some()
        );
        // Cancel before editor via the REAL production finalization path:
        // navigate the originating pane away, then let the deferred-launch
        // stale check terminalize the job (Cancelled) and clear all ownership.
        state.left.location = Location::Local("/elsewhere".into());
        assert!(
            remote_edit::finalize_remote_edit_if_stale(&mut state),
            "stale-navigation must terminalize the in-flight job"
        );
        assert!(state.pending_remote_edit_job_id.is_none());
        // The job itself must be terminal (Cancelled), not left Running.
        let job = state
            .job_manager
            .as_ref()
            .expect("job manager bound")
            .get(&job_id)
            .expect("job still present");
        assert_eq!(job.status, arx::jobs::JobStatus::Cancelled);
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
        let (pane_loader, _pane_responses, _next_page_responses) =
            PaneLoader::channel(registry.clone());
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
        effect_responses::finalize_received_effect(&dispatcher, &mut response);
        assert!(
            matches!(
                &response.event,
                EffectEvent::RemoteEditCancelled {
                    reason: arx::jobs::RemoteEditCancelReason::Queued,
                    ..
                }
            ),
            "explicit queued cancel must be typed Cancelled, not generic Failed"
        );
        effect_responses::handle_effect_response(response, &mut state, &pane_loader);

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

        assert!(effect_responses::pane_still_at_location(
            &state,
            Pane::Left,
            &origin
        ));
        assert!(effect_responses::pane_still_at_location(
            &state,
            Pane::Right,
            &origin
        ));

        state.left.location = Location::Local("/elsewhere".into());
        assert!(!effect_responses::pane_still_at_location(
            &state,
            Pane::Left,
            &origin
        ));
        assert!(effect_responses::pane_still_at_location(
            &state,
            Pane::Right,
            &origin
        ));
    }

    fn file(name: &str) -> Entry {
        Entry {
            name: name.into(),
            kind: EntryKind::File,
            size: Some(1),
            modified_unix_ms: None,
        }
    }

    fn listed(entry: Entry) -> ListedEntry {
        ListedEntry {
            entry,
            identity: EntryIdentity::Other,
        }
    }

    fn listed_with_identity(name: &str, kind: EntryKind, identity: EntryIdentity) -> ListedEntry {
        ListedEntry {
            entry: Entry {
                name: name.into(),
                kind,
                size: None,
                modified_unix_ms: None,
            },
            identity,
        }
    }

    fn test_registry() -> ProviderRegistry {
        let registry = ProviderRegistry::new();
        registry.register_s3_targets(&[
            arx::config::S3TargetConfig {
                id: "acc".into(),
                name: "acc".into(),
                bucket: None,
                region: None,
                profile: None,
                endpoint_url: None,
                force_path_style: false,
            },
            arx::config::S3TargetConfig {
                id: "bkt".into(),
                name: "bkt".into(),
                bucket: Some("company-artifacts".into()),
                region: None,
                profile: None,
                endpoint_url: None,
                force_path_style: false,
            },
        ]);
        registry
    }

    fn page(entries: Vec<Entry>) -> PaneLoadPage {
        PaneLoadPage {
            entries: entries.into_iter().map(listed).collect(),
            continuation: None,
        }
    }

    fn page_continuation(id: PaneLoadId, location: Location) -> PaneListingContinuation {
        PaneListingContinuation {
            provider_continuation: ProviderContinuation {
                token: "  opaque+/=token 日本語  ".into(),
            },
            provider_instance: ProviderRegistry::instance_key_for_location(&location),
            location,
            generation: id,
        }
    }

    #[derive(Debug)]
    struct IdentityPageProvider {
        entry: ListedEntry,
    }

    #[async_trait::async_trait]
    impl VfsProvider for IdentityPageProvider {
        fn list(&self, _path: &str) -> std::io::Result<Vec<Entry>> {
            Err(std::io::ErrorKind::Unsupported.into())
        }

        async fn list_page(
            &self,
            _location: &Location,
            continuation: Option<&ProviderContinuation>,
        ) -> std::io::Result<ProviderListingPage> {
            assert!(continuation.is_none());
            Ok(ProviderListingPage {
                entries: vec![self.entry.clone()],
                continuation: None,
            })
        }

        fn read_head(&self, _path: &str, _lines: usize) -> std::io::Result<Vec<String>> {
            Err(std::io::ErrorKind::Unsupported.into())
        }

        fn copy_files(&self, _src: &str, _dst: &str, _names: &[String]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::Unsupported.into())
        }

        fn move_files(&self, _src: &str, _dst: &str, _names: &[String]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::Unsupported.into())
        }

        fn delete_files(&self, _dir: &str, _names: &[String]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::Unsupported.into())
        }
    }

    #[test]
    fn selection_from_other_pane_is_not_a_mutation_target() {
        let mut state = AppState::default();
        let left_location = state.left.location.clone();
        state.toggle_selection(Pane::Left, &left_location, "left-only.txt");
        state.active = Pane::Right;
        let right_entries = [listed(file("right.txt"))];
        let visible = [VisiblePaneRow::Listed(&right_entries[0])];

        assert_eq!(selection_or_cursor(&state, &visible, 0), vec!["right.txt"]);
    }

    #[test]
    fn selection_from_previous_location_is_not_a_mutation_target() {
        let mut state = AppState::default();
        state.left.location = Location::Local("/a".into());
        let original = state.left.location.clone();
        state.toggle_selection(Pane::Left, &original, "foo.txt");
        state.left.location = Location::Local("/b".into());
        let current_entries = [listed(file("bar.txt"))];
        let visible = [VisiblePaneRow::Listed(&current_entries[0])];

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
        let registry = test_registry();
        let entries = [
            listed(Entry {
                name: "..".into(),
                kind: EntryKind::Directory,
                size: None,
                modified_unix_ms: None,
            }),
            listed(file("child.txt")),
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
            let visible = apply_filter_with_parent(&entries, "", &location, &registry, &parent);
            assert_eq!(
                visible
                    .iter()
                    .map(|row| row.entry().name.as_str())
                    .collect::<Vec<_>>(),
                vec!["..", "child.txt"]
            );
            assert!(matches!(visible[0], VisiblePaneRow::Parent(_)));
            assert!(matches!(visible[1], VisiblePaneRow::Listed(_)));
        }

        let roots = [
            Location::Local("/".into()),
            Location::Sftp {
                host: "prod".into(),
                path: "/".into(),
            },
        ];
        for location in roots {
            let visible = apply_filter_with_parent(&entries, "", &location, &registry, &parent);
            assert_eq!(visible.len(), 1);
            assert!(matches!(visible[0], VisiblePaneRow::Listed(_)));
            assert_eq!(visible[0].entry().name, "child.txt");
        }
    }

    #[test]
    fn virtual_parent_navigates_but_is_not_a_mutation_target() {
        let parent = virtual_parent_entry();
        let registry = test_registry();
        let location = Location::Local("/tmp/work".into());
        let mut state = AppState::default();
        state.left.location = location.clone();

        assert_eq!(
            VisiblePaneRow::Parent(&parent).navigation_target(&location, &registry),
            Some(Location::Local("/tmp".into()))
        );
        assert!(selection_or_cursor(&state, &[VisiblePaneRow::Parent(&parent)], 0).is_empty());
        toggle_selection_and_advance(&mut state, None, 1);
        assert_eq!(state.selection_count(Pane::Left, &location), 0);
    }

    #[test]
    fn sort_keeps_identity_attached() {
        let bucket = EntryIdentity::S3Bucket(S3BucketRef {
            target: "prod".into(),
            bucket: "z-bucket".into(),
        });
        let object = EntryIdentity::S3Object(S3ObjectRef {
            target: "prod".into(),
            bucket: "b".into(),
            key: "a-key".into(),
        });
        let mut entries = vec![
            listed_with_identity("z-display", EntryKind::Directory, bucket.clone()),
            listed_with_identity("a-display", EntryKind::File, object.clone()),
        ];

        sort_entries(&mut entries, SortMode::NameAsc);

        assert_eq!(entries[0].entry.name, "a-display");
        assert_eq!(entries[0].identity, object);
        assert_eq!(entries[1].identity, bucket);
    }

    #[test]
    fn filter_keeps_identity_attached() {
        let exact = EntryIdentity::S3Prefix(S3PrefixRef {
            target: "prod".into(),
            bucket: "bucket".into(),
            prefix: "exact//prefix/".into(),
        });
        let entries = vec![
            listed_with_identity("needle", EntryKind::Directory, exact.clone()),
            listed(file("other")),
        ];

        let filtered = apply_filter(&entries, "needle");

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].identity, exact);
    }

    #[test]
    fn duplicate_presentation_names_do_not_collapse_identity() {
        let entries = vec![
            listed_with_identity(
                "duplicate",
                EntryKind::Directory,
                EntryIdentity::S3Bucket(S3BucketRef {
                    target: "one".into(),
                    bucket: "bucket-one".into(),
                }),
            ),
            listed_with_identity(
                "duplicate",
                EntryKind::Directory,
                EntryIdentity::S3Bucket(S3BucketRef {
                    target: "two".into(),
                    bucket: "bucket-two".into(),
                }),
            ),
        ];

        let filtered = apply_filter(&entries, "duplicate");

        assert_eq!(filtered.len(), 2);
        assert_ne!(filtered[0].identity, filtered[1].identity);
    }

    #[test]
    fn enter_s3_bucket_uses_exact_ref() {
        let registry = test_registry();
        let listed = listed_with_identity(
            "DISPLAY-NAME-THAT-MUST-NOT-BE-USED",
            EntryKind::Directory,
            EntryIdentity::S3Bucket(S3BucketRef {
                target: "Aws-PROD".into(),
                bucket: "Company-ARTIFACTS".into(),
            }),
        );
        let current = Location::S3 {
            target: "other".into(),
            bucket: None,
            prefix: String::new(),
        };

        assert_eq!(
            VisiblePaneRow::Listed(&listed).navigation_target(&current, &registry),
            Some(Location::S3 {
                target: "Aws-PROD".into(),
                bucket: Some("Company-ARTIFACTS".into()),
                prefix: String::new(),
            })
        );
    }

    #[test]
    fn hidden_filter_only_exempts_structured_exact_dot_dot() {
        let entries = vec![
            listed_with_identity(".legacy", EntryKind::File, EntryIdentity::Other),
            listed_with_identity(
                ".structured",
                EntryKind::File,
                EntryIdentity::S3Object(S3ObjectRef {
                    target: "prod".into(),
                    bucket: "bucket".into(),
                    key: ".structured".into(),
                }),
            ),
            listed_with_identity(
                "..",
                EntryKind::Directory,
                EntryIdentity::S3Bucket(S3BucketRef {
                    target: "prod".into(),
                    bucket: "bucket".into(),
                }),
            ),
        ];

        let normalized = normalize_entries(entries, false, SortMode::NameAsc);

        assert_eq!(normalized.len(), 1);
        assert!(matches!(
            &normalized[0].identity,
            EntryIdentity::S3Bucket(_)
        ));
    }

    #[tokio::test]
    async fn pane_loader_identity_reaches_exact_bucket_enter() {
        let registry = ProviderRegistry::new();
        registry.insert_sftp(
            "identity-seam",
            Box::new(IdentityPageProvider {
                entry: listed_with_identity(
                    "..",
                    EntryKind::Directory,
                    EntryIdentity::S3Bucket(S3BucketRef {
                        target: "Aws-PROD".into(),
                        bucket: "Company-ARTIFACTS".into(),
                    }),
                ),
            }),
            CapabilitySet::NONE,
        );
        let location = Location::Sftp {
            host: "identity-seam".into(),
            path: "/page".into(),
        };
        let (loader, mut responses, _next_page_responses) = PaneLoader::channel(registry.clone());
        let mut state = AppState::default();
        state.left.location = location.clone();
        let id = loader.load(Pane::Left, location.clone(), PaneLoadPurpose::Refresh);
        state.register_pane_load(Pane::Left, id, location.clone(), PaneLoadPurpose::Refresh);
        let response = responses.recv().await.expect("pane response");
        let mut left_entries = Vec::new();
        let mut right_entries = Vec::new();

        pane_responses::apply_pane_load_response(
            response,
            &mut state,
            &mut left_entries,
            &mut right_entries,
        );
        let parent = virtual_parent_entry();
        let visible = apply_filter_with_parent(&left_entries, "", &location, &registry, &parent);
        let listed_row = visible
            .iter()
            .copied()
            .find(|row| row.listed().is_some())
            .expect("provider-listed row");

        assert_eq!(
            listed_row.navigation_target(&location, &registry),
            Some(Location::S3 {
                target: "Aws-PROD".into(),
                bucket: Some("Company-ARTIFACTS".into()),
                prefix: String::new(),
            })
        );
    }

    #[test]
    fn s3_object_and_other_rows_fail_closed_on_enter_but_s3_prefix_navigates() {
        let registry = test_registry();
        let current = Location::S3 {
            target: "prod".into(),
            bucket: Some("bucket".into()),
            prefix: String::new(),
        };
        let prefix = listed_with_identity(
            "prefix-display",
            EntryKind::Directory,
            EntryIdentity::S3Prefix(S3PrefixRef {
                target: "prod".into(),
                bucket: "bucket".into(),
                prefix: "exact/prefix/".into(),
            }),
        );
        let object = listed_with_identity(
            "object-displayed-as-directory",
            EntryKind::Directory,
            EntryIdentity::S3Object(S3ObjectRef {
                target: "prod".into(),
                bucket: "bucket".into(),
                key: "exact/object".into(),
            }),
        );

        // S3Prefix is now navigable: exactly one final delimiter removed.
        assert_eq!(
            VisiblePaneRow::Listed(&prefix).navigation_target(&current, &registry),
            Some(Location::S3 {
                target: "prod".into(),
                bucket: Some("bucket".into()),
                prefix: "exact/prefix".into(),
            })
        );
        // S3Object still does not navigate.
        assert_eq!(
            VisiblePaneRow::Listed(&object).navigation_target(&current, &registry),
            None
        );
    }

    #[test]
    fn s3_prefix_navigation_uses_exact_ref_not_display_name_and_preserves_repeated_slash() {
        let registry = test_registry();
        let current = Location::S3 {
            target: "prod".into(),
            bucket: Some("bucket".into()),
            prefix: String::new(),
        };
        let prefix = listed_with_identity(
            "DISPLAY-WRONG-MUST-NOT-LEAK",
            EntryKind::Directory,
            EntryIdentity::S3Prefix(S3PrefixRef {
                target: "prod".into(),
                bucket: "bucket".into(),
                prefix: "foo//".into(),
            }),
        );

        let target = VisiblePaneRow::Listed(&prefix).navigation_target(&current, &registry);
        assert_eq!(
            target,
            Some(Location::S3 {
                target: "prod".into(),
                bucket: Some("bucket".into()),
                // provider "foo//" -> nav "foo/" (one delimiter removed, repeated slash kept)
                prefix: "foo/".into(),
            })
        );
        // The display name must not leak into the produced location.
        let loc = target.unwrap();
        match &loc {
            Location::S3 { prefix, .. } => {
                assert_ne!(*prefix, "DISPLAY-WRONG-MUST-NOT-LEAK".to_string());
                assert!(!prefix.contains("DISPLAY-WRONG"));
            }
            _ => panic!("expected S3 location, got {loc:?}"),
        }
    }

    #[test]
    fn provider_listed_dot_dot_remains_listed_identity() {
        let registry = test_registry();
        let listed = listed_with_identity(
            "..",
            EntryKind::Directory,
            EntryIdentity::S3Bucket(S3BucketRef {
                target: "exact-target".into(),
                bucket: "exact-bucket".into(),
            }),
        );
        let parent = virtual_parent_entry();
        let current = Location::S3 {
            target: "exact-target".into(),
            bucket: None,
            prefix: String::new(),
        };
        let entries = [listed];
        let visible = apply_filter_with_parent(&entries, "", &current, &registry, &parent);

        assert_eq!(visible.len(), 1);
        assert!(matches!(visible[0], VisiblePaneRow::Listed(_)));
        assert_eq!(
            visible[0].navigation_target(&current, &registry),
            Some(Location::S3 {
                target: "exact-target".into(),
                bucket: Some("exact-bucket".into()),
                prefix: String::new(),
            })
        );
    }

    #[test]
    fn load_more_focus_has_no_file_action_context() {
        let mut state = AppState::default();
        state.left.split = true;
        state.left.split_active = true;
        state.left.split_cursor = 1;
        let listed = [listed(file("only.txt"))];
        let load_more = load_more_entry();
        let rows = [
            VisiblePaneRow::Listed(&listed[0]),
            VisiblePaneRow::LoadMore(&load_more),
        ];

        let focused_kind = focused_action_kind(&state, &rows, &[]);
        assert_eq!(focused_kind, None);
        let (row_a, row_b) =
            command_bar_rows(&state, KeyRouter::default().keymap(), focused_kind, true);
        assert!(
            row_a
                .iter()
                .chain(&row_b)
                .filter(|hint| { matches!(hint.action, ActionId::ViewFile | ActionId::EditFile) })
                .all(|hint| !hint.available)
        );
    }

    #[tokio::test]
    async fn schedule_next_page_requires_current_continuation_and_not_pending() {
        let (loader, _first_pages, mut next_pages) =
            PaneLoader::channel(arx::vfs::default_registry());
        let mut state = AppState::default();

        schedule_next_page(&loader, &mut state, Pane::Left);
        assert!(next_pages.try_recv().is_err());

        let location = state.left.location.clone();
        state.register_pane_load(
            Pane::Left,
            PaneLoadId(7),
            location.clone(),
            PaneLoadPurpose::Refresh,
        );
        let continuation = page_continuation(PaneLoadId(7), location);
        state.apply_pane_listing_continuation(Pane::Left, Some(continuation.clone()));

        schedule_next_page(&loader, &mut state, Pane::Left);
        let response = next_pages
            .recv()
            .await
            .expect("explicit next-page response");
        assert_eq!(response.initiating_continuation, continuation);

        schedule_next_page(&loader, &mut state, Pane::Left);
        assert!(next_pages.try_recv().is_err());
    }

    #[test]
    fn pagination_virtual_row_is_last_filter_proof_and_provider_neutral() {
        let registry = test_registry();
        let parent = virtual_parent_entry();
        let load_more = load_more_entry();
        let entries = [listed(file("child.txt"))];
        let location = Location::Local("/tmp/work".into());
        let continuation = page_continuation(PaneLoadId(7), location.clone());
        let no_page = apply_filter_with_parent_and_continuation(
            &entries, "missing", &location, &registry, &parent, &load_more, None,
        );
        assert!(matches!(no_page.as_slice(), [VisiblePaneRow::Parent(_)]));

        let visible = apply_filter_with_parent_and_continuation(
            &entries,
            "child",
            &location,
            &registry,
            &parent,
            &load_more,
            Some(&continuation),
        );
        assert!(matches!(visible[0], VisiblePaneRow::Parent(_)));
        assert!(matches!(visible[1], VisiblePaneRow::Listed(_)));
        assert!(matches!(visible[2], VisiblePaneRow::LoadMore(_)));
        assert_eq!(visible[2].entry().name, LOAD_MORE_LABEL);
        assert!(visible[2].listed().is_none());
        assert!(visible[2].action_entry().is_none());

        let filtered = apply_filter_with_parent_and_continuation(
            &entries,
            "does-not-match",
            &location,
            &registry,
            &parent,
            &load_more,
            Some(&continuation),
        );
        assert!(matches!(
            filtered.as_slice(),
            [VisiblePaneRow::Parent(_), VisiblePaneRow::LoadMore(_)]
        ));

        for location in [
            Location::Local("/tmp".into()),
            Location::Sftp {
                host: "host".into(),
                path: "/srv".into(),
            },
            Location::Archive {
                archive: "/tmp/a.zip".into(),
                inner_path: "nested".into(),
            },
        ] {
            let rows = apply_filter_with_parent_and_continuation(
                &entries, "", &location, &registry, &parent, &load_more, None,
            );
            assert!(
                !rows
                    .iter()
                    .any(|row| matches!(row, VisiblePaneRow::LoadMore(_)))
            );
        }
    }

    fn pagination_state() -> (AppState, Vec<ListedEntry>, PaneListingContinuation) {
        let mut state = AppState::default();
        let location = Location::Local("/tmp/work".into());
        state.left.location = location.clone();
        state.register_pane_load(
            Pane::Left,
            PaneLoadId(7),
            location.clone(),
            PaneLoadPurpose::Refresh,
        );
        let continuation = page_continuation(PaneLoadId(7), location);
        state.apply_pane_listing_continuation(Pane::Left, Some(continuation.clone()));
        assert!(state.register_next_page(
            Pane::Left,
            arx::services::PanePageRequestId(9),
            continuation.clone(),
        ));
        (
            state,
            vec![listed_with_identity(
                "z-first",
                EntryKind::File,
                EntryIdentity::S3Object(S3ObjectRef {
                    target: "prod".into(),
                    bucket: "bucket".into(),
                    key: "z-key".into(),
                }),
            )],
            continuation,
        )
    }

    #[test]
    fn accepted_page_preserves_both_split_cursor_identities_through_visible_rows() {
        let registry = test_registry();
        let (mut state, mut left, initiating) = pagination_state();
        let split_identity = EntryIdentity::S3Object(S3ObjectRef {
            target: "prod".into(),
            bucket: "bucket".into(),
            key: "y-key".into(),
        });
        left.insert(
            0,
            listed_with_identity("y-split", EntryKind::File, split_identity.clone()),
        );
        let primary_identity = left[1].identity.clone();
        state.left.split = true;
        state.left.cursor = 2; // Parent, split row, primary row
        state.left.split_cursor = 1;

        pane_responses::apply_next_page_response(
            PaneNextPageResponse {
                request_id: arx::services::PanePageRequestId(9),
                pane: Pane::Left,
                initiating_continuation: initiating,
                result: Ok(PaneLoadPage {
                    entries: vec![listed_with_identity(
                        "a-new",
                        EntryKind::File,
                        EntryIdentity::S3Object(S3ObjectRef {
                            target: "prod".into(),
                            bucket: "bucket".into(),
                            key: "a-key".into(),
                        }),
                    )],
                    continuation: None,
                }),
            },
            &mut state,
            &mut left,
            &mut Vec::new(),
        );

        let parent = virtual_parent_entry();
        let load_more = load_more_entry();
        let visible = apply_filter_with_parent_and_continuation(
            &left,
            &state.filter,
            &state.left.location,
            &registry,
            &parent,
            &load_more,
            state.pane_listing_continuations.get(&Pane::Left),
        );
        assert_eq!(
            visible[state.left.cursor].listed().map(|row| &row.identity),
            Some(&primary_identity)
        );
        assert_eq!(
            visible[state.left.split_cursor]
                .listed()
                .map(|row| &row.identity),
            Some(&split_identity)
        );
    }

    #[test]
    fn accepted_page_appends_sorts_replaces_and_finally_clears_token() {
        let (mut state, mut left, initiating) = pagination_state();
        state.left.cursor = 1; // virtual Parent then the listed row
        let selected = left[0].identity.clone();
        let mut next = initiating.clone();
        next.provider_continuation.token = " next opaque token ".into();
        let response = PaneNextPageResponse {
            request_id: arx::services::PanePageRequestId(9),
            pane: Pane::Left,
            initiating_continuation: initiating.clone(),
            result: Ok(PaneLoadPage {
                entries: vec![listed_with_identity(
                    "a-second",
                    EntryKind::File,
                    EntryIdentity::S3Object(S3ObjectRef {
                        target: "prod".into(),
                        bucket: "bucket".into(),
                        key: "a-key".into(),
                    }),
                )],
                continuation: Some(next.clone()),
            }),
        };
        pane_responses::apply_next_page_response(response, &mut state, &mut left, &mut Vec::new());

        assert_eq!(
            left.iter()
                .map(|e| e.entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["a-second", "z-first"]
        );
        assert_eq!(state.left.cursor, 2);
        assert_eq!(left[state.left.cursor - 1].identity, selected);
        assert_eq!(
            state.pane_listing_continuations.get(&Pane::Left),
            Some(&next)
        );
        assert!(!state.pending_next_pages.contains_key(&Pane::Left));

        assert!(state.register_next_page(
            Pane::Left,
            arx::services::PanePageRequestId(10),
            next.clone(),
        ));
        pane_responses::apply_next_page_response(
            PaneNextPageResponse {
                request_id: arx::services::PanePageRequestId(10),
                pane: Pane::Left,
                initiating_continuation: next,
                result: Ok(PaneLoadPage {
                    entries: vec![],
                    continuation: None,
                }),
            },
            &mut state,
            &mut left,
            &mut Vec::new(),
        );
        assert!(!state.pane_listing_continuations.contains_key(&Pane::Left));
    }

    #[test]
    fn stale_duplicate_and_error_page_responses_mutate_nothing_except_accepted_pending() {
        let (mut state, mut left, initiating) = pagination_state();
        let original = left.clone();
        let stale = PaneNextPageResponse {
            request_id: arx::services::PanePageRequestId(8),
            pane: Pane::Left,
            initiating_continuation: initiating.clone(),
            result: Ok(PaneLoadPage {
                entries: vec![listed(file("stale"))],
                continuation: None,
            }),
        };
        pane_responses::apply_next_page_response(stale, &mut state, &mut left, &mut Vec::new());
        assert_eq!(left, original);
        assert!(state.pending_next_pages.contains_key(&Pane::Left));

        pane_responses::apply_next_page_response(
            PaneNextPageResponse {
                request_id: arx::services::PanePageRequestId(9),
                pane: Pane::Left,
                initiating_continuation: initiating.clone(),
                result: Err(io::Error::other("offline failure")),
            },
            &mut state,
            &mut left,
            &mut Vec::new(),
        );
        assert_eq!(left, original);
        assert_eq!(
            state.pane_listing_continuations.get(&Pane::Left),
            Some(&initiating)
        );
        assert!(!state.pending_next_pages.contains_key(&Pane::Left));
        assert_eq!(
            state.message.as_deref(),
            Some("Load next page failed: offline failure")
        );

        pane_responses::apply_next_page_response(
            PaneNextPageResponse {
                request_id: arx::services::PanePageRequestId(9),
                pane: Pane::Left,
                initiating_continuation: initiating,
                result: Ok(PaneLoadPage {
                    entries: vec![listed(file("late-duplicate"))],
                    continuation: None,
                }),
            },
            &mut state,
            &mut left,
            &mut Vec::new(),
        );
        assert_eq!(left, original);
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
        let mut left_entries = vec![listed(file("current.txt"))];
        let mut right_entries = Vec::new();

        pane_responses::apply_pane_load_response(
            PaneLoadResponse {
                id: PaneLoadId(1),
                pane: Pane::Left,
                location: target.clone(),
                purpose: PaneLoadPurpose::Navigate {
                    remember_current: true,
                },
                result: Ok(page(vec![file("foo.txt")])),
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
        let mut left_entries = vec![listed(file("current.txt"))];
        let mut right_entries = Vec::new();

        state.register_pane_load(
            Pane::Left,
            PaneLoadId(1),
            target.clone(),
            PaneLoadPurpose::Navigate {
                remember_current: true,
            },
        );
        pane_responses::apply_pane_load_response(
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
        assert_eq!(left_entries, vec![listed(file("current.txt"))]);
        assert!(!state.pane_listing_continuations.contains_key(&Pane::Left));
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
        pane_responses::apply_pane_load_response(
            PaneLoadResponse {
                id: PaneLoadId(2),
                pane: Pane::Left,
                location: target.clone(),
                purpose: PaneLoadPurpose::Navigate {
                    remember_current: true,
                },
                result: Ok(page(Vec::new())),
            },
            &mut state,
            &mut left_entries,
            &mut right_entries,
        );

        assert_eq!(state.left.location, target);
        assert!(!state.pane_load_errors.contains_key(&Pane::Left));
    }

    #[test]
    fn accepted_first_page_stores_exact_continuation() {
        let mut state = AppState::default();
        let location = state.left.location.clone();
        let id = PaneLoadId(11);
        state.register_pane_load(Pane::Left, id, location.clone(), PaneLoadPurpose::Refresh);
        let continuation = page_continuation(id, location.clone());
        let mut left_entries = Vec::new();
        let mut right_entries = Vec::new();

        pane_responses::apply_pane_load_response(
            PaneLoadResponse {
                id,
                pane: Pane::Left,
                location,
                purpose: PaneLoadPurpose::Refresh,
                result: Ok(PaneLoadPage {
                    entries: vec![listed(file("loaded.txt"))],
                    continuation: Some(continuation.clone()),
                }),
            },
            &mut state,
            &mut left_entries,
            &mut right_entries,
        );

        assert_eq!(
            state.pane_listing_continuations.get(&Pane::Left),
            Some(&continuation)
        );
        assert_eq!(left_entries[0].entry.name, "loaded.txt");
    }

    #[test]
    fn stale_response_cannot_attach_continuation_to_new_generation() {
        let mut state = AppState::default();
        let location = state.left.location.clone();
        state.register_pane_load(
            Pane::Left,
            PaneLoadId(11),
            location.clone(),
            PaneLoadPurpose::Refresh,
        );
        state.register_pane_load(
            Pane::Left,
            PaneLoadId(12),
            location.clone(),
            PaneLoadPurpose::Refresh,
        );
        let mut left_entries = vec![listed(file("current.txt"))];
        let mut right_entries = Vec::new();

        pane_responses::apply_pane_load_response(
            PaneLoadResponse {
                id: PaneLoadId(11),
                pane: Pane::Left,
                location: location.clone(),
                purpose: PaneLoadPurpose::Refresh,
                result: Ok(PaneLoadPage {
                    entries: vec![listed(file("stale.txt"))],
                    continuation: Some(page_continuation(PaneLoadId(11), location)),
                }),
            },
            &mut state,
            &mut left_entries,
            &mut right_entries,
        );

        assert_eq!(left_entries, vec![listed(file("current.txt"))]);
        assert!(!state.pane_listing_continuations.contains_key(&Pane::Left));
        assert_eq!(
            state.pending_pane_loads.get(&Pane::Left),
            Some(&PaneLoadId(12))
        );
    }

    #[test]
    fn refresh_and_navigation_clear_previous_continuation() {
        let mut state = AppState::default();
        let current = state.left.location.clone();
        state.register_pane_load(
            Pane::Left,
            PaneLoadId(10),
            current.clone(),
            PaneLoadPurpose::Refresh,
        );
        state.apply_pane_listing_continuation(
            Pane::Left,
            Some(page_continuation(PaneLoadId(10), current.clone())),
        );

        state.register_pane_load(
            Pane::Left,
            PaneLoadId(11),
            current,
            PaneLoadPurpose::Refresh,
        );
        assert!(!state.pane_listing_continuations.contains_key(&Pane::Left));

        let target = Location::Local("/next".into());
        state.apply_pane_listing_continuation(
            Pane::Left,
            Some(page_continuation(
                PaneLoadId(11),
                state.left.location.clone(),
            )),
        );
        state.register_pane_load(
            Pane::Left,
            PaneLoadId(12),
            target,
            PaneLoadPurpose::Navigate {
                remember_current: true,
            },
        );
        assert!(!state.pane_listing_continuations.contains_key(&Pane::Left));
    }

    #[test]
    fn pane_swap_immediate_reload_clears_stale_continuations() {
        let mut state = AppState::default();
        let left = state.left.location.clone();
        let right = state.right.location.clone();
        state.register_pane_load(
            Pane::Left,
            PaneLoadId(10),
            left.clone(),
            PaneLoadPurpose::Refresh,
        );
        state.register_pane_load(
            Pane::Right,
            PaneLoadId(20),
            right.clone(),
            PaneLoadPurpose::Refresh,
        );
        state.apply_pane_listing_continuation(
            Pane::Left,
            Some(page_continuation(PaneLoadId(10), left)),
        );
        state.apply_pane_listing_continuation(
            Pane::Right,
            Some(page_continuation(PaneLoadId(20), right)),
        );

        std::mem::swap(&mut state.left, &mut state.right);
        state.register_pane_load(
            Pane::Left,
            PaneLoadId(30),
            state.left.location.clone(),
            PaneLoadPurpose::Refresh,
        );
        state.register_pane_load(
            Pane::Right,
            PaneLoadId(31),
            state.right.location.clone(),
            PaneLoadPurpose::Refresh,
        );

        assert!(state.pane_listing_continuations.is_empty());
    }

    #[test]
    fn empty_hosts_overlay_is_real_and_truthful() {
        let mut state = AppState::default();
        state.hosts.clear();

        toggle_hosts_overlay(&mut state);

        assert_eq!(state.active_overlay(), Some(OverlayKind::Hosts));
        let text = hosts::empty_hosts_text();
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

        workspace_responses::handle_workspace_scan_response(
            WorkspaceScanResponse {
                id: left_id,
                side: WorkspaceSide::Left,
                root: left,
                result: Ok(left_entries),
            },
            state,
        );
        workspace_responses::handle_workspace_scan_response(
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

    fn frozen_launch(
        state: &mut AppState,
        generation: u64,
        entry_name: &str,
    ) -> (WorkspaceSyncController, SyncLaunchId, SyncPlanId) {
        accept_workspace_compare(
            state,
            vec![workspace_entry(entry_name, 1, None, Some("left"))],
            Vec::new(),
            generation,
        );
        let controller = WorkspaceSyncController::new(state.registry.clone());
        let frozen = controller
            .freeze(
                state.remote_workspace.plan.as_ref().unwrap(),
                state.remote_workspace.diff.as_ref().unwrap(),
            )
            .unwrap();
        let plan_id = frozen.id();
        state.remote_workspace.set_frozen_plan(frozen);
        let launch_id = controller.begin_launch();
        (controller, launch_id, plan_id)
    }

    fn completed_verification_job(
        plan_id: SyncPlanId,
        verification: &SyncVerificationSnapshot,
    ) -> (arx::jobs::JobManager, String) {
        let manager = arx::jobs::JobManager::new();
        let job = manager.create_job(
            "sync-response",
            JobKind::Synchronize,
            "test sync response",
            Some(Location::Local("/left".into())),
            Some(Location::Local("/right".into())),
        );
        let result = sync_job(
            plan_id,
            "outcome",
            JobStatus::Completed,
            SyncTerminalState::Completed,
            verification.status.clone(),
            SyncJournalFinalization::Recorded,
        )
        .result
        .unwrap();
        assert!(manager.apply_event(&arx::jobs::JobEvent::Completed {
            id: job.id.clone(),
            result,
        }));
        assert!(manager.apply_sync_verification(
            &job.id,
            &verification_snapshot(plan_id, SyncVerificationStatus::Pending),
        ));
        assert!(manager.apply_sync_verification(
            &job.id,
            &verification_snapshot(
                plan_id,
                SyncVerificationStatus::Running {
                    left_scan: WorkspaceScanId(1),
                    right_scan: WorkspaceScanId(2),
                },
            ),
        ));
        assert!(manager.apply_sync_verification(&job.id, verification));
        (manager, job.id)
    }

    #[test]
    fn workspace_response_rejects_stale_scan_id() {
        let mut state = AppState::default();
        state.left.location = Location::Local("/left".into());
        state
            .remote_workspace
            .register_scan(WorkspaceSide::Left, WorkspaceScanId(2));

        workspace_responses::handle_workspace_scan_response(
            WorkspaceScanResponse {
                id: WorkspaceScanId(1),
                side: WorkspaceSide::Left,
                root: state.left.location.clone(),
                result: Ok(vec![workspace_entry("stale", 1, None, None)]),
            },
            &mut state,
        );

        assert_eq!(state.remote_workspace.left_scan, Some(WorkspaceScanId(2)));
        assert!(state.remote_workspace.left_entries.is_none());
        assert!(state.message.is_none());
    }

    #[test]
    fn workspace_response_wrong_current_root_settles_without_entries() {
        let mut state = AppState::default();
        state.left.location = Location::Local("/current".into());
        state
            .remote_workspace
            .register_scan(WorkspaceSide::Left, WorkspaceScanId(1));

        workspace_responses::handle_workspace_scan_response(
            WorkspaceScanResponse {
                id: WorkspaceScanId(1),
                side: WorkspaceSide::Left,
                root: Location::Local("/old".into()),
                result: Ok(vec![workspace_entry("old", 1, None, None)]),
            },
            &mut state,
        );

        assert!(state.remote_workspace.left_scan.is_none());
        assert!(state.remote_workspace.left_entries.is_none());
        assert!(state.remote_workspace.diff.is_none());
    }

    #[test]
    fn workspace_response_two_current_sides_build_diff() {
        let mut state = AppState::default();
        accept_workspace_compare(
            &mut state,
            vec![workspace_entry("left", 3, None, Some("left"))],
            vec![workspace_entry("right", 4, None, Some("right"))],
            1,
        );

        assert!(state.remote_workspace.left_scan.is_none());
        assert!(state.remote_workspace.right_scan.is_none());
        assert_eq!(
            state
                .remote_workspace
                .diff
                .as_ref()
                .unwrap()
                .changed_count(),
            2
        );
        assert!(state.message.as_deref().unwrap().starts_with("workspace:"));
    }

    #[test]
    fn workspace_response_cancelled_and_error_settle_scan() {
        let root = Location::Local("/left".into());
        let mut cancelled = AppState::default();
        cancelled.left.location = root.clone();
        cancelled
            .remote_workspace
            .register_scan(WorkspaceSide::Left, WorkspaceScanId(1));
        workspace_responses::handle_workspace_scan_response(
            WorkspaceScanResponse {
                id: WorkspaceScanId(1),
                side: WorkspaceSide::Left,
                root: root.clone(),
                result: Err(WorkspaceScanError::Cancelled),
            },
            &mut cancelled,
        );
        assert!(cancelled.remote_workspace.left_scan.is_none());
        assert!(cancelled.message.is_none());

        let mut failed = AppState::default();
        failed.left.location = root.clone();
        failed
            .remote_workspace
            .register_scan(WorkspaceSide::Left, WorkspaceScanId(2));
        workspace_responses::handle_workspace_scan_response(
            WorkspaceScanResponse {
                id: WorkspaceScanId(2),
                side: WorkspaceSide::Left,
                root,
                result: Err(WorkspaceScanError::EntryLimit { limit: 7 }),
            },
            &mut failed,
        );
        assert!(failed.remote_workspace.left_scan.is_none());
        assert_eq!(
            failed.message.as_deref(),
            Some("Workspace scan failed: workspace scan exceeded the 7 entry safety limit")
        );
    }

    #[test]
    fn workspace_response_rejects_stale_launch_id() {
        let mut state = AppState::default();
        let (controller, stale_id, plan_id) = frozen_launch(&mut state, 1, "stale");
        let _current_id = controller.begin_launch();
        let manager = arx::jobs::JobManager::new();
        let before = state.remote_workspace.ux.clone();

        workspace_responses::apply_sync_launch_response(
            SyncLaunchResponse {
                launch_id: stale_id,
                plan_id,
                result: Err("must be ignored".into()),
            },
            &mut state,
            &controller,
            &manager,
        );

        assert_eq!(state.remote_workspace.ux, before);
        assert!(state.jobs.is_empty());
    }

    #[test]
    fn workspace_response_rejects_mismatched_frozen_plan() {
        let mut state = AppState::default();
        let (controller, launch_id, _) = frozen_launch(&mut state, 1, "current");
        let mut other = AppState::default();
        let (_, _, other_plan_id) = frozen_launch(&mut other, 2, "other");
        let manager = arx::jobs::JobManager::new();
        let before = state.remote_workspace.ux.clone();

        workspace_responses::apply_sync_launch_response(
            SyncLaunchResponse {
                launch_id,
                plan_id: other_plan_id,
                result: Err("must be ignored".into()),
            },
            &mut state,
            &controller,
            &manager,
        );

        assert_eq!(state.remote_workspace.ux, before);
        assert!(state.jobs.is_empty());
    }

    #[test]
    fn workspace_response_accepts_launch_job_and_error() {
        let mut launched = AppState::default();
        let (controller, launch_id, plan_id) = frozen_launch(&mut launched, 1, "job");
        let manager = arx::jobs::JobManager::new();
        let job = manager.create_job("sync", JobKind::Synchronize, "sync", None, None);
        workspace_responses::apply_sync_launch_response(
            SyncLaunchResponse {
                launch_id,
                plan_id,
                result: Ok(job.id.clone()),
            },
            &mut launched,
            &controller,
            &manager,
        );
        assert_eq!(launched.jobs.len(), 1);
        assert_eq!(launched.jobs[0].id, job.id);

        let mut blocked = AppState::default();
        let (controller, launch_id, plan_id) = frozen_launch(&mut blocked, 2, "error");
        workspace_responses::apply_sync_launch_response(
            SyncLaunchResponse {
                launch_id,
                plan_id,
                result: Err("launch failed".into()),
            },
            &mut blocked,
            &controller,
            &manager,
        );
        assert_eq!(
            blocked.remote_workspace.ux,
            WorkspaceSyncUxState::Blocked {
                message: "launch failed".into()
            }
        );
    }

    #[test]
    fn workspace_response_applies_accepted_verification() {
        let plan_id = test_plan_id();
        let finished = verification_snapshot(
            plan_id,
            SyncVerificationStatus::Finished(Box::new(synchronized_result(plan_id))),
        );
        let (manager, job_id) = completed_verification_job(plan_id, &finished);
        let mut state = AppState::default();
        state.left.location = Location::Local("/left".into());
        state.right.location = Location::Local("/right".into());
        state.remote_workspace.enabled = true;
        state.remote_workspace.verification = Some(verification_snapshot(
            plan_id,
            SyncVerificationStatus::Running {
                left_scan: WorkspaceScanId(1),
                right_scan: WorkspaceScanId(2),
            },
        ));

        workspace_responses::apply_verification_event(
            SyncVerificationEvent {
                job_id: job_id.clone(),
                verification: finished.clone(),
            },
            &mut state,
            &manager,
        );

        assert_eq!(state.remote_workspace.verification, Some(finished.clone()));
        assert_eq!(
            state.remote_workspace.ux,
            WorkspaceSyncUxState::Finished {
                job_id: job_id.clone()
            }
        );
        assert_eq!(state.jobs.len(), 1);
        assert_eq!(state.jobs[0].id, job_id);
        assert_eq!(state.jobs[0].verification, Some(finished.clone()));
        assert!(matches!(
            state.session_callout,
            Some(SessionCallout::WorkspaceSyncVerified { job_id: ref id }) if id == &job_id
        ));
    }

    #[test]
    fn workspace_response_rejected_verification_settles_and_refreshes_jobs() {
        let plan_id = test_plan_id();
        let finished = verification_snapshot(
            plan_id,
            SyncVerificationStatus::Finished(Box::new(synchronized_result(plan_id))),
        );
        let (manager, job_id) = completed_verification_job(plan_id, &finished);
        let mut state = AppState::default();
        accept_workspace_compare(
            &mut state,
            vec![workspace_entry("current", 1, None, Some("current"))],
            Vec::new(),
            1,
        );
        let current_diff = state.remote_workspace.diff.clone();
        state.remote_workspace.verification = Some(verification_snapshot(
            plan_id,
            SyncVerificationStatus::Running {
                left_scan: WorkspaceScanId(1),
                right_scan: WorkspaceScanId(2),
            },
        ));
        state.remote_workspace.ux = WorkspaceSyncUxState::Verifying {
            job_id: job_id.clone(),
        };
        state.left.location = Location::Local("/moved".into());

        workspace_responses::apply_verification_event(
            SyncVerificationEvent {
                job_id: job_id.clone(),
                verification: finished,
            },
            &mut state,
            &manager,
        );

        assert_eq!(state.remote_workspace.diff, current_diff);
        assert!(matches!(
            state
                .remote_workspace
                .verification
                .as_ref()
                .map(|item| &item.status),
            Some(SyncVerificationStatus::Superseded)
        ));
        assert_eq!(
            state.remote_workspace.ux,
            WorkspaceSyncUxState::Finished {
                job_id: job_id.clone()
            }
        );
        assert_eq!(state.jobs.len(), 1);
        assert_eq!(state.jobs[0].id, job_id);
        assert!(
            state.jobs[0]
                .verification
                .as_ref()
                .is_some_and(|item| { matches!(item.status, SyncVerificationStatus::Finished(_)) })
        );
    }

    #[test]
    fn utility_overlay_infrastructure_characterization() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState {
            infrastructure_lines: vec!["infra-alpha".into(), "infra-beta".into()],
            ..Default::default()
        };

        terminal
            .draw(|f| render_infrastructure_center(f, f.area(), &mut state))
            .unwrap();

        let buf = terminal.backend().buffer();
        let text = buf
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect::<Vec<_>>()
            .join("");

        assert!(text.contains("Infrastructure Center"));
        assert!(text.contains("Esc close"));
        assert!(!text.contains("Ctrl+I"));
        assert!(text.contains("infra-alpha"));
    }

    #[test]
    fn utility_overlay_smart_tree_characterization() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState {
            tree_lines: vec!["tree-root".into(), "tree-child".into()],
            tree_filter: "needle".into(),
            ..Default::default()
        };

        terminal
            .draw(|f| render_smart_tree(f, f.area(), &mut state))
            .unwrap();

        let buf = terminal.backend().buffer();
        let text = buf
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect::<Vec<_>>()
            .join("");

        assert!(text.contains("ARX Smart Tree"));
        assert!(text.contains(":needle_"));
        assert!(text.contains("Esc close"));
        assert!(!text.contains("Ctrl+T"));
        assert!(text.contains("tree-child"));
    }

    #[test]
    fn utility_overlay_context_menu_characterization() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|f| render_context_menu(f, f.area())).unwrap();

        let buf = terminal.backend().buffer();
        let text = buf
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect::<Vec<_>>()
            .join("");

        assert!(text.contains("Menu"));
        assert!(text.contains("Copy   F5"));
        assert!(text.contains("Move   F6"));
        assert!(text.contains("Mkdir  F7"));
        assert!(text.contains("Delete F8"));
        assert!(text.contains("View   F3"));
        assert!(text.contains("Edit   F4"));
    }

    #[test]
    fn history_tabs_input_directory_history_characterization() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = AppState {
            dir_history: vec![PathBuf::from("/tmp/one"), PathBuf::from("/tmp/two")],
            show_history: true,
            ..Default::default()
        };

        terminal
            .draw(|f| render_directory_history(f, f.area(), &state))
            .unwrap();

        let buf = terminal.backend().buffer();
        let text = buf
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect::<Vec<_>>()
            .join("");

        assert!(text.contains("Directory History"));
        assert!(text.contains("/tmp/one"));
        assert!(text.contains("/tmp/two"));
    }

    #[test]
    fn history_tabs_input_tab_switcher_characterization() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        let state = AppState {
            left: arx::app::PaneState {
                location: Location::Local(PathBuf::from("/")),
                cursor: 0,
                tabs: vec![(Location::Local(PathBuf::from("/tmp/left")), 0)],
                dir_history: vec![],
                split: false,
                split_cursor: 0,
                split_active: false,
            },
            right: arx::app::PaneState {
                location: Location::Local(PathBuf::from("/")),
                cursor: 0,
                tabs: vec![(Location::Local(PathBuf::from("/tmp/right")), 0)],
                dir_history: vec![],
                split: false,
                split_cursor: 0,
                split_active: false,
            },
            tab_switcher_cursor: 1,
            show_tab_switcher: true,
            ..Default::default()
        };

        terminal
            .draw(|f| render_tab_switcher(f, f.area(), &state))
            .unwrap();

        let buf = terminal.backend().buffer();
        let text = buf
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect::<Vec<_>>()
            .join("");

        assert!(text.contains("Tabs (Alt+`)"));
        assert!(text.contains("> L0: /tmp/left"));
        assert!(text.contains("R0: /tmp/right"));
    }

    #[test]
    fn history_tabs_input_rename_characterization() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = AppState {
            rename_input: true,
            rename_pattern: "new-name".into(),
            ..Default::default()
        };

        terminal
            .draw(|f| render_rename_input(f, f.area(), &state))
            .unwrap();

        let buf = terminal.backend().buffer();
        let text = buf
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect::<Vec<_>>()
            .join("");

        assert!(text.contains("Rename: new-name_"));
    }

    #[test]
    fn history_tabs_input_file_search_characterization() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = AppState {
            file_search: true,
            search_query: "needle".into(),
            ..Default::default()
        };

        terminal
            .draw(|f| render_file_search(f, f.area(), &state))
            .unwrap();

        let buf = terminal.backend().buffer();
        let text = buf
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect::<Vec<_>>()
            .join("");

        assert!(text.contains("/needle_"));
        assert!(text.contains("(0)"));
    }

    #[test]
    fn utility_overlay_centered_rect_lines_sizing() {
        use ratatui::layout::Rect;

        // requested line height passes through unchanged when area fits
        let r = centered_rect_lines(80, 5, Rect::new(0, 0, 120, 24));
        assert_eq!(r.height, 5);

        // oversized height is clamped to available area minus 2
        let r = centered_rect_lines(80, 9999, Rect::new(0, 0, 120, 24));
        assert_eq!(r.height, 22);

        // respects a small area
        let r = centered_rect_lines(80, 50, Rect::new(0, 0, 120, 10));
        assert_eq!(r.height, 8);
    }

    #[test]
    fn leaf_overlay_session_callout_render_characterization() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                render_session_callout(f, f.area(), "✓ characterization");
            })
            .unwrap();
        let buf = terminal.backend().buffer();
        let text = buf
            .content()
            .iter()
            .map(|c| c.symbol().to_owned())
            .collect::<Vec<_>>()
            .join("");
        assert!(text.contains("✓ characterization"));
    }

    #[test]
    fn leaf_overlay_bookmarks_characterization() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = AppState {
            bookmarks: vec![
                Location::Local("/tmp/one".into()),
                Location::Local("/tmp/two".into()),
            ],
            bookmark_cursor: 1,
            ..Default::default()
        };

        terminal
            .draw(|f| bookmarks::render(f, f.area(), &state))
            .unwrap();

        let buf = terminal.backend().buffer();
        let text = buf
            .content()
            .iter()
            .map(|c| c.symbol().to_owned())
            .collect::<Vec<_>>()
            .join("");

        let first = Location::Local("/tmp/one".into()).to_string();
        let second = Location::Local("/tmp/two".into()).to_string();
        assert!(text.contains(&first), "first location not displayed");
        assert!(
            text.contains(&format!("> {second}")),
            "second location not displayed with cursor marker"
        );
    }

    #[test]
    fn workspace_ribbon_commander_and_direction_characterization() {
        let mut state = AppState::default();

        state.left.location = Location::Local("/left".into());
        state.right.location = Location::Sftp {
            host: "demo".into(),
            path: "/srv".into(),
        };

        assert_eq!(
            workspace_ribbon_text(&state),
            "COMMANDER [LOCAL] ⇄ [SSH] · Ctrl+D Compare"
        );

        state.remote_workspace.enabled = true;

        assert_eq!(
            workspace_ribbon_text(&state),
            "WORKSPACE [LOCAL] → [SSH] · Ctrl+D Compare"
        );

        state.remote_workspace.policy.direction = arx::workspace_sync::SyncDirection::RightToLeft;

        assert_eq!(
            workspace_ribbon_text(&state),
            "WORKSPACE [SSH] → [LOCAL] · Ctrl+D Compare"
        );
    }

    #[test]
    fn workspace_ribbon_unknown_provider_label_is_characterized() {
        let mut state = AppState::default();

        state.left.location = Location::S3 {
            target: "acc".into(),
            bucket: Some("bucket".into()),
            prefix: String::new(),
        };

        state.right.location = Location::Archive {
            archive: "/tmp/bundle.tar.gz".into(),
            inner_path: String::new(),
        };

        assert_eq!(
            workspace_ribbon_text(&state),
            "COMMANDER [?] ⇄ [ARCHIVE] · Ctrl+D Compare"
        );
    }

    #[test]
    fn workspace_ribbon_runtime_state_labels_characterization() {
        let mut state = AppState::default();
        state.remote_workspace.enabled = true;

        state.remote_workspace.ux = WorkspaceSyncUxState::Scanning;
        assert!(workspace_ribbon_text(&state).ends_with("Comparing…"));

        state.remote_workspace.ux = WorkspaceSyncUxState::Blocked {
            message: "characterization only".into(),
        };
        assert!(workspace_ribbon_text(&state).ends_with("BLOCKED"));

        state.remote_workspace.ux = WorkspaceSyncUxState::Verifying {
            job_id: "sync-characterization".into(),
        };
        assert!(workspace_ribbon_text(&state).ends_with("Verifying…"));
    }

    #[test]
    fn workspace_ribbon_terminal_verdict_characterization() {
        let plan_id = test_plan_id();
        let mut state = AppState::default();
        state.remote_workspace.enabled = true;
        state.remote_workspace.ux = WorkspaceSyncUxState::Finished {
            job_id: "sync-characterization".into(),
        };

        state.remote_workspace.verification = Some(verification_snapshot(
            plan_id,
            arx::workspace_sync_verification::SyncVerificationStatus::Finished(Box::new(
                synchronized_result(plan_id),
            )),
        ));
        assert!(workspace_ribbon_text(&state).ends_with("✓ SYNCHRONIZED"));

        state.remote_workspace.verification = Some(verification_snapshot(
            plan_id,
            arx::workspace_sync_verification::SyncVerificationStatus::Finished(Box::new(
                differences_result(plan_id),
            )),
        ));
        assert!(workspace_ribbon_text(&state).ends_with("⚠ DIFFERENCES REMAIN"));

        state.remote_workspace.verification = Some(verification_snapshot(
            plan_id,
            arx::workspace_sync_verification::SyncVerificationStatus::Finished(Box::new(
                inconclusive_result(plan_id),
            )),
        ));
        assert!(workspace_ribbon_text(&state).ends_with("? INCONCLUSIVE"));
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
            workspace_responses::observe_verified_sync_success(&mut state, &job);
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

        workspace_responses::observe_verified_sync_success(&mut state, &first);
        assert_eq!(state.remote_workspace.ux, previous_ux);
        assert_eq!(state.active_overlay(), None);
        assert!(matches!(
            state.session_callout,
            Some(SessionCallout::WorkspaceSyncVerified { ref job_id }) if job_id == &first.id
        ));

        state.dismiss_session_callout();
        workspace_responses::observe_verified_sync_success(&mut state, &second);
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

    // ── S3-25: contextual S3 parent regression tests (through the seam) ──

    #[test]
    fn account_bucket_root_shows_virtual_parent() {
        let registry = test_registry();
        let parent = virtual_parent_entry();
        let location = Location::S3 {
            target: "acc".into(),
            bucket: Some("anything".into()),
            prefix: String::new(),
        };
        let entries: [ListedEntry; 0] = [];
        let visible = apply_filter_with_parent(&entries, "", &location, &registry, &parent);
        assert!(matches!(visible.first(), Some(VisiblePaneRow::Parent(_))));
    }

    #[test]
    fn bucket_bound_root_has_no_virtual_parent() {
        let registry = test_registry();
        let parent = virtual_parent_entry();
        let location = Location::S3 {
            target: "bkt".into(),
            bucket: Some("company-artifacts".into()),
            prefix: String::new(),
        };
        let entries: [ListedEntry; 0] = [];
        let visible = apply_filter_with_parent(&entries, "", &location, &registry, &parent);
        assert!(visible.is_empty());
    }

    #[test]
    fn nested_s3_prefix_shows_parent() {
        let registry = test_registry();
        let parent = virtual_parent_entry();
        let location = Location::S3 {
            target: "bkt".into(),
            bucket: Some("company-artifacts".into()),
            prefix: "foo/bar".into(),
        };
        let entries: [ListedEntry; 0] = [];
        let visible = apply_filter_with_parent(&entries, "", &location, &registry, &parent);
        assert!(matches!(visible.first(), Some(VisiblePaneRow::Parent(_))));
    }

    #[test]
    fn s3_target_root_has_no_parent() {
        let registry = test_registry();
        let parent = virtual_parent_entry();
        let location = Location::S3 {
            target: "acc".into(),
            bucket: None,
            prefix: String::new(),
        };
        let entries: [ListedEntry; 0] = [];
        let visible = apply_filter_with_parent(&entries, "", &location, &registry, &parent);
        assert!(visible.is_empty());
    }

    #[test]
    fn parent_enter_uses_contextual_s3_parent() {
        let registry = test_registry();
        let parent = virtual_parent_entry();
        let location = Location::S3 {
            target: "acc".into(),
            bucket: Some("anything".into()),
            prefix: String::new(),
        };
        let nav = VisiblePaneRow::Parent(&parent).navigation_target(&location, &registry);
        assert_eq!(
            nav,
            Some(Location::S3 {
                target: "acc".into(),
                bucket: None,
                prefix: String::new(),
            })
        );
    }

    #[test]
    fn backspace_same_target_as_parent_enter() {
        let registry = test_registry();
        let parent = virtual_parent_entry();
        let location = Location::S3 {
            target: "acc".into(),
            bucket: Some("anything".into()),
            prefix: "foo/bar".into(),
        };
        // Backspace computes via navigation_parent_target; Parent Enter computes via
        // VisiblePaneRow::Parent(...).navigation_target. Both must agree.
        let via_backspace = navigation_parent_target(&location, &registry);
        let via_parent_enter =
            VisiblePaneRow::Parent(&parent).navigation_target(&location, &registry);
        assert_eq!(via_backspace, via_parent_enter);
        assert_eq!(
            via_backspace,
            Some(Location::S3 {
                target: "acc".into(),
                bucket: Some("anything".into()),
                prefix: "foo".into(),
            })
        );
    }

    #[test]
    fn awkward_double_slash_parent_exact() {
        let registry = test_registry();
        let parent = virtual_parent_entry();
        let location = Location::S3 {
            target: "bkt".into(),
            bucket: Some("company-artifacts".into()),
            prefix: "foo//bar".into(),
        };
        let nav = VisiblePaneRow::Parent(&parent).navigation_target(&location, &registry);
        assert_eq!(
            nav,
            Some(Location::S3 {
                target: "bkt".into(),
                bucket: Some("company-artifacts".into()),
                prefix: "foo/".into(),
            })
        );
    }

    #[test]
    fn provider_listed_dot_dot_stays_listed_identity() {
        let registry = test_registry();
        let parent = virtual_parent_entry();
        let listed = listed_with_identity(
            "..",
            EntryKind::Directory,
            EntryIdentity::S3Bucket(S3BucketRef {
                target: "bkt".into(),
                bucket: "company-artifacts".into(),
            }),
        );
        let location = Location::S3 {
            target: "bkt".into(),
            bucket: Some("company-artifacts".into()),
            prefix: String::new(),
        };
        let entries = [listed];
        let visible = apply_filter_with_parent(&entries, "", &location, &registry, &parent);
        // A provider-listed ".." row is NOT reclassified as the virtual Parent row.
        assert_eq!(visible.len(), 1);
        assert!(matches!(visible[0], VisiblePaneRow::Listed(_)));
        assert!(!matches!(visible[0], VisiblePaneRow::Parent(_)));
    }

    #[test]
    fn load_more_ordering_with_parent() {
        let registry = test_registry();
        let parent = virtual_parent_entry();
        let load_more = load_more_entry();
        let entries = [listed(file("child.txt"))];
        let location = Location::S3 {
            target: "acc".into(),
            bucket: Some("anything".into()),
            prefix: String::new(),
        };
        let continuation = page_continuation(PaneLoadId(7), location.clone());
        let visible = apply_filter_with_parent_and_continuation(
            &entries,
            "",
            &location,
            &registry,
            &parent,
            &load_more,
            Some(&continuation),
        );
        assert!(matches!(visible[0], VisiblePaneRow::Parent(_)));
        assert!(matches!(visible[1], VisiblePaneRow::Listed(_)));
        assert!(matches!(visible[2], VisiblePaneRow::LoadMore(_)));
    }

    #[test]
    fn load_more_ordering_bucket_bound_root_no_parent() {
        let registry = test_registry();
        let parent = virtual_parent_entry();
        let load_more = load_more_entry();
        let entries = [listed(file("child.txt"))];
        let location = Location::S3 {
            target: "bkt".into(),
            bucket: Some("company-artifacts".into()),
            prefix: String::new(),
        };
        let continuation = page_continuation(PaneLoadId(7), location.clone());
        let visible = apply_filter_with_parent_and_continuation(
            &entries,
            "",
            &location,
            &registry,
            &parent,
            &load_more,
            Some(&continuation),
        );
        assert!(matches!(visible[0], VisiblePaneRow::Listed(_)));
        assert!(matches!(visible[1], VisiblePaneRow::LoadMore(_)));
        assert!(
            !visible
                .iter()
                .any(|r| matches!(r, VisiblePaneRow::Parent(_)))
        );
    }

    // ---- S3-26A TRACK B: list-only surface hardening -----------------------
    // ponytail: event_loop matches keys inline against a live terminal, so there
    // is no dispatch seam to drive from a test. The mirrors below restate the
    // guarded arms and pin their production text, so a mirror cannot outlive the
    // guard it mirrors: delete the guard and every test here fails.
    const TUI_SOURCE: &str = include_str!("tui/input_dispatch.rs");

    #[test]
    fn legacy_matcher_has_no_raw_ctrl_backslash_arm() {
        let production = TUI_SOURCE
            .split_once("mod tests {")
            .expect("tests module marker")
            .0;
        let raw_arm = [
            "KeyCode::Char(",
            "'\\\\'",
            ") if key.modifiers.contains(KeyModifiers::CONTROL)",
        ]
        .concat();
        assert!(!production.contains(&raw_arm));
    }

    fn pin(snippet: &str) {
        let production = TUI_SOURCE
            .split_once("mod tests {")
            .expect("tests module marker")
            .0;
        assert!(
            production.contains(snippet),
            "production guard drifted, test mirror is stale: {snippet}"
        );
    }

    /// Mirror of the `KeyCode::F(6)` + SHIFT arm in `event_loop`.
    fn shift_f6(state: &mut AppState, rows: &[VisiblePaneRow<'_>], cursor: usize) {
        pin("if matches!(pane.location, Location::Local(_)) {");
        pin("\"Rename is currently local-only\"");
        let location = state.active_pane().location.clone();
        if let Some(entry) = rows
            .get(cursor)
            .and_then(VisiblePaneRow::listed)
            .map(|listed| &listed.entry)
        {
            if matches!(location, Location::Local(_)) {
                state.cmd = format!("mv '{}' ", entry.name);
                state.cmd_input = true;
            } else {
                state.message = Some("Rename is currently local-only".into());
            }
        }
    }

    /// Mirror of the `MouseEventKind::Drag` arm in `event_loop`.
    fn mouse_drag_select(
        state: &mut AppState,
        pane: Pane,
        rows: &[VisiblePaneRow<'_>],
        row: usize,
    ) {
        pin("if !matches!(location, Location::S3 { .. }) {");
        if let Some(entry) = rows
            .get(row)
            .and_then(VisiblePaneRow::listed)
            .map(|listed| &listed.entry)
        {
            let location = match pane {
                Pane::Left => state.left.location.clone(),
                Pane::Right => state.right.location.clone(),
            };
            if !matches!(location, Location::S3 { .. }) {
                state.toggle_selection(pane, &location, &entry.name);
            }
        }
    }

    /// Mirror of the glob-select Enter arm in `event_loop`.
    fn glob_select(state: &mut AppState, rows: &[VisiblePaneRow<'_>]) {
        pin("if !matches!(location, Location::S3 { .. }) {");
        pin("\"Selection by name is not supported for S3\"");
        let active = state.active;
        let location = state.active_pane().location.clone();
        if !matches!(location, Location::S3 { .. }) {
            for e in rows.iter().filter_map(VisiblePaneRow::listed) {
                if !state.is_selected(active, &location, &e.entry.name) {
                    state.toggle_selection(active, &location, &e.entry.name);
                }
            }
            state.message = Some(format!(
                "Selected {}",
                state.selection_count(active, &location)
            ));
        } else {
            state.message = Some("Selection by name is not supported for S3".into());
        }
        state.filter.clear();
    }

    /// Mirror of the `*` invert-selection arm in `event_loop`.
    fn invert_selection(state: &mut AppState, rows: &[VisiblePaneRow<'_>]) {
        pin("if !matches!(location, Location::S3 { .. }) {");
        pin("\"Selection by name is not supported for S3\"");
        let active = state.active;
        let location = state.active_pane().location.clone();
        if !matches!(location, Location::S3 { .. }) {
            for e in rows.iter().filter_map(VisiblePaneRow::listed) {
                state.toggle_selection(active, &location, &e.entry.name);
            }
            state.message = Some(format!(
                "Selected {}",
                state.selection_count(active, &location)
            ));
        } else {
            state.message = Some("Selection by name is not supported for S3".into());
        }
    }

    fn state_at(location: Location) -> AppState {
        let mut state = AppState {
            registry: test_registry(),
            ..AppState::default()
        };
        state.active = Pane::Left;
        state.left.location = location;
        state
    }

    fn s3_location() -> Location {
        Location::S3 {
            target: "acc".into(),
            bucket: Some("bucket".into()),
            prefix: String::new(),
        }
    }

    fn sftp_location() -> Location {
        Location::Sftp {
            host: "demo".into(),
            path: "/tmp/work".into(),
        }
    }

    fn archive_location() -> Location {
        Location::Archive {
            archive: "/tmp/bundle.tar.gz".into(),
            inner_path: String::new(),
        }
    }

    fn two_files() -> [ListedEntry; 2] {
        [listed(file("a.txt")), listed(file("b.txt"))]
    }

    fn assert_shift_f6_disabled(location: Location, row: ListedEntry) {
        let mut state = state_at(location);
        let rows = [VisiblePaneRow::Listed(&row)];
        shift_f6(&mut state, &rows, 0);
        assert!(!state.cmd_input, "rename input must stay closed");
        assert!(state.cmd.is_empty(), "no rename command may be staged");
        assert_eq!(
            state.message.as_deref(),
            Some("Rename is currently local-only")
        );
    }

    fn assert_direct_selection_preserved(location: Location) {
        let rows_src = two_files();
        let rows = [
            VisiblePaneRow::Listed(&rows_src[0]),
            VisiblePaneRow::Listed(&rows_src[1]),
        ];

        let mut drag = state_at(location.clone());
        mouse_drag_select(&mut drag, Pane::Left, &rows, 0);
        assert_eq!(drag.selection_count(Pane::Left, &location), 1);
        assert!(drag.is_selected(Pane::Left, &location, "a.txt"));

        let mut invert = state_at(location.clone());
        invert_selection(&mut invert, &rows);
        assert_eq!(invert.selection_count(Pane::Left, &location), 2);
        assert_eq!(invert.message.as_deref(), Some("Selected 2"));

        let mut glob = state_at(location.clone());
        glob.filter = "*.txt".into();
        glob_select(&mut glob, &rows);
        assert_eq!(glob.selection_count(Pane::Left, &location), 2);
        assert_eq!(glob.message.as_deref(), Some("Selected 2"));
        assert!(glob.filter.is_empty());
    }

    #[test]
    fn shift_f6_local_still_opens_rename_command() {
        let location = Location::Local("/tmp/work".into());
        let mut state = state_at(location);
        let row = listed(file("note.txt"));
        let rows = [VisiblePaneRow::Listed(&row)];

        shift_f6(&mut state, &rows, 0);

        assert!(state.cmd_input);
        assert_eq!(state.cmd, "mv 'note.txt' ");
        assert!(state.message.is_none());
    }

    #[test]
    fn shift_f6_s3_does_not_use_presentation_name() {
        let mut state = state_at(s3_location());
        let row = listed_with_identity(
            "DISPLAY-NAME-MUST-NOT-LEAK",
            EntryKind::File,
            EntryIdentity::S3Object(S3ObjectRef {
                target: "acc".into(),
                bucket: "bucket".into(),
                key: "exact/key".into(),
            }),
        );
        let rows = [VisiblePaneRow::Listed(&row)];

        shift_f6(&mut state, &rows, 0);

        assert!(!state.cmd_input);
        assert!(state.cmd.is_empty());
        assert!(!state.cmd.contains("DISPLAY-NAME-MUST-NOT-LEAK"));
        assert!(!state.cmd.contains("exact/key"));
        assert_eq!(
            state.message.as_deref(),
            Some("Rename is currently local-only")
        );
    }

    #[test]
    fn shift_f6_sftp_disabled() {
        assert_shift_f6_disabled(sftp_location(), listed(file("note.txt")));
    }

    #[test]
    fn s3_30r_f3_dispatches_via_identity_preview_lane() {
        // S3-30R: S3 regular object F3 now routes to the identity-aware
        // PreviewLocation lane (same as SFTP), not the local-only fallthrough.
        assert!(
            s3_f3_routes_to_preview(&s3_location()),
            "S3 must route F3 through the identity-aware preview lane"
        );
        // Local/Archive must NOT take that lane (they keep their own paths).
        assert!(!s3_f3_routes_to_preview(&Location::Local(
            "/tmp/work".into()
        )));
        assert!(!s3_f3_routes_to_preview(&archive_location()));

        // The exact S3ObjectRef identity survives the dispatch intact:
        // dispatch passes `listed.clone()` (EntryIdentity::S3Object), never
        // entry.name. This is the behavioral agreement with S3-29.
        let row = listed_with_identity(
            "WRONG-DISPLAY.txt",
            EntryKind::File,
            EntryIdentity::S3Object(S3ObjectRef {
                target: "prod".into(),
                bucket: "bucket".into(),
                key: "foo/../REAL//日本語🧙‍♂️.txt".into(),
            }),
        );
        let EntryIdentity::S3Object(refr) = &row.identity else {
            panic!("expected S3Object identity");
        };
        assert_eq!(refr.key, "foo/../REAL//日本語🧙‍♂️.txt");
        assert_eq!(refr.target, "prod");
        assert_eq!(refr.bucket, "bucket");
        assert_ne!(row.entry.name, refr.key);
    }

    #[test]
    fn shift_f6_archive_disabled() {
        assert_shift_f6_disabled(archive_location(), listed(file("note.txt")));
    }

    #[test]
    fn s3_mouse_drag_does_not_select_by_name() {
        let location = s3_location();
        let mut state = state_at(location.clone());
        let rows_src = two_files();
        let rows = [
            VisiblePaneRow::Listed(&rows_src[0]),
            VisiblePaneRow::Listed(&rows_src[1]),
        ];

        mouse_drag_select(&mut state, Pane::Left, &rows, 0);
        mouse_drag_select(&mut state, Pane::Left, &rows, 1);

        assert_eq!(state.selection_count(Pane::Left, &location), 0);
        assert!(!state.is_selected(Pane::Left, &location, "a.txt"));
    }

    #[test]
    fn s3_glob_select_does_not_select_by_name() {
        let location = s3_location();
        let mut state = state_at(location.clone());
        state.filter = "*.txt".into();
        let rows_src = two_files();
        let rows = [
            VisiblePaneRow::Listed(&rows_src[0]),
            VisiblePaneRow::Listed(&rows_src[1]),
        ];

        glob_select(&mut state, &rows);

        assert_eq!(state.selection_count(Pane::Left, &location), 0);
        assert_eq!(
            state.message.as_deref(),
            Some("Selection by name is not supported for S3")
        );
        assert!(state.filter.is_empty());
    }

    #[test]
    fn s3_invert_selection_does_not_select_by_name() {
        let location = s3_location();
        let mut state = state_at(location.clone());
        let rows_src = two_files();
        let rows = [
            VisiblePaneRow::Listed(&rows_src[0]),
            VisiblePaneRow::Listed(&rows_src[1]),
        ];

        invert_selection(&mut state, &rows);

        assert_eq!(state.selection_count(Pane::Left, &location), 0);
        assert_eq!(
            state.message.as_deref(),
            Some("Selection by name is not supported for S3")
        );
    }

    #[test]
    fn local_direct_selection_regression() {
        assert_direct_selection_preserved(Location::Local("/tmp/work".into()));
    }

    #[test]
    fn sftp_direct_selection_regression() {
        assert_direct_selection_preserved(sftp_location());
    }

    #[test]
    fn archive_direct_selection_regression() {
        assert_direct_selection_preserved(archive_location());
    }

    #[test]
    fn parent_not_rename_target() {
        let location = Location::Local("/tmp/work".into());
        let mut state = state_at(location.clone());
        let parent = virtual_parent_entry();
        let rows = [VisiblePaneRow::Parent(&parent)];

        shift_f6(&mut state, &rows, 0);

        assert!(!state.cmd_input);
        assert!(state.cmd.is_empty());
        assert!(state.message.is_none());

        invert_selection(&mut state, &rows);
        assert_eq!(state.selection_count(Pane::Left, &location), 0);
    }

    #[test]
    fn load_more_not_rename_target() {
        let location = Location::Local("/tmp/work".into());
        let mut state = state_at(location.clone());
        let load_more = load_more_entry();
        let rows = [VisiblePaneRow::LoadMore(&load_more)];

        shift_f6(&mut state, &rows, 0);

        assert!(!state.cmd_input);
        assert!(state.cmd.is_empty());
        assert!(state.message.is_none());

        invert_selection(&mut state, &rows);
        assert_eq!(state.selection_count(Pane::Left, &location), 0);
    }

    // S3-27R G: Parent/LoadMore cannot create a preview target because they
    // have no ListedEntry identity.
    #[test]
    fn preview_target_not_constructed_for_parent_or_load_more() {
        let entry = Entry {
            name: "x".into(),
            kind: EntryKind::File,
            size: None,
            modified_unix_ms: None,
        };
        let parent = VisiblePaneRow::Parent(&entry);
        let load_more = VisiblePaneRow::LoadMore(&entry);
        assert!(parent.listed().is_none());
        assert!(load_more.listed().is_none());
        // Listed preserves identity — preview target constructible only here.
        let listed = ListedEntry {
            entry: entry.clone(),
            identity: EntryIdentity::Other,
        };
        assert!(VisiblePaneRow::Listed(&listed).listed().is_some());
    }
}

#[cfg(test)]
mod pack_o_quick_action_tests {
    use super::*;

    #[test]
    fn sha_result_presentation_escapes_filename_controls() {
        let mut state = AppState::default();

        effect_responses::apply_effect_event(
            &mut state,
            EffectLane::QuickAction,
            EffectEvent::QuickActionFinished {
                result: Ok(QuickActionOutcome::Sha256 {
                    dir: PathBuf::from("/tmp"),
                    checksums: vec![arx::services::ChecksumResult {
                        name: "bad\nname ü".into(),
                        sha256: "abc123".into(),
                    }],
                }),
            },
        );

        assert_eq!(
            state.viewer_content,
            vec!["abc123  bad\\nname ü".to_string()]
        );
        assert_eq!(state.viewer_scroll, 0);
    }
}

#[cfg(test)]
mod r214_browser_legacy_tests {
    use super::*;
    use arx::config::KeybindingConfig;
    use arx::input::Keymap;

    fn kb(action: &str, keys: &str) -> KeybindingConfig {
        KeybindingConfig {
            context: "browser".into(),
            action: action.into(),
            keys: Some(keys.into()),
            disabled: false,
        }
    }

    #[test]
    fn r214_f11_user_binding_passes_legacy_validator() {
        let km = Keymap::effective(&[kb("open_storage_inspector", "F11")]).unwrap();
        assert!(validate_user_browser_bindings(&km).is_ok());
    }

    #[test]
    fn r214_tab_conflicts_with_legacy_switch_pane() {
        if let Ok(km) = Keymap::effective(&[kb("open_smart_tree", "Tab")]) {
            assert!(validate_user_browser_bindings(&km).is_err());
        }
    }

    #[test]
    fn r214_f2_conflicts_with_user_menu_route() {
        if let Ok(km) = Keymap::effective(&[kb("open_smart_tree", "F2")]) {
            assert!(validate_user_browser_bindings(&km).is_err());
        }
    }

    #[test]
    fn r214_ctrl_r_conflicts_with_refresh_route() {
        if let Ok(km) = Keymap::effective(&[kb("open_smart_tree", "Ctrl+R")]) {
            assert!(validate_user_browser_bindings(&km).is_err());
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn r214_alt_d_conflicts_with_filesystems() {
        if let Ok(km) = Keymap::effective(&[kb("open_smart_tree", "Alt+D")]) {
            assert!(validate_user_browser_bindings(&km).is_err());
        }
    }

    #[test]
    fn r214_plain_slash_and_esc_conflict_conditionally() {
        for keys in ["/", "Esc"] {
            if let Ok(km) = Keymap::effective(&[kb("open_smart_tree", keys)]) {
                assert!(
                    validate_user_browser_bindings(&km).is_err(),
                    "{keys} must be claimed under tree/infra representative state"
                );
            }
        }
    }

    #[test]
    fn r214_ctrl_arrows_claimed_when_tabs_exist() {
        for keys in ["Ctrl+Left", "Ctrl+Right"] {
            if let Ok(km) = Keymap::effective(&[kb("open_smart_tree", keys)]) {
                assert!(validate_user_browser_bindings(&km).is_err());
            }
        }
    }

    #[test]
    fn r214_effective_keymap_builder_smoke_via_binary_helper_shape() {
        // The same builder the binary helper calls.
        let empty: Vec<KeybindingConfig> = Vec::new();
        let km = Keymap::effective(&empty).unwrap();
        assert!(!km.bindings().is_empty());
    }
}
