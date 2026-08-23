use super::{Action, AppState, Pane};
use arx::vfs::Location;
use crossterm::event::{KeyCode as KC, KeyEvent};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, Paragraph},
};

pub(super) fn handle_action(state: &mut AppState, action: &Action) -> bool {
    if !matches!(action, Action::ToggleEmbeddedTerminal) {
        return false;
    }

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
    true
}

pub(super) fn handle_key(state: &mut AppState, key: KeyEvent) {
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

pub(super) fn render_if_active(frame: &mut Frame, area: Rect, state: &AppState) -> bool {
    if !state.show_terminal {
        return false;
    }

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
            .take(area.height.saturating_sub(2) as usize)
            .map(|s| Line::from(s.as_str()))
            .collect();
        frame.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Terminal ")
                    .border_style(border_style),
            ),
            area,
        );
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    use std::cell::Cell;

    fn render_result(state: &AppState) -> bool {
        let backend = TestBackend::new(20, 5);
        let mut terminal = Terminal::new(backend).unwrap();
        let rendered = Cell::new(false);
        terminal
            .draw(|frame| {
                rendered.set(render_if_active(frame, Rect::new(0, 0, 20, 5), state));
            })
            .unwrap();
        rendered.get()
    }

    #[test]
    fn unrelated_action_is_not_handled() {
        let mut state = AppState::default();
        assert!(!handle_action(&mut state, &Action::OpenHelp));
    }

    #[test]
    fn render_is_inactive_when_terminal_is_disabled() {
        assert!(!render_result(&AppState::default()));
    }

    #[test]
    fn terminal_mode_without_term_still_replaces_right_pane() {
        let state = AppState {
            show_terminal: true,
            term: None,
            ..AppState::default()
        };
        assert!(render_result(&state));
    }

    #[test]
    fn closing_without_term_is_safe() {
        let mut state = AppState {
            show_terminal: true,
            term: None,
            ..AppState::default()
        };
        assert!(handle_action(&mut state, &Action::ToggleEmbeddedTerminal));
        assert!(!state.show_terminal);
        assert!(state.term.is_none());
        assert_eq!(state.message.as_deref(), Some("Terminal closed"));
    }
}
