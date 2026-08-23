use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;

use arx::app::AppState;
use arx::services::{PaneLoadPurpose, PaneLoader};
use arx::vfs::Location;

use super::*;

const HOSTS_CONFIG_PATH: &str = "~/.config/arx/hosts.toml";

pub(super) fn empty_hosts_text() -> String {
    format!("No hosts configured\n\nAdd hosts to:\n{HOSTS_CONFIG_PATH}\n\nEsc Close")
}

pub(super) fn render(frame: &mut Frame, area: Rect, state: &AppState) {
    if state.hosts.is_empty() {
        let popup_area = centered_rect(60, 40, area);
        frame.render_widget(Clear, popup_area);
        frame.render_widget(
            Paragraph::new(empty_hosts_text())
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Remote Hosts ")
                        .border_style(Style::default().fg(Color::Cyan)),
                )
                .wrap(Wrap { trim: false }),
            popup_area,
        );
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

pub(super) fn handle_key(state: &mut AppState, key: KeyEvent, pane_loader: &PaneLoader) -> bool {
    if !state.show_hosts {
        return false;
    }
    match key.code {
        KeyCode::Esc => {
            state.show_hosts = false;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            state.host_cursor = state.host_cursor.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            let max = state.hosts.len().saturating_sub(1);
            if state.host_cursor < max {
                state.host_cursor += 1;
            }
        }
        KeyCode::Enter => {
            let host = state.hosts.get(state.host_cursor).cloned();
            if let Some(host) = host {
                let default_path = host.default_path.as_deref().unwrap_or("/");
                let target = Location::Sftp {
                    host: host.id.clone(),
                    path: default_path.into(),
                };
                let active = state.active;
                state.close_all_overlays();
                schedule_pane_navigation(
                    pane_loader,
                    state,
                    active,
                    target,
                    PaneLoadPurpose::Navigate {
                        remember_current: true,
                    },
                );
                state.message = Some(format!("Connecting to {}…", host.name));
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
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn loader() -> PaneLoader {
        PaneLoader::channel(ProviderRegistry::new()).0
    }

    fn host(name: &str, id: &str) -> arx::remote::Host {
        let mut h = arx::remote::Host::from_alias(id);
        h.name = name.into();
        h
    }

    #[test]
    fn handle_key_inactive_returns_false() {
        let mut state = AppState::default();
        state.show_hosts = false;
        let handled = handle_key(&mut state, key(KeyCode::Char('a')), &loader());
        assert!(!handled);
        assert_eq!(state.host_cursor, 0);
    }

    #[test]
    fn handle_key_close_key_closes() {
        let mut state = AppState::default();
        state.show_hosts = true;
        let handled = handle_key(&mut state, key(KeyCode::Esc), &loader());
        assert!(handled);
        assert!(!state.show_hosts);
    }

    #[test]
    fn handle_key_up_at_zero_stays_zero() {
        let mut state = AppState::default();
        state.show_hosts = true;
        state.hosts = vec![host("A", "a")];
        handle_key(&mut state, key(KeyCode::Up), &loader());
        assert_eq!(state.host_cursor, 0);
    }

    #[test]
    fn handle_key_down_advances_and_clamps() {
        let mut state = AppState::default();
        state.show_hosts = true;
        state.hosts = vec![host("A", "a"), host("B", "b")];
        handle_key(&mut state, key(KeyCode::Down), &loader());
        assert_eq!(state.host_cursor, 1);
        handle_key(&mut state, key(KeyCode::Down), &loader());
        assert_eq!(state.host_cursor, 1);
    }

    #[test]
    fn render_hides_host_id() {
        let mut state = AppState::default();
        state.show_hosts = true;
        state.hosts = vec![host("Example", "example.test")];
        let mut term = Terminal::new(TestBackend::new(120, 24)).unwrap();
        term.draw(|f| render(f, f.area(), &state)).unwrap();
        let buf = term.backend().buffer().clone();
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains("Example (example.test)"));
        assert!(!text.contains("example.test (example.test)"));
        assert!(!text.contains("internal-id"));
    }
}
