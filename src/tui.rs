use arx::app::{Action, AppState, Pane, PaneState, SortMode};
use arx::vfs::{Entry, EntryKind, Location, archive::ArchiveFs, local::LocalFs, sftp::SftpFs};
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
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
use std::path::{Path, PathBuf};

pub fn run(config: arx::config::ArxConfig) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(stdout))?;

    let result = event_loop(&mut terminal, config);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn event_loop(terminal: &mut DefaultTerminal, config: arx::config::ArxConfig) -> io::Result<()> {
    let mut state = AppState {
        show_hidden: config.ui.show_hidden,
        hosts: arx::remote::hosts_config::load_hosts(),
        menu: AppState::load_menu(),
        ..AppState::default()
    };
    let mut left_entries = load_entries(&state.left.location, state.show_hidden, state.sort_mode);
    let mut right_entries = load_entries(&state.right.location, state.show_hidden, state.sort_mode);
    let mut left_list = ListState::default();
    let mut right_list = ListState::default();

    // Background job notification channel
    let (job_tx, job_rx) = std::sync::mpsc::channel::<arx::jobs::JobEvent>();

    loop {
        if state.should_quit {
            break;
        }
        let mut refresh_entries = false;
        // Drain completed/failed job events
        while let Ok(event) = job_rx.try_recv() {
            match event {
                arx::jobs::JobEvent::Running { ref id } => {
                    if let Some(job) = state.jobs.iter_mut().find(|j| j.id == *id) {
                        job.status = arx::jobs::JobStatus::Running;
                    }
                }
                arx::jobs::JobEvent::Done {
                    ref id,
                    ref message,
                } => {
                    if let Some(job) = state.jobs.iter_mut().find(|j| j.id == *id) {
                        job.status = arx::jobs::JobStatus::Done;
                        job.progress = 100;
                    }
                    state.message = Some(message.clone());
                    refresh_entries = true;
                }
                arx::jobs::JobEvent::Failed { ref id, ref error } => {
                    if let Some(job) = state.jobs.iter_mut().find(|j| j.id == *id) {
                        job.status = arx::jobs::JobStatus::Failed;
                    }
                    state.message = Some(error.clone());
                    refresh_entries = true;
                }
            }
        }

        if refresh_entries {
            left_entries = load_entries(&state.left.location, state.show_hidden, state.sort_mode);
            right_entries = load_entries(&state.right.location, state.show_hidden, state.sort_mode);
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
        right_list.select(Some(state.right.cursor));

        let msg = state.message.clone();
        terminal.draw(|frame| {
            render(
                frame,
                &state,
                &left_filtered,
                &right_filtered,
                &mut left_list.clone(),
                &mut right_list.clone(),
                msg.as_deref(),
            )
        })?;
        state.message = None; // one-shot clear after render

        #[allow(clippy::collapsible_if)]
        if let Event::Key(key) = event::read()? {
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
                            state.message = Some(format!("Selected {}", state.selected.len()));
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
                            if resolved.is_dir() {
                                let pane = state.active_pane_mut();
                                pane.location = Location::Local(resolved);
                                pane.cursor = 0;
                                state.selected.clear();
                                left_entries = load_entries(
                                    &state.left.location,
                                    state.show_hidden,
                                    state.sort_mode,
                                );
                                right_entries = load_entries(
                                    &state.right.location,
                                    state.show_hidden,
                                    state.sort_mode,
                                );
                            } else {
                                state.message = Some(format!("Not a directory: {}", state.filter));
                            }
                            state.filter.clear();
                        }
                        if state.cmd_input {
                            let command = std::mem::take(&mut state.cmd);
                            state.cmd_input = false;
                            if command.is_empty() {
                                state.message = Some(": command cancelled".into());
                            } else {
                                // Run shell command, capture output
                                let output = std::process::Command::new("sh")
                                    .arg("-c")
                                    .arg(&command)
                                    .output();
                                state.message = match output {
                                    Ok(o) => {
                                        let stdout =
                                            String::from_utf8_lossy(&o.stdout).trim().to_string();
                                        let limit = 80;
                                        if stdout.is_empty() && o.status.success() {
                                            Some(format!(": {command} — ok"))
                                        } else if stdout.len() > limit {
                                            Some(format!(": {} — {}...", command, &stdout[..limit]))
                                        } else {
                                            Some(format!(": {} — {}", command, stdout))
                                        }
                                    }
                                    Err(e) => Some(format!(": {command} failed: {e}")),
                                };
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
                            let pane = state.active_pane_mut();
                            pane.location = loc;
                            pane.cursor = 0;
                            state.selected.clear();
                            state.show_bookmarks = false;
                            left_entries = load_entries(
                                &state.left.location,
                                state.show_hidden,
                                state.sort_mode,
                            );
                            right_entries = load_entries(
                                &state.right.location,
                                state.show_hidden,
                                state.sort_mode,
                            );
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
                            let pane = state.active_pane_mut();
                            let default_path = host.default_path.as_deref().unwrap_or("/");
                            pane.location = Location::Sftp {
                                host: host.id.clone(),
                                path: default_path.into(),
                            };
                            pane.cursor = 0;
                            state.selected.clear();
                            state.show_hosts = false;
                            left_entries = load_entries(
                                &state.left.location,
                                state.show_hidden,
                                state.sort_mode,
                            );
                            right_entries = load_entries(
                                &state.right.location,
                                state.show_hidden,
                                state.sort_mode,
                            );
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
                            state.show_menu = false;
                            // Run menu command, show output
                            let output = std::process::Command::new("sh")
                                .arg("-c")
                                .arg(&cmd)
                                .output();
                            state.message = match output {
                                Ok(o) => {
                                    let out = String::from_utf8_lossy(&o.stdout).trim().to_string();
                                    if out.is_empty() && o.status.success() {
                                        Some(format!("menu: {} — ok", entry.label))
                                    } else if out.len() > 80 {
                                        Some(format!("menu: {} — {}...", entry.label, &out[..80]))
                                    } else {
                                        Some(format!("menu: {} — {out}", entry.label))
                                    }
                                }
                                Err(e) => Some(format!("menu: {} failed: {e}", entry.label)),
                            };
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
            let cursor = if state.active == Pane::Left {
                state.left.cursor
            } else {
                state.right.cursor
            };
            let pane = state.active_pane_mut();

            match key.code {
                KeyCode::Char('q') => state.apply(Action::Quit),
                KeyCode::Tab => state.apply(Action::SwitchPane),
                KeyCode::Up | KeyCode::Char('k') => {
                    if cursor > 0 {
                        pane.cursor -= 1;
                    }
                }
                KeyCode::Down | KeyCode::Char('j')
                    if !key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    if cursor + 1 < entries.len() {
                        pane.cursor += 1;
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
                KeyCode::Char('/') => {
                    state.filter.clear();
                    state.filtering = true;
                }
                KeyCode::Char('?') => {
                    state.show_help = !state.show_help;
                }
                KeyCode::Enter => {
                    if let Some(entry) = entries.get(cursor) {
                        if entry.kind == EntryKind::Directory {
                            let new_location = match &pane.location {
                                Location::Local(p) => Location::Local(p.join(&entry.name)),
                                Location::Sftp { host, path } => {
                                    let new_path = if path.ends_with('/') {
                                        format!("{path}{}", entry.name)
                                    } else {
                                        format!("{path}/{}", entry.name)
                                    };
                                    Location::Sftp {
                                        host: host.clone(),
                                        path: new_path,
                                    }
                                }
                                Location::Archive {
                                    archive,
                                    inner_path,
                                } => {
                                    let new_path = if inner_path.is_empty() {
                                        entry.name.clone()
                                    } else if inner_path.ends_with('/') {
                                        format!("{inner_path}{}", entry.name)
                                    } else {
                                        format!("{inner_path}/{}", entry.name)
                                    };
                                    Location::Archive {
                                        archive: archive.clone(),
                                        inner_path: new_path,
                                    }
                                }
                            };
                            pane.location = new_location;
                            pane.cursor = 0;
                            state.selected.clear();
                            if state.active == Pane::Left {
                                left_entries = load_entries(
                                    &state.left.location,
                                    state.show_hidden,
                                    state.sort_mode,
                                );
                            } else {
                                right_entries = load_entries(
                                    &state.right.location,
                                    state.show_hidden,
                                    state.sort_mode,
                                );
                            }
                        } else if is_archive(&entry.name) {
                            // Open archive file
                            if let Location::Local(dir) = &pane.location {
                                let archive_path = dir.join(&entry.name);
                                pane.location = Location::Archive {
                                    archive: archive_path,
                                    inner_path: String::new(),
                                };
                                pane.cursor = 0;
                                state.selected.clear();
                                if state.active == Pane::Left {
                                    left_entries = load_entries(
                                        &state.left.location,
                                        state.show_hidden,
                                        state.sort_mode,
                                    );
                                } else {
                                    right_entries = load_entries(
                                        &state.right.location,
                                        state.show_hidden,
                                        state.sort_mode,
                                    );
                                }
                            }
                        }
                    }
                }
                KeyCode::Backspace => {
                    let go_back = match &pane.location {
                        Location::Local(p) => {
                            let parent = LocalFs::parent(p);
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
                        pane.location = new_loc;
                        pane.cursor = 0;
                        state.selected.clear();
                        if state.active == Pane::Left {
                            left_entries = load_entries(
                                &state.left.location,
                                state.show_hidden,
                                state.sort_mode,
                            );
                        } else {
                            right_entries = load_entries(
                                &state.right.location,
                                state.show_hidden,
                                state.sort_mode,
                            );
                        }
                    }
                }
                KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    left_entries =
                        load_entries(&state.left.location, state.show_hidden, state.sort_mode);
                    right_entries =
                        load_entries(&state.right.location, state.show_hidden, state.sort_mode);
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
                    left_entries =
                        load_entries(&state.left.location, state.show_hidden, state.sort_mode);
                    right_entries =
                        load_entries(&state.right.location, state.show_hidden, state.sort_mode);
                }
                // F5: copy selected (or cursor) from active pane to other pane — background
                KeyCode::F(5) => {
                    let names = selection_or_cursor(&state, entries, cursor);
                    if names.is_empty() {
                        continue;
                    }
                    match op_paths(&state) {
                        Some((src, dst)) => {
                            let id = format!("copy-{}", state.jobs.len());
                            let desc = format!("Copy {} → {}", names.join(", "), dst.display());
                            state.jobs.push(arx::jobs::Job {
                                id: id.clone(),
                                description: desc,
                                source: Location::Local(src.clone()),
                                destination: Location::Local(dst.clone()),
                                status: arx::jobs::JobStatus::Pending,
                                progress: 0,
                            });
                            let tx = job_tx.clone();
                            let names2 = names.clone();
                            std::thread::spawn(move || {
                                tx.send(arx::jobs::JobEvent::Running { id: id.clone() })
                                    .ok();
                                let result = LocalFs::copy_files(&src, &dst, &names2);
                                match result {
                                    Ok(n) => {
                                        tx.send(arx::jobs::JobEvent::Done {
                                            id,
                                            message: format!("Copied {n} item(s)"),
                                        })
                                        .ok();
                                    }
                                    Err(e) => {
                                        tx.send(arx::jobs::JobEvent::Failed {
                                            id,
                                            error: format!("Copy error: {e}"),
                                        })
                                        .ok();
                                    }
                                }
                            });
                            state.message = Some(format!("Copy queued (job {})", state.jobs.len()));
                        }
                        None => {
                            state.message = Some("Both panes must be local for copy".into());
                        }
                    }
                    state.selected.clear();
                    left_entries =
                        load_entries(&state.left.location, state.show_hidden, state.sort_mode);
                    right_entries =
                        load_entries(&state.right.location, state.show_hidden, state.sort_mode);
                }
                // F6: move selected (or cursor) from active pane to other pane — background
                KeyCode::F(6) => {
                    let names = selection_or_cursor(&state, entries, cursor);
                    if names.is_empty() {
                        continue;
                    }
                    match op_paths(&state) {
                        Some((src, dst)) => {
                            let id = format!("move-{}", state.jobs.len());
                            let desc = format!("Move {} → {}", names.join(", "), dst.display());
                            state.jobs.push(arx::jobs::Job {
                                id: id.clone(),
                                description: desc,
                                source: Location::Local(src.clone()),
                                destination: Location::Local(dst.clone()),
                                status: arx::jobs::JobStatus::Pending,
                                progress: 0,
                            });
                            let tx = job_tx.clone();
                            let names2 = names.clone();
                            std::thread::spawn(move || {
                                tx.send(arx::jobs::JobEvent::Running { id: id.clone() })
                                    .ok();
                                let result = LocalFs::move_files(&src, &dst, &names2);
                                match result {
                                    Ok(n) => {
                                        tx.send(arx::jobs::JobEvent::Done {
                                            id,
                                            message: format!("Moved {n} item(s)"),
                                        })
                                        .ok();
                                    }
                                    Err(e) => {
                                        tx.send(arx::jobs::JobEvent::Failed {
                                            id,
                                            error: format!("Move error: {e}"),
                                        })
                                        .ok();
                                    }
                                }
                            });
                            state.message = Some(format!("Move queued (job {})", state.jobs.len()));
                        }
                        None => {
                            state.message = Some("Both panes must be local for move".into());
                        }
                    }
                    state.selected.clear();
                    left_entries =
                        load_entries(&state.left.location, state.show_hidden, state.sort_mode);
                    right_entries =
                        load_entries(&state.right.location, state.show_hidden, state.sort_mode);
                }
                // F8: delete selected (or cursor) from active pane
                KeyCode::F(8) => {
                    let names = selection_or_cursor(&state, entries, cursor);
                    let active_path = pane_location_path(&state);
                    if let Some(dir) = active_path {
                        match LocalFs::delete_files(dir, &names) {
                            Ok(n) => {
                                state.message = Some(format!("Deleted {n} item(s)"));
                            }
                            Err(e) => {
                                state.message = Some(format!("Delete error: {e}"));
                            }
                        }
                        state.selected.clear();
                        left_entries =
                            load_entries(&state.left.location, state.show_hidden, state.sort_mode);
                        right_entries =
                            load_entries(&state.right.location, state.show_hidden, state.sort_mode);
                    }
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
                        left_entries =
                            load_entries(&state.left.location, state.show_hidden, state.sort_mode);
                        right_entries =
                            load_entries(&state.right.location, state.show_hidden, state.sort_mode);
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
                            state.viewer_content = LocalFs::read_head(&path, 500)
                                .unwrap_or_else(|e| vec![format!("Error reading file: {e}")]);
                            state.viewer_scroll = 0;
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
                        let editor = std::env::var("EDITOR")
                            .or_else(|_| std::env::var("VISUAL"))
                            .unwrap_or_else(|_| "vi".into());
                        // Leave raw mode, spawn editor, restore
                        disable_raw_mode()?;
                        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
                        let _ = std::process::Command::new(&editor).arg(&path).status();
                        execute!(terminal.backend_mut(), EnterAlternateScreen)?;
                        enable_raw_mode()?;
                        // Refresh after edit
                        left_entries =
                            load_entries(&state.left.location, state.show_hidden, state.sort_mode);
                        right_entries =
                            load_entries(&state.right.location, state.show_hidden, state.sort_mode);
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
                        state.message =
                            Some("No hosts configured — add ~/.config/arx/hosts.toml".into());
                    }
                }
                // Ctrl+J: jobs panel
                KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    state.show_jobs = !state.show_jobs;
                    state.job_cursor = 0;
                }
                // Ctrl+X D: toggle directory compare
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
                // Ctrl+T: new tab in active pane
                KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let show_hidden = state.show_hidden;
                    let sort_mode = state.sort_mode;
                    state.active_pane_mut().new_tab();
                    let tabs = state.active_pane().tabs.len() + 1;
                    left_entries = load_entries(&state.left.location, show_hidden, sort_mode);
                    right_entries = load_entries(&state.right.location, show_hidden, sort_mode);
                    state.message = Some(format!("Tab {tabs}/{tabs}"));
                }
                // Ctrl+W: close tab in active pane
                KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let show_hidden = state.show_hidden;
                    let sort_mode = state.sort_mode;
                    state.active_pane_mut().close_tab();
                    let tabs = state.active_pane().tabs.len() + 1;
                    left_entries = load_entries(&state.left.location, show_hidden, sort_mode);
                    right_entries = load_entries(&state.right.location, show_hidden, sort_mode);
                    state.message = Some(format!("Tab {}/{}", tabs.min(1), tabs));
                }
                // Ctrl+Left: previous tab
                KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let show_hidden = state.show_hidden;
                    let sort_mode = state.sort_mode;
                    let tabs_len = state.active_pane().tabs.len();
                    if tabs_len > 0 {
                        state.active_pane_mut().switch_tab(tabs_len - 1);
                        left_entries = load_entries(&state.left.location, show_hidden, sort_mode);
                        right_entries = load_entries(&state.right.location, show_hidden, sort_mode);
                        state.message = Some("Tab ←".into());
                    }
                }
                // Ctrl+Right: next tab
                KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let show_hidden = state.show_hidden;
                    let sort_mode = state.sort_mode;
                    if state.active_pane().tabs.len() >= 2 {
                        state.active_pane_mut().switch_tab(0);
                        left_entries = load_entries(&state.left.location, show_hidden, sort_mode);
                        right_entries = load_entries(&state.right.location, show_hidden, sort_mode);
                        state.message = Some("Tab →".into());
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn load_entries(location: &Location, show_hidden: bool, sort_mode: SortMode) -> Vec<Entry> {
    let mut entries = match location {
        Location::Local(path) => LocalFs::list(path).unwrap_or_default(),
        Location::Sftp { host, path } => {
            let synthetic = arx::remote::Host {
                id: host.clone(),
                name: host.clone(),
                ssh_alias: host.clone(),
                hostname: host.clone(),
                port: 22,
                user: std::env::var("USER").unwrap_or_else(|_| "root".into()),
                group_ids: Default::default(),
                tags: Default::default(),
                favorite: false,
                default_path: None,
                transfer_preference: arx::remote::TransferPreference::Auto,
                notes: None,
            };
            SftpFs::list(&synthetic, path).unwrap_or_default()
        }
        Location::Archive {
            archive,
            inner_path,
        } => ArchiveFs::list(archive, inner_path).unwrap_or_default(),
    };
    if !show_hidden {
        entries.retain(|e| !e.name.starts_with('.'));
    }
    sort_entries(&mut entries, sort_mode);
    entries
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
        let lower = filter.to_lowercase();
        entries
            .iter()
            .filter(|e| e.name.to_lowercase().contains(&lower))
            .collect()
    }
}

fn pane_location_path(state: &AppState) -> Option<&Path> {
    match &state.active_pane().location {
        Location::Local(p) => Some(p),
        _ => None,
    }
}

fn other_pane_location_path(state: &AppState) -> Option<&Path> {
    let other = match state.active {
        Pane::Left => &state.right,
        Pane::Right => &state.left,
    };
    match &other.location {
        Location::Local(p) => Some(p),
        _ => None,
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

/// Returns (src_path, dst_path) for active→other pane file operations.
fn op_paths(state: &AppState) -> Option<(PathBuf, PathBuf)> {
    let src = pane_location_path(state)?.to_path_buf();
    let dst = other_pane_location_path(state)?.to_path_buf();
    Some((src, dst))
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

fn render(
    frame: &mut ratatui::Frame,
    state: &AppState,
    left_entries: &[&Entry],
    right_entries: &[&Entry],
    left_list: &mut ListState,
    right_list: &mut ListState,
    message: Option<&str>,
) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)])
        .split(chunks[0]);

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

    render_pane(
        frame,
        panes[0],
        &state.left,
        left_entries,
        left_list,
        state.active == Pane::Left,
        &state.selected,
        &left_only,
    );
    render_pane(
        frame,
        panes[1],
        &state.right,
        right_entries,
        right_list,
        state.active == Pane::Right,
        &state.selected,
        &right_only,
    );

    // Help overlay
    if state.show_help {
        render_help(frame, area);
    }

    // Viewer overlay
    if !state.viewer_content.is_empty() {
        render_viewer(frame, area, state);
    }

    // Bookmarks overlay
    if state.show_bookmarks {
        render_bookmarks(frame, area, state);
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
    let msg_hint = message.map(|m| format!(" | {m}")).unwrap_or_default();

    let status = Paragraph::new(Line::from(format!(
        "ARX v0.1.0 | {}{hidden}{tab_info} | sel: {} |{hint}{msg_hint} | ?: help",
        loc_str,
        state.selected.len(),
    )))
    .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(status, chunks[1]);
}

fn render_help(frame: &mut ratatui::Frame, area: Rect) {
    let popup_area = centered_rect(60, 60, area);
    frame.render_widget(Clear, popup_area);

    let text = vec![
        Line::from(Span::styled("ARX Help", Style::default().fg(Color::Yellow))),
        Line::from(""),
        Line::from("Navigation"),
        Line::from("  j/↓ k/↑      Move cursor"),
        Line::from("  Enter         Enter directory"),
        Line::from("  Backspace     Go to parent directory"),
        Line::from("  Tab           Switch active pane"),
        Line::from("  Ctrl+G        Go to path"),
        Line::from(""),
        Line::from("Selection & Filter"),
        Line::from("  Space         Toggle selection"),
        Line::from("  *             Invert selection"),
        Line::from("  +             Select by glob pattern"),
        Line::from("  /             Quick filter by name"),
        Line::from("  Ctrl+H        Toggle hidden (dot) files"),
        Line::from(""),
        Line::from("File Operations"),
        Line::from("  F5            Copy to other pane"),
        Line::from("  F6            Move to other pane"),
        Line::from("  F8            Delete"),
        Line::from(""),
        Line::from("Other"),
        Line::from("  Ctrl+R        Refresh"),
        Line::from("  q             Quit"),
        Line::from("  ?             This help"),
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

    let title = format!(" View ({} lines) ", state.viewer_content.len());
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
                ListItem::new(Line::from(format!("{prefix}{j}")))
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
) {
    let border_style = if active {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

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
                Style::default()
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
