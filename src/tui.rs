use arx::app::{Action, AppState, Pane, PaneState};
use arx::vfs::{Entry, EntryKind, Location, local::LocalFs};
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    DefaultTerminal,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};
use std::io;

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
    let mut left_entries = load_entries(&state.left.location);
    let mut right_entries = load_entries(&state.right.location);
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

        terminal.draw(|frame| {
            render(
                frame,
                &state,
                &left_filtered,
                &right_filtered,
                &mut left_list.clone(),
                &mut right_list.clone(),
            )
        })?;

        #[allow(clippy::collapsible_if)]
        if let Event::Key(key) = event::read()? {
            // If composing filter, keys go to filter buffer
            if state.filtering {
                match key.code {
                    KeyCode::Esc => {
                        state.filter.clear();
                        state.filtering = false;
                    }
                    KeyCode::Enter => {
                        state.filtering = false;
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
                            // reload entries for this pane
                            if state.active == Pane::Left {
                                left_entries = load_entries(&state.left.location);
                            } else {
                                right_entries = load_entries(&state.right.location);
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
                                left_entries = load_entries(&state.left.location);
                            } else {
                                right_entries = load_entries(&state.right.location);
                            }
                        }
                    }
                }
                KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    // manual refresh
                    left_entries = load_entries(&state.left.location);
                    right_entries = load_entries(&state.right.location);
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn load_entries(location: &Location) -> Vec<Entry> {
    match location {
        Location::Local(path) => LocalFs::list(path).unwrap_or_default(),
        _ => vec![],
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

fn render(
    frame: &mut ratatui::Frame,
    state: &AppState,
    left_entries: &[&Entry],
    right_entries: &[&Entry],
    left_list: &mut ListState,
    right_list: &mut ListState,
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

    // Status bar
    let pane = state.active_pane();
    let loc_str = match &pane.location {
        Location::Local(p) => p.display().to_string(),
        other => other.to_string(),
    };
    let filter_hint = if state.filtering {
        format!(" filter: {}_", state.filter)
    } else if !state.filter.is_empty() {
        format!(" filter: {}", state.filter)
    } else {
        String::new()
    };

    let status = Paragraph::new(Line::from(format!(
        "ARX v0.1.0 | {} | sel: {} |{filter_hint} | q: quit | Tab: switch | Space: select | /: filter",
        loc_str,
        state.selected.len(),
    )))
    .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(status, chunks[1]);
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
