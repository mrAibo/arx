use super::*;

#[derive(Debug)]
struct PositionedChip {
    text: String,
    rect: Rect,
    action: Action,
    available: bool,
}

// --- Commander core action helpers (ponytail: fixed match covers the hitbox set) ---

fn action_id_to_action(id: ActionId) -> Option<Action> {
    Some(match id {
        ActionId::ViewFile => Action::ViewFile,
        ActionId::EditFile => Action::EditFile,
        ActionId::Copy => Action::Copy,
        ActionId::Move => Action::Move,
        ActionId::Mkdir => Action::Mkdir,
        ActionId::Delete => Action::Delete,
        ActionId::OpenHosts => Action::OpenHosts,
        ActionId::OpenCommandCenter => Action::OpenCommandCenter,
        ActionId::ToggleWorkspaceComparison => Action::ToggleWorkspaceComparison,
        ActionId::PreviewWorkspaceSync => Action::PreviewWorkspaceSync,
        ActionId::ToggleEmbeddedTerminal => Action::ToggleEmbeddedTerminal,
        ActionId::OpenHelp => Action::OpenHelp,
        ActionId::Quit => Action::Quit,
        _ => return None,
    })
}

pub(super) fn action_to_id(a: Action) -> ActionId {
    match a {
        Action::ViewFile => ActionId::ViewFile,
        Action::EditFile => ActionId::EditFile,
        Action::Copy => ActionId::Copy,
        Action::Move => ActionId::Move,
        Action::Mkdir => ActionId::Mkdir,
        Action::Delete => ActionId::Delete,
        Action::OpenHosts => ActionId::OpenHosts,
        Action::OpenCommandCenter => ActionId::OpenCommandCenter,
        Action::ToggleWorkspaceComparison => ActionId::ToggleWorkspaceComparison,
        Action::PreviewWorkspaceSync => ActionId::PreviewWorkspaceSync,
        Action::ToggleEmbeddedTerminal => ActionId::ToggleEmbeddedTerminal,
        Action::OpenHelp => ActionId::OpenHelp,
        Action::Quit => ActionId::Quit,
        _ => unreachable!("only commander core actions reach hitboxes"),
    }
}

fn compact_action_label(action: ActionId) -> &'static str {
    match action {
        ActionId::ViewFile => "View",
        ActionId::EditFile => "Edit",
        ActionId::Copy => "Copy",
        ActionId::Move => "Move",
        ActionId::Mkdir => "MkDir",
        ActionId::Delete => "Del",
        ActionId::OpenHosts => "Hosts",
        ActionId::OpenCommandCenter => "Cmd",
        ActionId::ToggleWorkspaceComparison => "Diff",
        ActionId::PreviewWorkspaceSync => "Sync",
        ActionId::ToggleEmbeddedTerminal => "Term",
        ActionId::OpenHelp => "Help",
        ActionId::Quit => "Quit",
        _ => "",
    }
}

/// Format one command-bar row from hints, respecting width.
#[cfg(test)]
fn format_command_row(hints: &[ContextHint], width: u16) -> String {
    let mut text = String::new();
    for hint in hints {
        let item = format!("{} {}", hint.binding, hint.label);
        let candidate = if text.is_empty() {
            item
        } else {
            format!("{text}    {item}")
        };
        if Line::from(candidate.as_str()).width() > usize::from(width) {
            break;
        }
        text = candidate;
    }
    text
}

fn row_a_chips(area: Rect, hints: &[ContextHint]) -> Vec<PositionedChip> {
    let compact = area.width < 90;
    let mut chips = Vec::new();
    let mut cursor = area.x;

    for (index, hint) in hints.iter().enumerate() {
        let chip_x = if index == 0 { cursor } else { cursor + 2 };
        let label = if compact {
            compact_action_label(hint.action)
        } else {
            hint.label
        };
        let text = format!("{} {}", hint.binding, label);
        let width = Line::from(text.as_str()).width() as u16;
        if let Some(action) = action_id_to_action(hint.action) {
            chips.push(PositionedChip {
                text,
                rect: Rect::new(chip_x, area.y, width, 1),
                action,
                available: hint.available,
            });
        }
        cursor = chip_x + width;
    }

    chips
}

