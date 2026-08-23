use crossterm::event::{KeyEvent, KeyModifiers};
use ratatui::Frame;

use super::*;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum KeyOutcome {
    NotHandled,
    Consumed,
    Execute(CommandTarget),
}

pub(super) fn open(state: &mut AppState, focused_kind: Option<EntryKind>, editor_available: bool) {
    state.open_overlay(OverlayKind::CommandCenter);
    state.filter.clear();
    state.command_matches =
        build_command_items_with_file_context("", state, focused_kind, editor_available);
    state
        .overlay_list_state
        .select((!state.command_matches.is_empty()).then_some(0));
}

pub(super) fn handle_key(
    state: &mut AppState,
    key: KeyEvent,
    focused_kind: Option<EntryKind>,
    editor_available: bool,
) -> KeyOutcome {
    if !state.show_command_center {
        return KeyOutcome::NotHandled;
    }

    match key.code {
        KeyCode::Esc => {
            state.show_command_center = false;
            state.filter.clear();
            state.command_matches.clear();
            state.overlay_list_state = ListState::default();
        }
        KeyCode::Enter => {
            let idx = state.overlay_list_state.selected().unwrap_or(0);
            let idx = idx.min(state.command_matches.len().saturating_sub(1));
            if let Some(item) = state.command_matches.get(idx).cloned() {
                if let ActionAvailability::Disabled { reason } = item.availability {
                    state.message = Some(reason);
                    return KeyOutcome::Consumed;
                }
                state.show_command_center = false;
                state.filter.clear();
                state.command_matches.clear();
                return KeyOutcome::Execute(item.target);
            }
        }
        KeyCode::Up | KeyCode::Char('k') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            let current = state.overlay_list_state.selected().unwrap_or(0);
            state
                .overlay_list_state
                .select(Some(current.saturating_sub(1)));
        }
        KeyCode::Down | KeyCode::Char('j') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            let current = state.overlay_list_state.selected().unwrap_or(0);
            let max = state.command_matches.len().saturating_sub(1);
            state
                .overlay_list_state
                .select(Some((current + 1).min(max)));
        }
        KeyCode::Backspace => {
            state.filter.pop();
            rebuild(state, focused_kind, editor_available);
        }
        KeyCode::Char(c)
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            state.filter.push(c);
            rebuild(state, focused_kind, editor_available);
        }
        _ => {}
    }

    KeyOutcome::Consumed
}

fn rebuild(state: &mut AppState, focused_kind: Option<EntryKind>, editor_available: bool) {
    state.command_matches =
        build_command_items_with_file_context(&state.filter, state, focused_kind, editor_available);
    state
        .overlay_list_state
        .select((!state.command_matches.is_empty()).then_some(0));
}

