use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};

use super::*;

pub(super) fn handle_key(state: &mut AppState, key: KeyEvent) -> bool {
    if state.viewer_content.is_empty() {
        return false;
    }

    match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::F(3) => {
            state.viewer_content.clear();
        }
        KeyCode::Up | KeyCode::Char('k') => {
            state.viewer_scroll = state.viewer_scroll.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.viewer_scroll = (state.viewer_scroll + 1).min(state.viewer_content.len() - 1);
        }
        KeyCode::PageUp => {
            state.viewer_scroll = state.viewer_scroll.saturating_sub(20);
        }
        KeyCode::PageDown => {
            state.viewer_scroll = (state.viewer_scroll + 20).min(state.viewer_content.len() - 1);
        }
        _ => {}
    }

    true
}

pub(super) fn render(frame: &mut Frame, area: Rect, state: &AppState) {
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

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    fn active_state(scroll: usize) -> AppState {
        AppState {
            viewer_content: vec!["alpha".into(), "beta".into(), "gamma".into()],
            viewer_scroll: scroll,
            ..Default::default()
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn inactive_does_not_consume_or_mutate() {
        let mut state = AppState {
            viewer_scroll: 7,
            ..Default::default()
        };

        assert!(!handle_key(&mut state, key(KeyCode::Down)));
        assert_eq!(state.viewer_scroll, 7);
    }

    #[test]
    fn close_keys_clear_content() {
        for code in [KeyCode::Esc, KeyCode::Char('q'), KeyCode::F(3)] {
            let mut state = active_state(0);
            assert!(handle_key(&mut state, key(code)));
            assert!(state.viewer_content.is_empty());
        }
    }

    #[test]
    fn up_saturates() {
        let mut state = active_state(0);
        assert!(handle_key(&mut state, key(KeyCode::Up)));
        assert_eq!(state.viewer_scroll, 0);
    }

    #[test]
    fn down_clamps() {
        let mut state = active_state(2);
        assert!(handle_key(&mut state, key(KeyCode::Down)));
        assert_eq!(state.viewer_scroll, 2);
    }

    #[test]
    fn page_keys_clamp() {
        let mut state = active_state(2);
        assert!(handle_key(&mut state, key(KeyCode::PageUp)));
        assert_eq!(state.viewer_scroll, 0);

        assert!(handle_key(&mut state, key(KeyCode::PageDown)));
        assert_eq!(state.viewer_scroll, 2);
    }

    #[test]
    fn unhandled_key_is_consumed_while_active() {
        let mut state = active_state(1);
        assert!(handle_key(&mut state, key(KeyCode::Char('x'))));
        assert_eq!(state.viewer_scroll, 1);
        assert_eq!(state.viewer_content.len(), 3);
    }

    #[test]
    fn render_shows_content_and_title() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = active_state(1);

        terminal.draw(|f| render(f, f.area(), &state)).unwrap();

        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol().to_owned())
            .collect::<Vec<_>>()
            .join("");

        assert!(text.contains("View (3 lines, 33%)"));
        assert!(text.contains("1/2 | j/k:scroll q/Esc:close"));
        assert!(text.contains("beta"));
    }
}
