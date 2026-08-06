use arx::app::{Action, AppState, Pane, PaneState};
use arx::vfs::{Entry, EntryKind, Location, local::LocalFs, sftp::SftpFs};
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

pub fn run() -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(stdout))?;

    let result = event_loop(&mut terminal);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn event_loop(terminal: &mut DefaultTerminal) -> io::Result<()> {
    let mut state = AppState::default();
    let mut left_entries = load_entries(&state.left.location, state.show_hidden);
    let mut right_entries = load_entries(&state.right.location, state.show_hidden);
    let mut left_list = ListState::default();
    let mut right_list = ListState::default();

    while !state.should_quit {
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
            if state.filtering || state.glob_input || state.go_input {
                match key.code {
                    KeyCode::Esc => {
                        state.filter.clear();
                        state.filtering = false;
                        state.glob_input = false;
                        state.go_input = false;
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
                                left_entries =
                                    load_entries(&state.left.location, state.show_hidden);
                                right_entries =
                                    load_entries(&state.right.location, state.show_hidden);
                            } else {
                                state.message = Some(format!("Not a directory: {}", state.filter));
                            }
                            state.filter.clear();
                        }
                        state.filtering = false;
                        state.glob_input = false;
                        state.go_input = false;
                    }
                    KeyCode::Backspace => {
                        state.filter.pop();
                    }
                    KeyCode::Char(c) => {
                        state.filter.push(c);
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
                KeyCode::Down | KeyCode::Char('j') => {
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
                            let new_path = match &pane.location {
                                Location::Local(p) => p.join(&entry.name),
                                _ => continue,
                            };
                            pane.location = Location::Local(new_path);
                            pane.cursor = 0;
                            state.selected.clear();
                            if state.active == Pane::Left {
                                left_entries =
                                    load_entries(&state.left.location, state.show_hidden);
                            } else {
                                right_entries =
                                    load_entries(&state.right.location, state.show_hidden);
                            }
                        }
                    }
                }
                KeyCode::Backspace => {
                    if let Location::Local(p) = &pane.location {
                        let parent = LocalFs::parent(p);
                        if parent != *p {
                            pane.location = Location::Local(parent);
                            pane.cursor = 0;
                            state.selected.clear();
                            if state.active == Pane::Left {
                                left_entries =
                                    load_entries(&state.left.location, state.show_hidden);
                            } else {
                                right_entries =
                                    load_entries(&state.right.location, state.show_hidden);
                            }
                        }
                    }
                }
                KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    left_entries = load_entries(&state.left.location, state.show_hidden);
                    right_entries = load_entries(&state.right.location, state.show_hidden);
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
                    left_entries = load_entries(&state.left.location, state.show_hidden);
                    right_entries = load_entries(&state.right.location, state.show_hidden);
                }
                // F5: copy selected (or cursor) from active pane to other pane
                KeyCode::F(5) => {
                    let names = selection_or_cursor(&state, entries, cursor);
                    let result = do_op(&state, |src, dst| LocalFs::copy_files(src, dst, &names));
                    state.message = Some(match result {
                        Ok(n) => format!("Copied {n} item(s)"),
                        Err(e) => format!("Copy error: {e}"),
                    });
                    state.selected.clear();
                    left_entries = load_entries(&state.left.location, state.show_hidden);
                    right_entries = load_entries(&state.right.location, state.show_hidden);
                }
                // F6: move selected (or cursor) from active pane to other pane
                KeyCode::F(6) => {
                    let names = selection_or_cursor(&state, entries, cursor);
                    let result = do_op(&state, |src, dst| LocalFs::move_files(src, dst, &names));
                    state.message = Some(match result {
                        Ok(n) => format!("Moved {n} item(s)"),
                        Err(e) => format!("Move error: {e}"),
                    });
                    state.selected.clear();
                    left_entries = load_entries(&state.left.location, state.show_hidden);
                    right_entries = load_entries(&state.right.location, state.show_hidden);
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
                        left_entries = load_entries(&state.left.location, state.show_hidden);
                        right_entries = load_entries(&state.right.location, state.show_hidden);
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
                _ => {}
            }
        }
    }
    Ok(())
}

fn load_entries(location: &Location, show_hidden: bool) -> Vec<Entry> {
    let mut entries = match location {
        Location::Local(path) => LocalFs::list(path).unwrap_or_default(),
        Location::Sftp { host, path } => SftpFs::list(host, path).unwrap_or_default(),
        _ => vec![],
    };
    if !show_hidden {
        entries.retain(|e| !e.name.starts_with('.'));
    }
    entries
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

fn do_op<F>(state: &AppState, op: F) -> io::Result<usize>
where
    F: FnOnce(&Path, &Path) -> io::Result<usize>,
{
    let src = pane_location_path(state)
        .ok_or_else(|| io::Error::other("active pane is not a local directory"))?;
    let dst = other_pane_location_path(state)
        .ok_or_else(|| io::Error::other("other pane is not a local directory"))?;
    op(src, dst)
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

    render_pane(
        frame,
        panes[0],
        &state.left,
        left_entries,
        left_list,
        state.active == Pane::Left,
        &state.selected,
    );
    render_pane(
        frame,
        panes[1],
        &state.right,
        right_entries,
        right_list,
        state.active == Pane::Right,
        &state.selected,
    );

    // Help overlay
    if state.show_help {
        render_help(frame, area);
    }

    // Status bar
    let pane = state.active_pane();
    let loc_str = match &pane.location {
        Location::Local(p) => p.display().to_string(),
        other => other.to_string(),
    };
    let hint = if state.go_input {
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
    let msg_hint = message.map(|m| format!(" | {m}")).unwrap_or_default();

    let status = Paragraph::new(Line::from(format!(
        "ARX v0.1.0 | {}{hidden} | sel: {} |{hint}{msg_hint} | ?: help",
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

fn render_pane(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    pane: &PaneState,
    entries: &[&Entry],
    list_state: &mut ListState,
    active: bool,
    selected: &std::collections::BTreeSet<String>,
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