pub(super) fn render(frame: &mut Frame, area: Rect, state: &AppState) {
    let h = (state.command_matches.len().max(1) + 3).min(20) as u16;
    let popup = centered_rect_lines(70, h, area);
    frame.render_widget(Clear, popup);
    let items: Vec<ListItem> = state
        .command_matches
        .iter()
        .map(|item| {
            let style = if !item.availability.is_available() {
                Style::default().fg(Color::DarkGray)
            } else {
                match item.kind {
                    CommandKind::Action => Style::default().fg(Color::Cyan),
                    CommandKind::Host => Style::default().fg(Color::Green),
                    CommandKind::Bookmark => Style::default().fg(Color::Magenta),
                    CommandKind::History => Style::default(),
                    CommandKind::Session => Style::default().fg(Color::Yellow),
                    CommandKind::UserCommand => Style::default().fg(Color::Blue),
                }
            };
            let line = match item.availability.reason() {
                Some(reason) => format!("{}  —  unavailable: {reason}", item.display_line()),
                None => item.display_line(),
            };
            ListItem::new(line).style(style)
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(format!(
            " ARX Command Center — :{}_ | bat chafa pdftotext ffprobe 7z ",
            state.filter
        )))
        .highlight_style(Style::default().fg(Color::Yellow));
    let mut list_state = state.overlay_list_state;
    frame.render_stateful_widget(list, popup, &mut list_state);
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn item(target: CommandTarget, availability: ActionAvailability) -> CommandItem {
        CommandItem {
            title: "Blocked Demo".into(),
            subtitle: Some("example subtitle".into()),
            kind: CommandKind::UserCommand,
            target,
            score: 0,
            availability,
        }
    }

    fn active_state(items: Vec<CommandItem>) -> AppState {
        AppState {
            show_command_center: true,
            command_matches: items,
            ..Default::default()
        }
    }

    #[test]
    fn inactive_is_not_handled() {
        let mut state = AppState::default();
        assert_eq!(
            handle_key(&mut state, key(KeyCode::F(12)), None, false),
            KeyOutcome::NotHandled
        );
    }

    #[test]
    fn escape_performs_exact_cleanup() {
        let mut state = active_state(vec![item(
            CommandTarget::ShellCommand("demo".into()),
            ActionAvailability::Available,
        )]);
        state.filter = "needle".into();
        state.overlay_list_state.select(Some(4));

        assert_eq!(
            handle_key(&mut state, key(KeyCode::Esc), None, false),
            KeyOutcome::Consumed
        );
        assert!(!state.show_command_center);
        assert!(state.filter.is_empty());
        assert!(state.command_matches.is_empty());
        assert_eq!(state.overlay_list_state, ListState::default());
    }

    #[test]
    fn up_and_down_clamp_selection() {
        let available = ActionAvailability::Available;
        let mut state = active_state(vec![
            item(CommandTarget::ShellCommand("one".into()), available.clone()),
            item(CommandTarget::ShellCommand("two".into()), available),
        ]);
        state.overlay_list_state.select(Some(0));

        handle_key(&mut state, key(KeyCode::Up), None, false);
        assert_eq!(state.overlay_list_state.selected(), Some(0));
        handle_key(&mut state, key(KeyCode::Down), None, false);
        handle_key(&mut state, key(KeyCode::Down), None, false);
        assert_eq!(state.overlay_list_state.selected(), Some(1));
    }

    #[test]
    fn backspace_rebuilds_and_selects_first_result() {
        let mut state = active_state(Vec::new());
        state.filter = "viewx".into();

        handle_key(
            &mut state,
            key(KeyCode::Backspace),
            Some(EntryKind::File),
            true,
        );

        assert_eq!(state.filter, "view");
        assert!(
            state
                .command_matches
                .iter()
                .any(|item| { item.target == CommandTarget::Action(Action::ViewFile) })
        );
        assert_eq!(state.overlay_list_state.selected(), Some(0));
    }

    #[test]
    fn character_rebuilds_and_selects_first_result() {
        let mut state = active_state(Vec::new());

        handle_key(
            &mut state,
            key(KeyCode::Char('v')),
            Some(EntryKind::File),
            true,
        );

        assert_eq!(state.filter, "v");
        assert!(!state.command_matches.is_empty());
        assert_eq!(state.overlay_list_state.selected(), Some(0));
    }

    #[test]
    fn disabled_enter_reports_reason_and_stays_open() {
        let target = CommandTarget::Action(Action::EditFile);
        let mut state = active_state(vec![item(
            target,
            ActionAvailability::Disabled {
                reason: "editor unavailable".into(),
            },
        )]);

        assert_eq!(
            handle_key(&mut state, key(KeyCode::Enter), None, false),
            KeyOutcome::Consumed
        );
        assert_eq!(state.message.as_deref(), Some("editor unavailable"));
        assert!(state.show_command_center);
        assert_eq!(state.command_matches.len(), 1);
    }

    #[test]
    fn available_enter_executes_exact_target_and_closes() {
        let target = CommandTarget::ShellCommand("printf demo".into());
        let mut state = active_state(vec![item(target.clone(), ActionAvailability::Available)]);
        state.filter = "demo".into();
        state.overlay_list_state.select(Some(9));

        assert_eq!(
            handle_key(&mut state, key(KeyCode::Enter), None, false),
            KeyOutcome::Execute(target)
        );
        assert!(!state.show_command_center);
        assert!(state.filter.is_empty());
        assert!(state.command_matches.is_empty());
        assert_eq!(state.overlay_list_state.selected(), Some(9));
    }

    #[test]
    fn unrecognized_key_is_consumed_while_active() {
        let mut state = active_state(Vec::new());
        assert_eq!(
            handle_key(&mut state, key(KeyCode::F(12)), None, false),
            KeyOutcome::Consumed
        );
    }

    #[test]
    fn open_uses_focused_kind_and_editor_availability() {
        let mut state = AppState::default();
        open(&mut state, Some(EntryKind::File), true);
        let edit = state
            .command_matches
            .iter()
            .find(|item| item.target == CommandTarget::Action(Action::EditFile))
            .unwrap();
        assert!(edit.availability.is_available());
        assert!(state.show_command_center);
        assert_eq!(state.overlay_list_state.selected(), Some(0));

        open(&mut state, Some(EntryKind::Directory), false);
        let edit = state
            .command_matches
            .iter()
            .find(|item| item.target == CommandTarget::Action(Action::EditFile))
            .unwrap();
        assert!(!edit.availability.is_available());
    }

    #[test]
    fn render_shows_disabled_reason() {
        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = AppState {
            filter: "needle".into(),
            command_matches: vec![item(
                CommandTarget::ShellCommand("demo".into()),
                ActionAvailability::Disabled {
                    reason: "char reason".into(),
                },
            )],
            ..Default::default()
        };

        terminal
            .draw(|frame| render(frame, frame.area(), &state))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(text.contains("ARX Command Center"));
        assert!(text.contains(":needle_"));
        assert!(text.contains("[COMMAND] Blocked Demo"));
        assert!(text.contains("example subtitle"));
        assert!(text.contains("unavailable: char reason"));
    }
}
