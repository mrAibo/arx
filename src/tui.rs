use arx::app::{Action, AppState};
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    DefaultTerminal,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, Paragraph},
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

    while !state.should_quit {
        terminal.draw(|frame| render(frame, &state))?;
        #[allow(clippy::collapsible_if)]
        if let Event::Key(key) = event::read()? {
            if let KeyCode::Char('q') = key.code {
                state.apply(Action::Quit);
            }
        }
    }
    Ok(())
}

fn render(frame: &mut ratatui::Frame, _state: &AppState) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)])
        .split(chunks[0]);

    let left_pane = Paragraph::new(Line::from("Loading..."))
        .block(Block::default().borders(Borders::ALL).title(" Left "));
    frame.render_widget(left_pane, panes[0]);

    let right_pane = Paragraph::new(Line::from("Loading..."))
        .block(Block::default().borders(Borders::ALL).title(" Right "));
    frame.render_widget(right_pane, panes[1]);

    let status = Paragraph::new(Line::from("ARX v0.1.0 | q: quit | Tab: switch pane"))
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(status, chunks[1]);
}
