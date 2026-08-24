use super::*;
use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Alignment;

pub(super) fn handle_key(state: &mut AppState, key: KeyEvent, key_router: &mut KeyRouter) -> bool {
    if state.input_context() != InputContext::Help {
        return false;
    }

    match key.code {
        KeyCode::Esc | KeyCode::F(1) | KeyCode::Char('?') | KeyCode::Char('q') => {
            key_router.clear_pending();
            state.show_help = false;
            state.help_scroll = 0;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            state.help_scroll = state.help_scroll.saturating_add(1);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            state.help_scroll = state.help_scroll.saturating_sub(1);
        }
        KeyCode::PageDown => {
            state.help_scroll = state.help_scroll.saturating_add(20);
        }
        KeyCode::PageUp => {
            state.help_scroll = state.help_scroll.saturating_sub(20);
        }
        KeyCode::Home => state.help_scroll = 0,
        KeyCode::End => state.help_scroll = usize::MAX,
        _ => return false,
    }

    true
}

pub(super) fn render(
    frame: &mut Frame,
    area: Rect,
    state: &mut AppState,
    keymap: &arx::input::Keymap,
) {
    let popup_area = centered_rect(68, 90, area);
    frame.render_widget(Clear, popup_area);

    // Full help content (Getting Started first, then full reference).
    // #214: physical keys for MANAGED actions come from the effective Keymap.
    let full_lines = help_full_lines(Some(keymap));

    // Compute visible slice
    let content_height = popup_area.height.saturating_sub(2) as usize; // minus borders
    let total = full_lines.len();
    let scroll = state.help_scroll.min(total.saturating_sub(content_height));
    state.help_scroll = scroll;
    let visible: Vec<Line> = full_lines
        .iter()
        .skip(scroll)
        .take(content_height)
        .cloned()
        .collect();

    // Scroll hint
    let scroll_hint = if total > content_height {
        format!(
            " {}/{} | j/k/↑↓:scroll PgUp/PgDn:page Home/End:jump q/Esc/F1:close ",
            scroll + visible.len().min(total - scroll),
            total
        )
    } else {
        " q/Esc/F1/?:close ".into()
    };

    let help = Paragraph::new(visible)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Help ")
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .style(Style::default().fg(Color::White))
        .wrap(Wrap { trim: false });
    frame.render_widget(help, popup_area);

    // Scroll hint at bottom
    let hint_area = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(popup_area);
    let hint = Paragraph::new(Line::from(scroll_hint))
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);
    frame.render_widget(hint, hint_area[1]);
}

/// Managed-action key label from the effective Keymap, or "—" when unbound.
fn dyn_key(keymap: Option<&arx::input::Keymap>, action_id: arx::app::ActionId) -> String {
    match keymap.and_then(|km| km.primary_binding_label(arx::app::InputContext::Browser, action_id))
    {
        Some(label) => format!("{:<12}", label),
        None => format!("{:<12}", "—"),
    }
}