fn row_b_chips(area: Rect, hints: &[ContextHint]) -> Vec<PositionedChip> {
    let mut chips = Vec::new();
    let mut cursor = area.x;
    let spacing = 3u16;

    for hint in hints {
        let text = format!("{} {}", hint.binding, hint.label);
        let width = Line::from(text.as_str()).width() as u16;
        let chip_x = if chips.is_empty() {
            cursor
        } else {
            cursor + spacing
        };

        if chip_x + width > area.x + area.width {
            break;
        }

        if let Some(action) = action_id_to_action(hint.action) {
            chips.push(PositionedChip {
                text,
                rect: Rect::new(chip_x, area.y, width, 1),
                action,
                available: hint.available,
            });
        }
        cursor = chip_x + width;
    }

    chips
}

fn chip_style(available: bool) -> Style {
    if available {
        Style::default().fg(Color::Black).bg(Color::DarkGray)
    } else {
        Style::default()
            .fg(Color::Gray)
            .bg(Color::DarkGray)
            .add_modifier(Modifier::DIM)
    }
}

fn render_row_a(frame: &mut Frame, area: Rect, chips: &[PositionedChip]) {
    let mut spans = Vec::new();
    let mut render_cursor = area.x;
    for chip in chips {
        if chip.rect.x > render_cursor {
            spans.push(Span::raw(
                " ".repeat(usize::from(chip.rect.x - render_cursor)),
            ));
        }
        spans.push(Span::styled(chip.text.clone(), chip_style(chip.available)));
        render_cursor = chip.rect.x + chip.rect.width;
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_row_b(frame: &mut Frame, area: Rect, chips: &[PositionedChip]) {
    let background = Style::default().fg(Color::Black).bg(Color::DarkGray);
    let mut spans = Vec::new();
    let mut render_cursor = area.x;
    for chip in chips {
        if chip.rect.x > render_cursor {
            spans.push(Span::styled(
                " ".repeat(usize::from(chip.rect.x - render_cursor)),
                background,
            ));
        }
        spans.push(Span::styled(chip.text.clone(), chip_style(chip.available)));
        render_cursor = chip.rect.x + chip.rect.width;
    }
    frame.render_widget(Paragraph::new(Line::from(spans)).style(background), area);
}

pub(super) fn render(
    frame: &mut Frame,
    row_a_area: Rect,
    row_b_area: Rect,
    hitboxes: &mut Vec<arx::app::CommandHitbox>,
    row_a: &[ContextHint],
    row_b: &[ContextHint],
) {
    hitboxes.clear();

    if !row_a.is_empty() && row_a_area.width > 0 {
        let chips = row_a_chips(row_a_area, row_a);
        hitboxes.extend(chips.iter().map(|chip| arx::app::CommandHitbox {
            rect: chip.rect,
            action: chip.action,
            available: chip.available,
        }));
        render_row_a(frame, row_a_area, &chips);
    }

    if !row_b.is_empty() && row_b_area.width > 0 {
        let chips = row_b_chips(row_b_area, row_b);
        hitboxes.extend(chips.iter().map(|chip| arx::app::CommandHitbox {
            rect: chip.rect,
            action: chip.action,
            available: chip.available,
        }));
        render_row_b(frame, row_b_area, &chips);
    }
}

/// Test-only wrapper for the legacy contextual footer text contract.
#[cfg(test)]
fn command_bar_text_wrapper(
    state: &AppState,
    key_router: &KeyRouter,
    focused_kind: Option<EntryKind>,
    editor_available: bool,
    width: u16,
) -> Option<String> {
    if !key_router.pending().is_empty() || width == 0 {
        return None;
    }
    let (row_a, row_b) =
        command_bar_rows(state, key_router.keymap(), focused_kind, editor_available);
    let mut text = format_command_row(&row_a, width);
    if text.is_empty() {
        text = format_command_row(&row_b, width);
    }
    if text.is_empty() {
        let hints = contextual_hints_with_file_context(
            state,
            key_router.keymap(),
            focused_kind,
            editor_available,
        );
        text = format_command_row(&hints, width);
    }
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arx::input::{HintPriority, KeyBinding, KeyStroke, Keymap};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn hint(action: ActionId, binding: &str, label: &'static str, available: bool) -> ContextHint {
        ContextHint {
            action,
            binding: binding.into(),
            label,
            priority: HintPriority::Discovery,
            available,
        }
    }

    fn file(name: &str) -> Entry {
        Entry {
            name: name.into(),
            kind: EntryKind::File,
            size: Some(1),
            modified_unix_ms: None,
        }
    }

    fn rendered_line(width: u16, row_a: &[ContextHint], row_b: &[ContextHint]) -> String {
        let backend = TestBackend::new(width, 2);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hitboxes = Vec::new();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    Rect::new(0, 0, width, 1),
                    Rect::new(0, 1, width, 1),
                    &mut hitboxes,
                    row_a,
                    row_b,
                );
            })
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .take(usize::from(width))
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn command_bar_wide_and_compact_row_a_labels() {
        let row = [hint(ActionId::Mkdir, "F7", "New directory", true)];
        assert!(rendered_line(100, &row, &[]).starts_with("F7 New directory"));
        assert!(rendered_line(89, &row, &[]).starts_with("F7 MkDir"));
    }

    #[test]
    fn command_bar_row_b_stops_at_first_overflow_and_keeps_spacing() {
        let row = [
            hint(ActionId::OpenCommandCenter, "Ctrl+P", "Commands", true),
            hint(ActionId::OpenHelp, "F1", "Help", true),
            hint(ActionId::Quit, "F10", "Quit", true),
        ];
        let first_width = Line::from("Ctrl+P Commands").width() as u16;
        let second_width = Line::from("F1 Help").width() as u16;
        let chips = row_b_chips(Rect::new(5, 1, first_width + 3 + second_width - 1, 1), &row);
        assert_eq!(chips.len(), 1);

        let chips = row_b_chips(Rect::new(5, 1, 80, 1), &row);
        assert_eq!(chips[1].rect.x - (chips[0].rect.x + chips[0].rect.width), 3);
    }

    #[test]
    fn command_bar_hitboxes_use_rendered_chip_geometry_and_keep_disabled_chip() {
        let row = [
            hint(ActionId::OpenCommandCenter, "Ctrl+P", "Commands", true),
            hint(ActionId::OpenHelp, "F1", "Help", false),
        ];
        let backend = TestBackend::new(60, 2);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hitboxes = Vec::new();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    Rect::new(0, 0, 60, 1),
                    Rect::new(4, 1, 50, 1),
                    &mut hitboxes,
                    &[],
                    &row,
                );
            })
            .unwrap();

        assert_eq!(hitboxes.len(), 2);
        assert!(!hitboxes[1].available);
        let buffer = terminal.backend().buffer();
        for (hitbox, expected) in hitboxes.iter().zip(["Ctrl+P Commands", "F1 Help"]) {
            let rendered: String = (hitbox.rect.x..hitbox.rect.x + hitbox.rect.width)
                .map(|x| buffer[(x, hitbox.rect.y)].symbol())
                .collect();
            assert_eq!(hitbox.rect.width, Line::from(expected).width() as u16);
            assert_eq!(rendered, expected);
        }
    }

    #[test]
    fn command_bar_uses_remapped_bindings_and_pending_prefix_hides_it() {
        let keymap = Keymap::new(vec![
            KeyBinding::new(
                InputContext::Browser,
                vec![KeyStroke::new(KeyCode::F(12), KeyModifiers::NONE)],
                Action::ViewFile,
            ),
            KeyBinding::new(
                InputContext::Browser,
                vec![KeyStroke::new(KeyCode::F(11), KeyModifiers::NONE)],
                Action::EditFile,
            ),
            KeyBinding::new(
                InputContext::Browser,
                vec![
                    KeyStroke::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
                    KeyStroke::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                ],
                Action::BeginChmod,
            ),
        ]);
        let mut router = KeyRouter::new(keymap);
        assert_eq!(
            command_bar_text_wrapper(
                &AppState::default(),
                &router,
                Some(EntryKind::File),
                true,
                u16::MAX,
            )
            .as_deref(),
            Some("F12 View file    F11 Edit file")
        );
        assert_eq!(
            router.resolve_stroke(
                InputContext::Browser,
                KeyStroke::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
            ),
            KeyResolution::Pending
        );
        assert!(
            command_bar_text_wrapper(
                &AppState::default(),
                &router,
                Some(EntryKind::File),
                true,
                u16::MAX,
            )
            .is_none()
        );
    }

    #[test]
    fn command_bar_fits_priority_prefix_to_real_width() {
        let state = AppState::default();
        let router = KeyRouter::default();
        let wide = command_bar_text_wrapper(&state, &router, Some(EntryKind::File), true, u16::MAX)
            .unwrap();
        assert_eq!(wide.split("    ").count(), 7);
        assert!(wide.contains("F3 View file"));
        assert!(wide.contains("F4 Edit file"));
        assert!(wide.contains("F5 Copy"));
        assert!(wide.contains("F6 Move"));
        assert!(wide.contains("F7 New directory"));
        assert!(wide.contains("F8 Delete"));
        assert!(wide.contains("F9 Hosts"));

        let first = wide.split("    ").next().unwrap();
        let first_width = u16::try_from(Line::from(first).width()).unwrap();
        assert_eq!(
            command_bar_text_wrapper(&state, &router, Some(EntryKind::File), true, first_width)
                .as_deref(),
            Some(first)
        );
        assert!(
            command_bar_text_wrapper(
                &state,
                &router,
                Some(EntryKind::File),
                true,
                first_width - 1,
            )
            .is_none()
        );

        let dir_text =
            command_bar_text_wrapper(&state, &router, Some(EntryKind::Directory), true, u16::MAX)
                .unwrap();
        assert!(dir_text.contains("F3 View file"));
    }

    #[test]
    fn command_bar_derives_file_action_from_keymap_not_hardcoded() {
        let state = AppState::default();
        let base = Keymap::default();
        let mut bindings: Vec<_> = base
            .bindings()
            .iter()
            .filter(|binding| {
                !(binding.context == InputContext::Browser
                    && binding.action == Action::Copy
                    && binding.sequence.len() == 1
                    && matches!(binding.sequence[0].code, KeyCode::F(5)))
            })
            .cloned()
            .collect();
        bindings.push(KeyBinding::new(
            InputContext::Browser,
            vec![KeyStroke::new(KeyCode::F(10), KeyModifiers::NONE)],
            Action::Copy,
        ));
        let router = KeyRouter::new(Keymap::new(bindings));
        let wide = command_bar_text_wrapper(&state, &router, Some(EntryKind::File), true, u16::MAX)
            .unwrap();
        assert!(wide.contains("F10 Copy"));
        assert!(!wide.contains("F5 Copy"));
    }

    #[test]
    fn command_bar_pending_chord_leaves_discovery_to_which_key() {
        let mut router = KeyRouter::default();
        let resolution = router.resolve_stroke(
            InputContext::Browser,
            KeyStroke::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
        );
        assert_eq!(resolution, KeyResolution::Pending);
        assert!(
            command_bar_text_wrapper(&AppState::default(), &router, None, false, u16::MAX)
                .is_none()
        );
    }

    #[test]
    fn command_bar_tracks_sync_preview_confirmation_and_running_contexts() {
        let left = Location::Local("/left".into());
        let right = Location::Local("/right".into());
        let router = KeyRouter::default();

        let mut preview = AppState::default();
        preview.remote_workspace.preview_open = true;
        preview.remote_workspace.refresh_visible(
            left.clone(),
            right.clone(),
            &[file("source.txt")],
            &[],
        );
        let preview_footer =
            command_bar_text_wrapper(&preview, &router, None, false, u16::MAX).unwrap_or_default();
        assert!(preview_footer.contains("Enter Execute workspace sync"));
        assert!(preview_footer.contains("D Reverse sync direction"));
        assert!(preview_footer.contains("M Toggle update/mirror"));

        let mut confirmation = AppState::default();
        confirmation.left.location = left.clone();
        confirmation.right.location = right.clone();
        confirmation.remote_workspace.preview_open = true;
        confirmation.remote_workspace.refresh_visible(
            left,
            right,
            &[],
            &[file("destination-only.txt")],
        );
        confirmation.remote_workspace.toggle_mode();
        let plan = confirmation.remote_workspace.plan.clone().unwrap();
        let diff = confirmation.remote_workspace.diff.clone().unwrap();
        let frozen = arx::workspace_sync_execution::SyncPlanValidator::freeze(
            &plan,
            &diff,
            &arx::vfs::default_registry(),
        )
        .unwrap();
        confirmation.remote_workspace.set_frozen_plan(frozen);
        let confirmation_footer =
            command_bar_text_wrapper(&confirmation, &router, None, false, u16::MAX)
                .unwrap_or_default();
        assert!(confirmation_footer.contains("Enter Confirm workspace sync"));
        assert!(confirmation_footer.contains("Esc Back in workspace sync"));

        let mut running = AppState::default();
        running.remote_workspace.preview_open = true;
        running.remote_workspace.ux = WorkspaceSyncUxState::Running {
            job_id: "sync-1".into(),
        };
        let running_footer =
            command_bar_text_wrapper(&running, &router, None, false, u16::MAX).unwrap_or_default();
        assert!(running_footer.contains("C Cancel workspace sync"));
        assert!(running_footer.contains("Esc Hide workspace sync"));
    }

    #[test]
    fn command_bar_has_no_second_shortcut_table() {
        let production = include_str!("command_bar.rs")
            .split_once("#[cfg(test)]\nmod tests")
            .unwrap()
            .0;
        assert!(!production.contains("KeyCode::"));
        assert!(!production.contains("\"F3\""));
    }
}
