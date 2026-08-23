use arx::app::{AppState, OverlayKind};
use arx::services::{PaneLoadPurpose, PaneLoader};
use arx::vfs::Location;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem};

use super::{centered_rect_lines, schedule_pane_navigation};

pub(super) fn open(state: &mut AppState) {
    let entries = AppState::load_hotlist();
    state.open_overlay(OverlayKind::Hotlist);
    state.hotlist_entries = entries;
}

pub(super) fn handle_key(state: &mut AppState, key: KeyEvent, pane_loader: &PaneLoader) -> bool {
    if !state.show_hotlist {
        return false;
    }

    match key.code {
        KeyCode::Esc => state.close_overlay(OverlayKind::Hotlist),
        KeyCode::Up | KeyCode::Char('k') => {
            state.hotlist_cursor = state.hotlist_cursor.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            let max = state.hotlist_entries.len().saturating_sub(1);
            if state.hotlist_cursor < max {
                state.hotlist_cursor += 1;
            }
        }
        KeyCode::Enter => {
            let selected = state.hotlist_entries.get(state.hotlist_cursor).cloned();
            if let Some(selected) = selected {
                let active = state.active;
                schedule_pane_navigation(
                    pane_loader,
                    state,
                    active,
                    Location::Local(selected),
                    PaneLoadPurpose::Navigate {
                        remember_current: true,
                    },
                );
                state.close_overlay(OverlayKind::Hotlist);
                state.message = Some("Opening hotlist entry…".into());
            }
        }
        _ => {}
    }

    true
}

pub(super) fn render(frame: &mut Frame, area: Rect, state: &AppState) {
    // The empty state still renders one truthful placeholder row, so reserve
    // one content line plus the two border lines even when the configured list
    // has zero entries.
    let content_lines = state.hotlist_entries.len().max(1);
    let h = (content_lines + 2).min(20) as u16;
    let popup = centered_rect_lines(60, h, area);
    frame.render_widget(Clear, popup);
    let items: Vec<ListItem> = if state.hotlist_entries.is_empty() {
        vec![ListItem::new("(empty - create ~/.config/arx/hotlist)")]
    } else {
        state
            .hotlist_entries
            .iter()
            .enumerate()
            .map(|(i, path)| {
                let prefix = if i == state.hotlist_cursor {
                    "> "
                } else {
                    "  "
                };
                ListItem::new(format!("{prefix}{}", path.display()))
            })
            .collect()
    };
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Hotlist (Enter: go, Esc: close) ")
                .border_style(Style::default().fg(Color::Magenta)),
        ),
        popup,
    );
}

#[cfg(test)]
mod tests {
    use super::{handle_key, render};
    use arx::app::AppState;
    use arx::services::{PaneLoadPurpose, PaneLoader};
    use arx::vfs::{Location, ProviderRegistry};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::path::PathBuf;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn loader() -> PaneLoader {
        PaneLoader::channel(ProviderRegistry::new()).0
    }

    #[test]
    fn inactive_returns_false_without_mutation() {
        let mut state = AppState {
            hotlist_cursor: 1,
            hotlist_entries: vec![PathBuf::from("/a"), PathBuf::from("/b")],
            ..Default::default()
        };
        let before = (
            state.show_hotlist,
            state.hotlist_cursor,
            state.hotlist_entries.clone(),
        );

        assert!(!handle_key(&mut state, key(KeyCode::Esc), &loader()));
        assert_eq!(
            (
                state.show_hotlist,
                state.hotlist_cursor,
                state.hotlist_entries
            ),
            before
        );
    }

    #[test]
    fn escape_closes() {
        let mut state = AppState {
            show_hotlist: true,
            ..Default::default()
        };

        assert!(handle_key(&mut state, key(KeyCode::Esc), &loader()));
        assert!(!state.show_hotlist);
    }

    #[test]
    fn up_at_zero_does_not_underflow() {
        let mut state = AppState {
            show_hotlist: true,
            hotlist_entries: vec![PathBuf::from("/a")],
            ..Default::default()
        };

        handle_key(&mut state, key(KeyCode::Up), &loader());
        assert_eq!(state.hotlist_cursor, 0);
    }

    #[test]
    fn down_advances_and_clamps() {
        let mut state = AppState {
            show_hotlist: true,
            hotlist_entries: vec![PathBuf::from("/a"), PathBuf::from("/b")],
            ..Default::default()
        };

        handle_key(&mut state, key(KeyCode::Down), &loader());
        assert_eq!(state.hotlist_cursor, 1);
        handle_key(&mut state, key(KeyCode::Char('j')), &loader());
        assert_eq!(state.hotlist_cursor, 1);
    }

    #[test]
    fn empty_enter_is_safe() {
        let mut state = AppState {
            show_hotlist: true,
            ..Default::default()
        };

        assert!(handle_key(&mut state, key(KeyCode::Enter), &loader()));
        assert!(state.show_hotlist);
        assert!(state.pending_pane_targets.is_empty());
        assert!(state.message.is_none());
    }

    #[tokio::test]
    async fn enter_schedules_selected_local_target() {
        let selected = PathBuf::from("/hotlist-target");
        let mut state = AppState {
            show_hotlist: true,
            hotlist_cursor: 1,
            hotlist_entries: vec![PathBuf::from("/other"), selected.clone()],
            ..Default::default()
        };

        assert!(handle_key(&mut state, key(KeyCode::Enter), &loader()));
        assert!(!state.show_hotlist);
        assert_eq!(state.message.as_deref(), Some("Opening hotlist entry…"));
        assert_eq!(
            state.pending_pane_targets.get(&state.active),
            Some(&(
                Location::Local(selected),
                PaneLoadPurpose::Navigate {
                    remember_current: true,
                },
            ))
        );
    }

    #[test]
    fn render_uses_logical_line_height_at_120x24() {
        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = AppState {
            show_hotlist: true,
            hotlist_entries: vec![
                PathBuf::from("/a"),
                PathBuf::from("/b"),
                PathBuf::from("/c"),
            ],
            ..Default::default()
        };

        terminal
            .draw(|frame| render(frame, frame.area(), &state))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let top = buffer
            .content()
            .iter()
            .position(|cell| cell.symbol() == "┌")
            .unwrap() as u16
            / buffer.area.width;
        let bottom = buffer
            .content()
            .iter()
            .position(|cell| cell.symbol() == "└")
            .unwrap() as u16
            / buffer.area.width;
        assert_eq!((top, bottom, bottom - top + 1), (9, 13, 5));
    }

    #[test]
    fn empty_render_reserves_a_content_row_for_truthful_placeholder() {
        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = AppState {
            show_hotlist: true,
            ..Default::default()
        };

        terminal
            .draw(|frame| render(frame, frame.area(), &state))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let rendered: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
        assert!(rendered.contains("(empty - create ~/.config/arx/hotlist)"));

        let top = buffer
            .content()
            .iter()
            .position(|cell| cell.symbol() == "┌")
            .unwrap() as u16
            / buffer.area.width;
        let bottom = buffer
            .content()
            .iter()
            .position(|cell| cell.symbol() == "└")
            .unwrap() as u16
            / buffer.area.width;
        assert_eq!(bottom - top + 1, 3);
    }

    #[test]
    fn render_body_has_no_file_loader_call() {
        let source = include_str!("hotlist.rs");
        let render_body = source
            .split_once("pub(super) fn render")
            .expect("render function exists")
            .1
            .split_once("#[cfg(test)]")
            .expect("test module follows render")
            .0;
        assert!(!render_body.contains(concat!("load_", "hotlist")));
    }
}
