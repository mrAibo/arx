use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;

use arx::app::AppState;
use arx::effect_dispatcher::{EffectDispatcher, EffectLane, EffectScope};
use arx::effects::Effect;

use super::*;

pub(super) fn render(frame: &mut Frame, area: Rect, state: &AppState) {
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

pub(super) fn handle_key(
    state: &mut AppState,
    key: KeyEvent,
    effect_dispatcher: &EffectDispatcher,
) -> bool {
    if !state.show_menu {
        return false;
    }
    match key.code {
        KeyCode::Esc | KeyCode::F(2) => {
            state.show_menu = false;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            state.menu_cursor = state.menu_cursor.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            let max = state.menu.len().saturating_sub(1);
            if state.menu_cursor < max {
                state.menu_cursor += 1;
            }
        }
        KeyCode::Enter => {
            if let Some(entry) = state.menu.get(state.menu_cursor) {
                let cmd = entry.command.clone();
                state.close_all_overlays();
                let id = effect_dispatcher.dispatch(
                    EffectLane::GlobalProcess,
                    EffectScope::Global,
                    Effect::RunShellCapture { command: cmd },
                );
                state.register_effect(EffectLane::GlobalProcess, id);
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

    fn dispatcher() -> EffectDispatcher {
        EffectDispatcher::channel(ProviderRegistry::new()).0
    }

    #[test]
    fn handle_key_inactive_returns_false() {
        let mut state = AppState::default();
        state.show_menu = false;
        let d = dispatcher();
        let handled = handle_key(&mut state, key(KeyCode::Char('a')), &d);
        assert!(!handled);
        assert_eq!(state.menu_cursor, 0);
    }

    #[test]
    fn handle_key_close_key_closes() {
        let mut state = AppState::default();
        state.show_menu = true;
        let d = dispatcher();
        let handled = handle_key(&mut state, key(KeyCode::Esc), &d);
        assert!(handled);
        assert!(!state.show_menu);
    }

    #[test]
    fn handle_key_up_down_clamp() {
        let mut state = AppState::default();
        state.show_menu = true;
        state.menu = vec![
            arx::app::MenuEntry {
                label: "one".into(),
                command: "echo one".into(),
            },
            arx::app::MenuEntry {
                label: "two".into(),
                command: "echo two".into(),
            },
        ];
        let d = dispatcher();
        handle_key(&mut state, key(KeyCode::Up), &d);
        assert_eq!(state.menu_cursor, 0);
        handle_key(&mut state, key(KeyCode::Down), &d);
        assert_eq!(state.menu_cursor, 1);
        handle_key(&mut state, key(KeyCode::Down), &d);
        assert_eq!(state.menu_cursor, 1);
    }
}
