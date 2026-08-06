use arx::app::{Action, AppState, Pane, PaneState, PanelMode, SortMode};
use arx::vfs::{Entry, EntryKind, Location, VfsOps};
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
use tokio::sync::mpsc;

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
    let mut left_entries = load_entries(&state.left.location, state.show_hidden, state.sort_mode);
    let mut right_entries = load_entries(&state.right.location, state.show_hidden, state.sort_mode);
    let mut left_list = ListState::default();
    let mut right_list = ListState::default();

    // Background job notification channel
    let (job_tx, mut job_rx) = mpsc::unbounded_channel::<arx::jobs::JobEvent>();

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
                &mut state,
                &left_filtered,
                &right_filtered,
                &mut left_list.clone(),
                &mut right_list.clone(),
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
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(50));
        let next_input = tokio::select! {
            Some(ev) = job_rx.recv() => {
                // Background job completed — update state and refresh
                handle_job_event(ev, &mut state, &mut left_entries, &mut right_entries);
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
                    use crossterm::event::MouseEventKind;
                    match mouse.kind {
                        MouseEventKind::ScrollDown if !state.viewer_content.is_empty() => {
                            let max = state.viewer_content.len().saturating_sub(1);
                            if state.viewer_scroll < max {
                                state.viewer_scroll += 1;
                            }
                        }
                        MouseEventKind::ScrollUp if !state.viewer_content.is_empty() => {
                            state.viewer_scroll = state.viewer_scroll.saturating_sub(1);
                        }
                        _ => {}
                    }
                    if let MouseEventKind::Down(_) = mouse.kind {
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
                }
                Event::Key(key) => {
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
                                        state.message =
                                            Some(format!("Not a directory: {}", state.filter));
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
                                                let stdout = String::from_utf8_lossy(&o.stdout)
                                                    .trim()
                                                    .to_string();
                                                let limit = 80;
                                                if stdout.is_empty() && o.status.success() {
                                                    Some(format!(": {command} — ok"))
                                                } else if stdout.len() > limit {
                                                    Some(format!(
                                                        ": {} — {}...",
                                                        command,
                                                        &stdout[..limit]
                                                    ))
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
                                    if state.show_command_center {
                                        state.command_matches =
                                            build_cc_matches(&state.filter, &state);
                                    }
                                    if state.show_command_center {
                                        state.command_matches =
                                            build_cc_matches(&state.filter, &state);
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
                                            let out = String::from_utf8_lossy(&o.stdout)
                                                .trim()
                                                .to_string();
                                            if out.is_empty() && o.status.success() {
                                                Some(format!("menu: {} — ok", entry.label))
                                            } else if out.len() > 80 {
                                                Some(format!(
                                                    "menu: {} — {}...",
                                                    entry.label,
                                                    &out[..80]
                                                ))
                                            } else {
                                                Some(format!("menu: {} — {out}", entry.label))
                                            }
                                        }
                                        Err(e) => {
                                            Some(format!("menu: {} failed: {e}", entry.label))
                                        }
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
                    let cmd_prefix = state.cmd_prefix;

                    if state.show_command_center && key.code == KeyCode::Enter {
                        let idx = state.overlay_list_state.selected().unwrap_or(0);
                        let idx = idx.min(state.command_matches.len().saturating_sub(1));
                        if let Some((_, target)) = state.command_matches.get(idx) {
                            let target = target.clone();
                            navigate_to(&mut state, &target);
                            state.show_command_center = false;
                            state.filtering = false;
                            state.filter.clear();
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
                            continue;
                        }
                    }

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
                        // Ctrl+Space: hash for files, du/df for dirs
                        KeyCode::Char(' ') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            if let Some(entry) = entries.get(cursor) {
                                match entry.kind {
                                    EntryKind::Directory => {
                                        if let Location::Local(dir) = &pane.location {
                                            let p = dir.join(&entry.name);
                                            let du = std::process::Command::new("du")
                                                .args(["-sh", &p.to_string_lossy()])
                                                .output()
                                                .map(|o| {
                                                    String::from_utf8_lossy(&o.stdout)
                                                        .trim()
                                                        .to_string()
                                                })
                                                .unwrap_or_default();
                                            let df = std::process::Command::new("df")
                                                .args(["-h", &p.to_string_lossy()])
                                                .output()
                                                .map(|o| {
                                                    let s = String::from_utf8_lossy(&o.stdout);
                                                    s.lines().last().unwrap_or_default().to_string()
                                                })
                                                .unwrap_or_default();
                                            state.viewer_content = vec![
                                                format!("Directory: {}", p.display()),
                                                format!("Size:     {du}"),
                                                format!("Free:     {df}"),
                                            ];
                                            state.viewer_scroll = 0;
                                        }
                                    }
                                    _ => {
                                        // File: show sha256 hash
                                        if let Location::Local(dir) = &pane.location {
                                            let p = dir.join(&entry.name);
                                            let hash = std::process::Command::new("sha256sum")
                                                .arg(&p)
                                                .output()
                                                .map(|o| {
                                                    let s = String::from_utf8_lossy(&o.stdout);
                                                    s.split_whitespace()
                                                        .next()
                                                        .unwrap_or("?")
                                                        .to_string()
                                                })
                                                .unwrap_or_else(|_| "?".into());
                                            let size =
                                                entry.size.map(format_size).unwrap_or_default();
                                            state.viewer_content = vec![
                                                format!("File: {}", p.display()),
                                                format!("Size: {size}"),
                                                format!("SHA256: {hash}"),
                                            ];
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
                                let dir_c = dir.clone();
                                let _dir_loc = Location::Local(dir_c.clone());
                                let entries = _dir_loc.list().unwrap_or_default();
                                let mut lines = vec!["Directory sizes:".into()];
                                for e in &entries {
                                    if e.kind == EntryKind::Directory {
                                        let p = dir_c.join(&e.name);
                                        let size = std::process::Command::new("du")
                                            .args(["-sh", &p.to_string_lossy()])
                                            .output()
                                            .map(|o| {
                                                String::from_utf8_lossy(&o.stdout)
                                                    .trim()
                                                    .to_string()
                                            })
                                            .unwrap_or_else(|_| "?".into());
                                        lines.push(format!("  {}  {}", size, e.name));
                                    }
                                }
                                state.viewer_content = lines;
                                state.viewer_scroll = 0;
                            }
                        }
                        KeyCode::Enter => {
                            if let Some(entry) = entries.get(cursor) {
                                if entry.kind == EntryKind::Directory {
                                    let old_location = pane.location.clone();
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
                                    pane.dir_history.push(old_location);
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
                                } else if state.show_diff {
                                    // Content diff: diff this file against other pane's same-named file
                                    if let (Location::Local(left_dir), Location::Local(right_dir)) =
                                        (&state.left.location, &state.right.location)
                                    {
                                        let left_path = left_dir.join(&entry.name);
                                        let right_path = right_dir.join(&entry.name);
                                        if left_path.exists() && right_path.exists() {
                                            let output = std::process::Command::new("diff")
                                                .args(["--color=never", "-u"])
                                                .arg(&left_path)
                                                .arg(&right_path)
                                                .output()
                                                .map(|o| {
                                                    let s = String::from_utf8_lossy(&o.stdout)
                                                        .into_owned();
                                                    if s.is_empty() {
                                                        "Files are identical".into()
                                                    } else {
                                                        s
                                                    }
                                                })
                                                .unwrap_or_else(|e| format!("diff error: {e}"));
                                            state.viewer_content =
                                                output.lines().map(|l| l.to_string()).collect();
                                            state.viewer_scroll = 0;
                                        }
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
                        // Ctrl+U: swap panes
                        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            std::mem::swap(&mut state.left, &mut state.right);
                            state.message = Some("Swapped".into());
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
                                    let stat_out = std::process::Command::new("stat")
                                        .args(["-c", "%a %A %U:%G %s", &p.to_string_lossy()])
                                        .output()
                                        .map(|o| {
                                            String::from_utf8_lossy(&o.stdout).trim().to_string()
                                        })
                                        .unwrap_or_default();
                                    let parts: Vec<&str> = stat_out.splitn(4, ' ').collect();
                                    let (octal, symbolic, owner, size_str) = if parts.len() >= 4 {
                                        (parts[0], parts[1], parts[2], parts[3])
                                    } else {
                                        ("?", "?", "?", "?")
                                    };
                                    state.viewer_content = vec![
                                        format!("File: {}", p.display()),
                                        format!("Permissions: {symbolic} ({octal})"),
                                        format!("Owner:Group: {owner}"),
                                        format!("Size:     {size_str} bytes"),
                                    ];
                                    state.viewer_scroll = 0;
                                }
                            }
                        }
                        // Ctrl+I: file info (stat)
                        KeyCode::Char('i') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            if let Some(entry) = entries.get(cursor) {
                                if let Location::Local(dir) = &pane.location {
                                    let path = dir.join(&entry.name);
                                    if let Ok(meta) = std::fs::symlink_metadata(&path) {
                                        let k = match entry.kind {
                                            EntryKind::Directory => "d",
                                            EntryKind::Symlink => "l",
                                            _ => "-",
                                        };
                                        let mode = format!(
                                            "{k}r--r--r-- {}",
                                            if meta.permissions().readonly() {
                                                "ro"
                                            } else {
                                                "rw"
                                            }
                                        );
                                        let size = entry.size.map(format_size).unwrap_or_default();
                                        let info = vec![
                                            format!("Name:      {}", entry.name),
                                            format!("Path:      {}", path.display()),
                                            format!("Type:      {:?}", entry.kind),
                                            format!("Size:      {size}"),
                                            format!("Mode:      {mode}"),
                                        ];
                                        state.viewer_content = info;
                                        state.viewer_scroll = 0;
                                    }
                                }
                            }
                        }
                        // Alt+O: sync other pane to active pane
                        KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::ALT) => {
                            let src = state.active_pane().location.clone();
                            let dst = state.other_pane_mut();
                            dst.location = src;
                            dst.cursor = 0;
                            state.message = Some("Directory synced".into());
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
                        // Alt+Down: go back in directory history
                        KeyCode::Down if key.modifiers.contains(KeyModifiers::ALT) => {
                            let pane = state.active_pane_mut();
                            if let Some(prev) = pane.dir_history.pop() {
                                pane.location = prev;
                                pane.cursor = 0;
                                state.message = Some("History back".into());
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
                        // Ctrl+\\: open active directory in file explorer
                        KeyCode::Char('\\') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            if let Location::Local(dir) = &state.active_pane().location {
                                let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
                                let dir_c = dir.clone();
                                let _dir_loc = Location::Local(dir_c.clone());
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
                            let _ = std::process::Command::new(
                                std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()),
                            )
                            .status();
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
                        // F5: copy selected (or cursor) from active pane to other pane — background
                        KeyCode::F(5) => {
                            let names = selection_or_cursor(&state, entries, cursor);
                            if names.is_empty() {
                                continue;
                            }
                            match op_paths(&state) {
                                Some((src, dst)) => {
                                    let id = format!("copy-{}", state.jobs.len());
                                    let desc =
                                        format!("Copy {} → {}", names.join(", "), dst.display());
                                    state.jobs.push(arx::jobs::Job {
                                        id: id.clone(),
                                        description: desc,
                                        kind: arx::jobs::JobKind::Copy,
                                        status: arx::jobs::JobStatus::Pending,
                                        progress: 0,
                                        source: Some(Location::Local(src.clone())),
                                        destination: Some(Location::Local(dst.clone())),
                                    });
                                    let tx = job_tx.clone();
                                    let names2 = names.clone();
                                    let use_rsync = std::process::Command::new("rsync")
                                        .arg("--version")
                                        .output()
                                        .map(|o| o.status.success())
                                        .unwrap_or(false);
                                    tokio::task::spawn_blocking(move || {
                                        tx.send(arx::jobs::JobEvent::Running { id: id.clone() })
                                            .ok();
                                        if use_rsync {
                                            // ponytail: rsync -avh --progress for each selected file
                                            let mut ok = 0u64;
                                            for name in &names2 {
                                                let src_path = src.join(name);
                                                if src_path.is_dir() {
                                                    let status =
                                                        std::process::Command::new("rsync")
                                                            .args([
                                                                "-avh",
                                                                "--progress",
                                                                src_path.to_str().unwrap_or(""),
                                                                dst.to_str().unwrap_or(""),
                                                            ])
                                                            .status();
                                                    if status.map(|s| s.success()).unwrap_or(false)
                                                    {
                                                        ok += 1;
                                                    }
                                                } else if let Ok(status) =
                                                    std::process::Command::new("rsync")
                                                        .args([
                                                            "-avh",
                                                            "--progress",
                                                            src_path.to_str().unwrap_or(""),
                                                            dst.to_str().unwrap_or(""),
                                                        ])
                                                        .status()
                                                {
                                                    if status.success() {
                                                        ok += 1;
                                                    }
                                                }
                                            }
                                            tx.send(arx::jobs::JobEvent::Done {
                                                id,
                                                message: format!(
                                                    "rsync: {ok}/{} file(s)",
                                                    names2.len()
                                                ),
                                            })
                                            .ok();
                                        } else {
                                            let src_loc = Location::Local(src.clone());
                                            let result = src_loc.copy_files(&src, &dst, &names2);
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
                                        }
                                    });
                                    state.message =
                                        Some(format!("Copy queued (job {})", state.jobs.len()));
                                }
                                None => {
                                    state.message =
                                        Some("Both panes must be local for copy".into());
                                }
                            }
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
                                    let desc =
                                        format!("Move {} → {}", names.join(", "), dst.display());
                                    state.jobs.push(arx::jobs::Job {
                                        id: id.clone(),
                                        description: desc,
                                        kind: arx::jobs::JobKind::Copy,
                                        status: arx::jobs::JobStatus::Pending,
                                        progress: 0,
                                        source: Some(Location::Local(src.clone())),
                                        destination: Some(Location::Local(dst.clone())),
                                    });
                                    let tx = job_tx.clone();
                                    let names2 = names.clone();
                                    tokio::task::spawn_blocking(move || {
                                        tx.send(arx::jobs::JobEvent::Running { id: id.clone() })
                                            .ok();
                                        let src_loc = Location::Local(src.clone());
                                        let result = src_loc.move_files(&src, &dst, &names2);
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
                                    state.message =
                                        Some(format!("Move queued (job {})", state.jobs.len()));
                                }
                                None => {
                                    state.message =
                                        Some("Both panes must be local for move".into());
                                }
                            }
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
                        }
                        // F8: delete selected (or cursor) from active pane
                        KeyCode::F(8) => {
                            let names = selection_or_cursor(&state, entries, cursor);
                            let active_path = pane_location_path(&state);
                            if let Some(dir) = active_path {
                                let loc = Location::Local(dir.to_path_buf());
                                match loc.delete_files(dir, &names) {
                                    Ok(n) => {
                                        state.message = Some(format!("Trashed {n} item(s)"));
                                    }
                                    Err(e) => {
                                        state.message = Some(format!("Delete error: {e}"));
                                    }
                                }
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
                        // F3: view file
                        KeyCode::F(3) => {
                            if let Some(entry) = entries.get(cursor) {
                                if entry.kind != EntryKind::Directory {
                                    let path = match &pane.location {
                                        Location::Local(dir) => dir.join(&entry.name),
                                        _ => continue,
                                    };
                                    if key.modifiers.contains(KeyModifiers::SHIFT) {
                                        // ponytail: use bat for syntax-highlighted paging
                                        let _ = std::process::Command::new("bat")
                                            .arg("--paging=always")
                                            .arg(&path)
                                            .status();
                                    } else {
                                        state.viewer_content = preview_file(&path);
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
                                let _ = std::process::Command::new(&editor_cmd).arg(&path).status();
                                execute!(terminal.backend_mut(), EnterAlternateScreen)?;
                                enable_raw_mode()?;
                                // Refresh after edit
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
                        // Ctrl+C: copy filename to clipboard
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            if let Some(entry) = entries.get(cursor) {
                                let name = &entry.name;
                                if let Location::Local(dir) = &pane.location {
                                    let full = dir.join(name);
                                    let path = full.to_string_lossy();
                                    let _ = std::process::Command::new("sh")
                                    .arg("-c")
                                    .arg(format!(
                                        "echo -n '{}' | xclip -selection clipboard 2>/dev/null || echo -n '{}' | wl-copy 2>/dev/null || printf '%s' '{}'",
                                        path, path, path
                                    ))
                                    .output();
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
                            if state.job_cursor < state.jobs.len() {
                                state.jobs.remove(state.job_cursor);
                                state.job_cursor =
                                    state.job_cursor.min(state.jobs.len().saturating_sub(1));
                            }
                        }
                        KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            state.show_jobs = !state.show_jobs;
                            state.job_cursor = 0;
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
                        // Ctrl+X S: symlink (MC-style prefix)
                        KeyCode::Char('x') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            state.cmd_prefix = true;
                        }
                        KeyCode::Char('s')
                            if key.modifiers.contains(KeyModifiers::CONTROL) && cmd_prefix =>
                        {
                            if let Some(entry) = entries.get(cursor) {
                                state.cmd = format!("ln -s '{}' ", entry.name);
                                state.cmd_input = true;
                            }
                            state.cmd_prefix = false;
                        }
                        KeyCode::Char('c')
                            if key.modifiers.contains(KeyModifiers::CONTROL) && cmd_prefix =>
                        {
                            state.cmd = "chmod ".into();
                            state.cmd_input = true;
                            state.cmd_prefix = false;
                        }
                        KeyCode::Char('l')
                            if key.modifiers.contains(KeyModifiers::CONTROL) && cmd_prefix =>
                        {
                            // Ctrl+X L: hard link
                            if let Some(entry) = entries.get(cursor) {
                                state.cmd = format!("ln '{}' ", entry.name);
                                state.cmd_input = true;
                            }
                            state.cmd_prefix = false;
                        }
                        KeyCode::Char('o')
                            if key.modifiers.contains(KeyModifiers::CONTROL) && cmd_prefix =>
                        {
                            // Ctrl+X O: chown
                            state.cmd = "chown ".into();
                            state.cmd_input = true;
                            state.cmd_prefix = false;
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
                                let show = state.show_hidden;
                                let sort = state.sort_mode;
                                left_entries = load_entries(&state.left.location, show, sort);
                                right_entries = load_entries(&state.right.location, show, sort);
                            }
                        }
                        // Ctrl+T: new tab in active pane
                        KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            let show_hidden = state.show_hidden;
                            let sort_mode = state.sort_mode;
                            state.active_pane_mut().new_tab();
                            let tabs = state.active_pane().tabs.len() + 1;
                            left_entries =
                                load_entries(&state.left.location, show_hidden, sort_mode);
                            right_entries =
                                load_entries(&state.right.location, show_hidden, sort_mode);
                            state.message = Some(format!("Tab {tabs}/{tabs}"));
                        }
                        // Ctrl+W: close tab in active pane
                        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            let show_hidden = state.show_hidden;
                            let sort_mode = state.sort_mode;
                            state.active_pane_mut().close_tab();
                            let tabs = state.active_pane().tabs.len() + 1;
                            left_entries =
                                load_entries(&state.left.location, show_hidden, sort_mode);
                            right_entries =
                                load_entries(&state.right.location, show_hidden, sort_mode);
                            state.message = Some(format!("Tab {}/{}", tabs.min(1), tabs));
                        }
                        // Ctrl+Left: previous tab
                        KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            let show_hidden = state.show_hidden;
                            let sort_mode = state.sort_mode;
                            let tabs_len = state.active_pane().tabs.len();
                            if tabs_len > 0 {
                                state.active_pane_mut().switch_tab(tabs_len - 1);
                                left_entries =
                                    load_entries(&state.left.location, show_hidden, sort_mode);
                                right_entries =
                                    load_entries(&state.right.location, show_hidden, sort_mode);
                                state.message = Some("Tab ←".into());
                            }
                        }
                        // Ctrl+Right: next tab
                        KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            let show_hidden = state.show_hidden;
                            let sort_mode = state.sort_mode;
                            if state.active_pane().tabs.len() >= 2 {
                                state.active_pane_mut().switch_tab(0);
                                left_entries =
                                    load_entries(&state.left.location, show_hidden, sort_mode);
                                right_entries =
                                    load_entries(&state.right.location, show_hidden, sort_mode);
                                state.message = Some("Tab →".into());
                            }
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

fn load_entries(location: &Location, show_hidden: bool, sort_mode: SortMode) -> Vec<Entry> {
    // ponytail: VfsOps trait dispatch handles all backends
    let mut entries = location.list().unwrap_or_default();
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
    state: &mut AppState,
    left_entries: &[&Entry],
    right_entries: &[&Entry],
    left_list: &mut ListState,
    right_list: &mut ListState,
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

    // Help overlay
    if state.show_help {
        render_help(frame, area);
    }

    // Viewer overlay
    if !state.viewer_content.is_empty() {
        render_viewer(frame, area, state);
    }

    // Command Center overlay (Ctrl+P)
    if state.show_command_center {
        let h = (state.command_matches.len().max(1) + 3).min(20) as u16;
        let popup = centered_rect(70, h, area);
        frame.render_widget(Clear, popup);
        let items: Vec<ListItem> = state
            .command_matches
            .iter()
            .map(|(l, _)| ListItem::new(l.as_str()))
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
    if state.show_command_center {
        let h = (state.command_matches.len().max(1) + 3).min(20) as u16;
        let popup = centered_rect(70, h, area);
        frame.render_widget(Clear, popup);
        let items: Vec<ListItem> = state
            .command_matches
            .iter()
            .map(|(l, _)| ListItem::new(l.as_str()))
            .collect();
        let list = ratatui::widgets::List::new(items)
            .block(Block::default().borders(Borders::ALL).title(format!(
                " ARX Command Center — :{}_ | bat chafa pdftotext ffprobe 7z ",
                state.filter
            )))
            .highlight_style(Style::default().fg(Color::Yellow));
        frame.render_stateful_widget(list, popup, &mut state.overlay_list_state);
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
    // Git info — ponytail: cached branch/dirty check per navigation, stat(2) on Cargo.toml/.git
    let git_info = git_branch_for(&pane.location);
    let msg_hint = message.map(|m| format!(" | {m}")).unwrap_or_default();

    let status = Paragraph::new(Line::from(format!(
        "ARX v0.1.0 | {}{hidden}{tab_info} | sel: {} |{hint}{msg_hint}{git_info} | ?: help",
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

/// Get git branch + dirty count for a local directory.
fn git_branch_for(loc: &arx::vfs::Location) -> String {
    let dir = match loc {
        arx::vfs::Location::Local(p) => p.clone(),
        _ => return String::new(),
    };
    // ponytail: check .git directly, no heavy status call
    if !dir.join(".git").exists() && !dir.join("HEAD").exists() {
        return String::new();
    }
    let branch = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(&dir)
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if branch.is_empty() {
        return String::new();
    }
    let dirty = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&dir)
        .output()
        .ok()
        .map(|o| o.stdout.iter().filter(|&&b| b == b'\n').count())
        .unwrap_or(0);
    if dirty > 0 {
        format!(" | git:{}+{}", branch, dirty)
    } else {
        format!(" | git:{}", branch)
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

/// Process a job event — updates state and refreshes file lists after copy/move/delete.
fn handle_job_event(
    ev: arx::jobs::JobEvent,
    state: &mut AppState,
    left: &mut Vec<Entry>,
    right: &mut Vec<Entry>,
) {
    match ev {
        arx::jobs::JobEvent::Running { id } => {
            if let Some(job) = state.jobs.iter_mut().find(|j| j.id == id) {
                job.status = arx::jobs::JobStatus::Running;
            }
        }
        arx::jobs::JobEvent::Done { id, message } => {
            if let Some(job) = state.jobs.iter_mut().find(|j| j.id == id) {
                job.status = arx::jobs::JobStatus::Done;
                job.progress = 100;
            }
            state.message = Some(message);
            *left = load_entries(&state.left.location, state.show_hidden, state.sort_mode);
            *right = load_entries(&state.right.location, state.show_hidden, state.sort_mode);
        }
        arx::jobs::JobEvent::Failed { id, error } => {
            if let Some(job) = state.jobs.iter_mut().find(|j| j.id == id) {
                job.status = arx::jobs::JobStatus::Failed;
            }
            state.message = Some(error);
            *left = load_entries(&state.left.location, state.show_hidden, state.sort_mode);
            *right = load_entries(&state.right.location, state.show_hidden, state.sort_mode);
        }
    }
}

/// Preview a file using system tools: bat, chafa, pdftotext, ffprobe, 7z.
/// Falls back to plain head(1) read.
/// Preview a file using system tools: chafa, pdftotext, ffprobe, 7z, bat.
fn preview_file(path: &std::path::Path) -> Vec<String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let path_str = path.to_str().unwrap_or("");

    // Images → chafa
    if matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp"
    ) && run_cmd(
        "chafa",
        &["--symbols", "block", "--size", "80x20", path_str],
    ) {
        let mut lines = run_cmd_output(
            "chafa",
            &["--symbols", "block", "--size", "80x20", path_str],
        );
        lines.insert(0, format!("[Image] {}", path.display()));
        return lines;
    }

    // PDF → pdftotext
    if ext == "pdf" && run_cmd("pdftotext", &["-l", "1", path_str, "-"]) {
        let mut lines = run_cmd_output("pdftotext", &["-l", "1", path_str, "-"]);
        lines.insert(
            0,
            format!("[PDF] {} — {} lines", path.display(), lines.len() - 1),
        );
        return lines;
    }

    // Media → ffprobe
    if matches!(
        ext.as_str(),
        "mp4" | "mkv" | "avi" | "mov" | "webm" | "mp3" | "flac"
    ) && run_cmd(
        "ffprobe",
        &[
            "-hide_banner",
            "-show_entries",
            "format=duration,size,bit_rate:stream=codec_type,codec_name,width,height",
            "-of",
            "default=noprint_wrappers=1",
            path_str,
        ],
    ) {
        let mut lines = run_cmd_output(
            "ffprobe",
            &[
                "-hide_banner",
                "-show_entries",
                "format=duration,size,bit_rate:stream=codec_type,codec_name,width,height",
                "-of",
                "default=noprint_wrappers=1",
                path_str,
            ],
        );
        lines.insert(0, format!("[Media] {}", path.display()));
        return lines;
    }

    // Archives
    if matches!(ext.as_str(), "zip" | "tar" | "gz" | "xz" | "7z" | "rar") {
        let (cmd, args) = if matches!(ext.as_str(), "7z" | "rar" | "zip") {
            ("7z", vec!["l", path_str])
        } else {
            ("tar", vec!["tvf", path_str])
        };
        if run_cmd(cmd, &args) {
            let mut lines = run_cmd_output(cmd, &args);
            lines.insert(0, format!("[Archive] {}", path.display()));
            return lines;
        }
    }

    // Code → bat
    if run_cmd(
        "bat",
        &[
            "--style=plain",
            "--color=never",
            "--paging=never",
            "--line-range=:200",
            path_str,
        ],
    ) {
        let mut lines = run_cmd_output(
            "bat",
            &[
                "--style=plain",
                "--color=never",
                "--paging=never",
                "--line-range=:200",
                path_str,
            ],
        );
        let total = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        lines.insert(0, format!("[Code] {} — {} bytes", path.display(), total));
        return lines;
    }

    // Fallback
    let loc = arx::vfs::Location::Local(
        path.parent()
            .unwrap_or(std::path::Path::new("/"))
            .to_path_buf(),
    );
    loc.read_head(path, 500)
        .unwrap_or_else(|e| vec![format!("Error: {e}")])
}

/// Check if a command runs successfully (exit 0).
fn run_cmd(cmd: &str, args: &[&str]) -> bool {
    std::process::Command::new(cmd)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Run a command and return stdout lines.
fn run_cmd_output(cmd: &str, args: &[&str]) -> Vec<String> {
    std::process::Command::new(cmd)
        .args(args)
        .output()
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

fn build_cc_matches(filter: &str, state: &AppState) -> Vec<(String, String)> {
    let q = filter.to_lowercase();
    let mut m = Vec::new();
    for h in arx::remote::hosts_config::load_hosts() {
        let l = format!("Host: {}", h.name);
        if l.to_lowercase().contains(&q) || h.hostname.to_lowercase().contains(&q) {
            m.push((l, format!("sftp://{}", h.hostname)));
        }
    }
    for loc in &state.bookmarks {
        let l = loc.to_string();
        if l.to_lowercase().contains(&q) {
            m.push((format!("Bookmark: {}", l), l));
        }
    }
    for d in &state.dir_history {
        let l = d.to_string_lossy().to_string();
        if l.to_lowercase().contains(&q) {
            m.push((format!("History: {}", l), l));
        }
    }
    for e in &state.menu {
        if e.label.to_lowercase().contains(&q) {
            m.push((format!("Cmd: {}", e.label), e.command.clone()));
        }
    }
    m.sort_by_key(|(l, _)| l.to_lowercase());
    m.truncate(50);
    m
}

fn navigate_to(state: &mut AppState, target: &str) {
    if target.starts_with("sftp://") {
        let h = target.trim_start_matches("sftp://");
        state.active_pane_mut().location = arx::vfs::Location::Sftp {
            host: h.into(),
            path: "/".into(),
        };
    } else if target.starts_with('/') || target.starts_with('~') {
        let p = std::path::PathBuf::from(
            target.replace('~', &std::env::var("HOME").unwrap_or_default()),
        );
        if p.is_dir() {
            state.active_pane_mut().location = arx::vfs::Location::Local(p);
        }
    } else {
        let _ = std::process::Command::new("sh")
            .arg("-c")
            .arg(target)
            .spawn();
    }
}
