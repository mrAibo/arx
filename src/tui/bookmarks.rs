use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;

use arx::app::AppState;
use arx::services::{PaneLoadPurpose, PaneLoader};

use super::*;

pub(super) fn render(frame: &mut Frame, area: Rect, state: &AppState) {
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

pub(super) fn handle_key(state: &mut AppState, key: KeyEvent, pane_loader: &PaneLoader) -> bool {
    if !state.show_bookmarks {
        return false;
    }
    match key.code {
        KeyCode::Esc | KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.show_bookmarks = false;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            state.bookmark_cursor = state.bookmark_cursor.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
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
                    pane_loader,
                    state,
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
    true
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;
    use arx::vfs::ProviderRegistry;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn loader() -> PaneLoader {
        PaneLoader::channel(ProviderRegistry::new()).0
    }

    #[test]
    fn handle_key_inactive_returns_false() {
        let mut state = AppState::default();
        state.show_bookmarks = false;
        let handled = handle_key(&mut state, key(KeyCode::Char('a')), &loader());
        assert!(!handled);
        assert_eq!(state.bookmark_cursor, 0);
    }

    #[test]
    fn handle_key_close_key_closes() {
        let mut state = AppState::default();
        state.show_bookmarks = true;
        let key = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL);
        let handled = handle_key(&mut state, key, &loader());
        assert!(handled);
        assert!(!state.show_bookmarks);
    }

    #[test]
    fn handle_key_up_does_not_underflow() {
        let mut state = AppState::default();
        state.show_bookmarks = true;
        state.bookmarks = vec![Location::Local("/a".into())];
        handle_key(&mut state, key(KeyCode::Up), &loader());
        assert_eq!(state.bookmark_cursor, 0);
    }

    #[test]
    fn handle_key_down_advances_and_clamps() {
        let mut state = AppState::default();
        state.show_bookmarks = true;
        state.bookmarks = vec![
            Location::Local("/a".into()),
            Location::Local("/b".into()),
            Location::Local("/c".into()),
        ];
        handle_key(&mut state, key(KeyCode::Down), &loader());
        assert_eq!(state.bookmark_cursor, 1);
        handle_key(&mut state, key(KeyCode::Down), &loader());
        handle_key(&mut state, key(KeyCode::Down), &loader());
        assert_eq!(state.bookmark_cursor, 2);
    }
}
