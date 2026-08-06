use arx::app::{Action, AppState};
use arx::vfs::{EntryKind, Location, local::LocalFs};
use crossterm::{
    event::{self, Event, KeyCode},
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
    let mut entries: Vec<arx::vfs::Entry> = load_entries(&state.current_location);
    let mut list_state = ListState::default();
    list_state.select(Some(0));

    while !state.should_quit {
        terminal.draw(|frame| render(frame, &state, &entries, &mut list_state.clone()))?;

        #[allow(clippy::collapsible_if)]
        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Char('q') => state.apply(Action::Quit),
                KeyCode::Up | KeyCode::Char('k') => {
                    if state.cursor > 0 {
                        state.cursor -= 1;
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if state.cursor + 1 < entries.len() {
                        state.cursor += 1;
                    }
                }
                KeyCode::Enter => {
                    if let Some(entry) = entries.get(state.cursor) {
                        if entry.kind == EntryKind::Directory {
                            let new_path = match &state.current_location {
                                Location::Local(p) => p.join(&entry.name),
                                _ => continue,
                            };
                            state.current_location = Location::Local(new_path);
                            state.cursor = 0;
                            entries = load_entries(&state.current_location);
                        }
                    }
                }
                KeyCode::Backspace => {
                    if let Location::Local(p) = &state.current_location {
                        let parent = LocalFs::parent(p);
                        if parent != *p {
                            state.current_location = Location::Local(parent);
                            state.cursor = 0;
                            entries = load_entries(&state.current_location);
                        }
                    }
                }
                _ => {}
            }
            list_state.select(Some(state.cursor));
        }
    }
    Ok(())
}

fn load_entries(location: &Location) -> Vec<arx::vfs::Entry> {
    match location {
        Location::Local(path) => LocalFs::list(path).unwrap_or_default(),
        _ => vec![],
    }
}

fn render(
    frame: &mut ratatui::Frame,
    state: &AppState,
    entries: &[arx::vfs::Entry],
    list_state: &mut ListState,
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

    // Left pane: directory listing
    let items: Vec<ListItem> = entries
        .iter()
        .map(|e| {
            let icon = match e.kind {
                EntryKind::Directory => "📁 ",
                EntryKind::Symlink => "🔗 ",
                _ => "📄 ",
            };
            let size_str = e.size.map(format_size).unwrap_or_default();
            let line = Line::from(vec![
                Span::raw(icon),
                Span::raw(&e.name),
                Span::styled(
                    format!("  {size_str}"),
                    Style::default().fg(Color::DarkGray),
                ),
            ]);
            ListItem::new(line)
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} ", state.current_location.label())),
        )
        .highlight_style(Style::default().fg(Color::Black).bg(Color::White))
        .highlight_symbol(">> ");
    frame.render_stateful_widget(list, panes[0], list_state);

    // Right pane: placeholder
    let right = Paragraph::new(Line::from("No location selected"))
        .block(Block::default().borders(Borders::ALL).title(" Right "));
    frame.render_widget(right, panes[1]);

    // Status bar
    let loc_str = match &state.current_location {
        Location::Local(p) => p.display().to_string(),
        other => other.to_string(),
    };
    let status = Paragraph::new(Line::from(format!(
        "ARX v0.1.0 | {} | {}/{} | q: quit | j/k: nav | Enter: open | Backspace: up",
        loc_str,
        state.cursor.saturating_add(1),
        entries.len(),
    )))
    .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(status, chunks[1]);
}

// ponytail: simple size formatting; add --human-readable toggle when needed
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
