use ratatui::Frame;

use super::*;

pub(super) fn render(frame: &mut Frame, area: Rect, state: &AppState, key_router: &KeyRouter) {
    // Which-Key is derived from the active KeyRouter prefix and the shared
    // Action Catalog. There is intentionally no second shortcut table here.
    let input_context = state.input_context();
    let continuations = key_router.continuations(input_context);
    if continuations.is_empty() {
        return;
    }

    let prefix = key_router
        .pending()
        .iter()
        .map(|stroke| stroke.label())
        .collect::<Vec<_>>()
        .join(" ");

    let items: Vec<ListItem> = continuations
        .iter()
        .filter_map(|continuation| {
            action_meta(continuation.action).map(|meta| {
                ListItem::new(format!(
                    "{:<10} {}",
                    continuation.stroke.label(),
                    meta.label
                ))
            })
        })
        .collect();

    if items.is_empty() {
        return;
    }

    let height = (items.len() as u16 + 2).min(area.height.max(1));
    let width = area.width.saturating_mul(70).saturating_div(100).max(30);
    let width = width.min(area.width);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area
        .y
        .saturating_add(area.height.saturating_sub(height).saturating_sub(1));
    let popup = Rect::new(x, y, width, height);

    frame.render_widget(Clear, popup);
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {prefix} … "))
                .border_style(Style::default().fg(Color::Cyan)),
        ),
        popup,
    );
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyEvent, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;

    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    fn ctrl_x_router() -> KeyRouter {
        let mut router = KeyRouter::default();
        assert_eq!(
            router.resolve(
                InputContext::Browser,
                KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
            ),
            KeyResolution::Pending
        );
        router
    }

    #[test]
    fn no_pending_prefix_renders_no_overlay_or_items() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = AppState::default();
        let router = KeyRouter::default();

        assert!(router.continuations(state.input_context()).is_empty());
        terminal
            .draw(|frame| render(frame, frame.area(), &state, &router))
            .unwrap();
        assert!(buffer_text(&terminal).trim().is_empty());
    }

    #[test]
    fn ctrl_x_pending_derives_continuations_from_key_router() {
        let state = AppState::default();
        let router = ctrl_x_router();
        let continuations = router.continuations(state.input_context());

        assert!(!continuations.is_empty());
        assert!(continuations.iter().all(|continuation| {
            router.keymap().bindings().iter().any(|binding| {
                binding.action.id() == continuation.action
                    && binding.sequence.starts_with(router.pending())
                    && binding.sequence.get(router.pending().len()) == Some(&continuation.stroke)
            })
        }));
    }

    #[test]
    fn rendered_labels_come_from_action_catalog() {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = AppState::default();
        let router = ctrl_x_router();
        let continuations = router.continuations(state.input_context());

        terminal
            .draw(|frame| render(frame, frame.area(), &state, &router))
            .unwrap();
        let text = buffer_text(&terminal);
        for continuation in continuations {
            let meta = action_meta(continuation.action).unwrap();
            assert!(text.contains(meta.label));
            assert!(text.contains(&continuation.stroke.label()));
        }
    }

    #[test]
    fn module_has_no_second_static_shortcut_table() {
        let source = include_str!("which_key.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        assert!(!production.contains("KeyBinding::"));
        assert!(!production.contains("Keymap::"));
        assert!(!production.contains("ACTION_CATALOG"));
        assert!(production.contains("key_router.continuations(input_context)"));
        assert!(production.contains("action_meta(continuation.action)"));
    }

    #[test]
    fn parent_pending_resolution_consumes_before_legacy_matcher() {
        let source = include_str!("input_dispatch.rs");
        let pending = source
            .find("KeyResolution::Pending =>")
            .expect("parent Pending arm");
        let consumed = source[pending..]
            .find("flow: InputFlow::ContinueLoop")
            .map(|offset| pending + offset)
            .expect("Pending is consumed");
        let legacy = source
            .find("let pane = state.active_pane_mut();")
            .expect("legacy matcher setup");

        assert!(pending < consumed && consumed < legacy);
    }
}