fn help_full_lines(keymap: Option<&arx::input::Keymap>) -> Vec<Line<'static>> {
    use arx::app::ActionId;
    let k = |id: ActionId| dyn_key(keymap, id);
    let _ = &k;
    vec![
        // Getting Started (short, first)
        Line::from(Span::styled(
            "ARX Help — Getting Started",
            Style::default().fg(Color::Yellow),
        )),
        Line::from(Span::styled(
            "F1/? or Esc to close  |  j/k/↑↓/PgUp/PgDn/Home/End to scroll",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Core Workflow",
            Style::default().fg(Color::Cyan),
        )),
        Line::from("  Tab           Switch pane (left ↔ right)"),
        Line::from(format!(
            "  {}Hosts / SFTP (connect remote)",
            k(ActionId::OpenHosts)
        )),
        Line::from(format!(
            "  {}Compare workspace (diff)",
            k(ActionId::ToggleWorkspaceComparison)
        )),
        Line::from(format!(
            "  {}Sync Preview (after compare)",
            k(ActionId::PreviewWorkspaceSync)
        )),
        Line::from("  Enter         Execute preview"),
        Line::from(format!(
            "  {}Command Center",
            k(ActionId::OpenCommandCenter)
        )),
        Line::from("  F1/?          This help"),
        Line::from(format!("  {} / q       Quit", k(ActionId::Quit).trim_end())),
        Line::from(""),
        Line::from(Span::styled("Navigation", Style::default().fg(Color::Cyan))),
        Line::from("  j / ↓ / k / ↑      Move cursor"),
        Line::from("  Enter              Enter directory / content diff"),
        Line::from("  Backspace          Parent directory"),
        Line::from("  Ctrl+G             Go to path"),
        Line::from("  Ctrl+U             Swap panes"),
        Line::from(format!(
            "  {}Storage Inspector (local, read-only)",
            k(ActionId::OpenStorageInspector)
        )),
        Line::from("  Alt+D              Filesystems (df++, read-only)"),
        Line::from("  Alt+O              Sync other pane to active"),
        Line::from("  Alt+Down           Go back in directory history"),
        Line::from("  Alt+/              Recursive file search (find)"),
        Line::from("  Ctrl+\\             Toggle split pane"),
        Line::from("  Ctrl+P             Command Center (incl. Open in file manager)"),
        Line::from(""),
        Line::from(Span::styled("Tabs", Style::default().fg(Color::Cyan))),
        Line::from("  Ctrl+T         New tab"),
        Line::from("  Ctrl+W         Close tab"),
        Line::from("  Ctrl+←/→       Previous / next tab"),
        Line::from("  Alt+1 … 9      Switch to tab N"),
        Line::from(""),
        Line::from(Span::styled(
            "Selection & Filter",
            Style::default().fg(Color::Cyan),
        )),
        Line::from("  Space          Toggle selection"),
        Line::from("  *              Invert selection"),
        Line::from("  +              Select by glob pattern"),
        Line::from("  /              Quick filter by name"),
        Line::from("  Ctrl+H         Toggle hidden (dot) files"),
        Line::from("  Alt+T          Toggle panel mode (Full/Brief)"),
        Line::from(""),
        Line::from(Span::styled(
            "File Operations",
            Style::default().fg(Color::Cyan),
        )),
        Line::from(format!("  {}Copy to other pane", k(ActionId::Copy))),
        Line::from(format!("  {}Move to other pane", k(ActionId::Move))),
        Line::from(format!("  {}Create directory", k(ActionId::Mkdir))),
        Line::from(format!("  {}Delete", k(ActionId::Delete))),
        Line::from("  Shift+F6       Rename file"),
        Line::from("  Ctrl+I         File info (stat)"),
        Line::from("  Ctrl+Space     Directory size / free space"),
        Line::from(""),
        Line::from(Span::styled(
            "View & Edit",
            Style::default().fg(Color::Cyan),
        )),
        Line::from(format!("  {}View file (built-in)", k(ActionId::ViewFile))),
        Line::from("  Shift+F3       View with bat (syntax highlight)"),
        Line::from(format!(
            "  {}Edit (configurable editor)",
            k(ActionId::EditFile)
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Ctrl+X Prefix (MC-style)",
            Style::default().fg(Color::Cyan),
        )),
        Line::from("  Ctrl+X S       Symlink (ln -s)"),
        Line::from("  Ctrl+X L       Hard link (ln)"),
        Line::from("  Ctrl+X C       chmod"),
        Line::from("  Ctrl+X O       chown"),
        Line::from(""),
        Line::from(Span::styled(
            "Panels & Tools",
            Style::default().fg(Color::Cyan),
        )),
        Line::from("  F2             User menu (arx.menu)"),
        Line::from("  Ctrl+B         Bookmarks"),
        Line::from("  Ctrl+J         Background jobs"),
        Line::from("  Ctrl+O         Shell (drop to subshell)"),
        Line::from("  :              Command line"),
        Line::from(""),
        Line::from(Span::styled("Other", Style::default().fg(Color::Cyan))),
        Line::from("  Ctrl+R         Refresh"),
        Line::from("  q              Quit"),
        Line::from("  F1 / ?         This help"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn help_state(scroll: usize) -> AppState {
        AppState {
            show_help: true,
            help_scroll: scroll,
            ..Default::default()
        }
    }

    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol().to_owned())
            .collect::<Vec<_>>()
            .join("")
    }

    fn source_text() -> String {
        help_full_lines(None)
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn inactive_returns_false_without_mutation() {
        let mut state = AppState {
            help_scroll: 7,
            ..Default::default()
        };
        let mut router = KeyRouter::default();
        router.resolve(
            InputContext::Browser,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
        );
        let pending = router.pending().to_vec();

        assert!(!handle_key(&mut state, key(KeyCode::Down), &mut router));
        assert!(!state.show_help);
        assert_eq!(state.help_scroll, 7);
        assert_eq!(router.pending(), pending);
    }

    #[test]
    fn close_keys_reset_help_and_pending_chord() {
        for code in [
            KeyCode::Esc,
            KeyCode::F(1),
            KeyCode::Char('?'),
            KeyCode::Char('q'),
        ] {
            let mut state = help_state(9);
            let mut router = KeyRouter::default();
            router.resolve(
                InputContext::Browser,
                KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
            );
            assert!(!router.pending().is_empty());

            assert!(handle_key(&mut state, key(code), &mut router));
            assert!(!state.show_help);
            assert_eq!(state.help_scroll, 0);
            assert!(router.pending().is_empty());
        }
    }

    #[test]
    fn down_and_j_increment_while_up_and_k_saturate() {
        let mut state = help_state(0);
        let mut router = KeyRouter::default();

        assert!(handle_key(&mut state, key(KeyCode::Down), &mut router));
        assert!(handle_key(&mut state, key(KeyCode::Char('j')), &mut router));
        assert_eq!(state.help_scroll, 2);

        state.help_scroll = 0;
        assert!(handle_key(&mut state, key(KeyCode::Up), &mut router));
        assert!(handle_key(&mut state, key(KeyCode::Char('k')), &mut router));
        assert_eq!(state.help_scroll, 0);
    }

    #[test]
    fn page_and_jump_keys_update_scroll() {
        let mut state = help_state(5);
        let mut router = KeyRouter::default();

        assert!(handle_key(&mut state, key(KeyCode::PageDown), &mut router));
        assert_eq!(state.help_scroll, 25);
        assert!(handle_key(&mut state, key(KeyCode::PageUp), &mut router));
        assert_eq!(state.help_scroll, 5);
        state.help_scroll = 3;
        assert!(handle_key(&mut state, key(KeyCode::PageUp), &mut router));
        assert_eq!(state.help_scroll, 0);
        assert!(handle_key(&mut state, key(KeyCode::End), &mut router));
        assert_eq!(state.help_scroll, usize::MAX);
        assert!(handle_key(&mut state, key(KeyCode::Home), &mut router));
        assert_eq!(state.help_scroll, 0);
    }

    #[test]
    fn unhandled_key_returns_false() {
        let mut state = help_state(4);
        let mut router = KeyRouter::default();

        assert!(!handle_key(
            &mut state,
            key(KeyCode::Char('z')),
            &mut router
        ));
        assert!(state.show_help);
        assert_eq!(state.help_scroll, 4);
    }

    #[test]
    fn render_clamps_scroll_and_contains_current_shortcuts() {
        let backend = TestBackend::new(120, 80);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = help_state(usize::MAX);

        terminal
            .draw(|frame| {
                render(
                    frame,
                    frame.area(),
                    &mut state,
                    &arx::input::Keymap::default(),
                )
            })
            .unwrap();
        assert_ne!(state.help_scroll, usize::MAX);
        let bottom = buffer_text(&terminal);
        assert!(bottom.contains("Help"));
        assert!(bottom.contains("q/Esc/F1:close"));

        state.help_scroll = 0;
        terminal
            .draw(|frame| {
                render(
                    frame,
                    frame.area(),
                    &mut state,
                    &arx::input::Keymap::default(),
                )
            })
            .unwrap();
        assert_eq!(state.help_scroll, 0);
        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("ARX Help"));
        assert!(rendered.contains("Toggle split pane"));
        assert!(!rendered.contains("Ctrl+\\             Open in file manager"));

        let source = source_text();
        assert!(source.contains("Ctrl+\\             Toggle split pane"));
        assert!(source.contains("Command Center (incl. Open in file manager)"));
        assert!(!source.contains("Ctrl+\\             Open in file manager"));
    }
}
