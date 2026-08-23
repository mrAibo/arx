use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MouseRoute {
    Ignore,
    CommandBar { action: Action, available: bool },
    ViewerScrollDown,
    ViewerScrollUp,
    ContextMenu { column: u16, row: u16 },
    DragSelect { pane: Pane, row: usize },
    ActivatePaneRow { pane: Pane, row: usize },
}

pub(super) fn classify(state: &AppState, mouse: MouseEvent) -> MouseRoute {
    if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
        for hitbox in &state.command_hitboxes {
            if mouse.column >= hitbox.rect.x
                && mouse.column < hitbox.rect.x + hitbox.rect.width
                && mouse.row >= hitbox.rect.y
                && mouse.row < hitbox.rect.y + hitbox.rect.height
            {
                return MouseRoute::CommandBar {
                    action: hitbox.action,
                    available: hitbox.available,
                };
            }
        }
    }

    let (area, pane) = if let Some(area) = state.left_area {
        if mouse.column >= area.x
            && mouse.column < area.x + area.width
            && mouse.row >= area.y
            && mouse.row < area.y + area.height
        {
            (area, Pane::Left)
        } else if let Some(area) = state.right_area {
            if mouse.column >= area.x
                && mouse.column < area.x + area.width
                && mouse.row >= area.y
                && mouse.row < area.y + area.height
            {
                (area, Pane::Right)
            } else {
                return MouseRoute::Ignore;
            }
        } else {
            return MouseRoute::Ignore;
        }
    } else {
        return MouseRoute::Ignore;
    };
    let row = mouse.row.saturating_sub(area.y + 1) as usize;

    match mouse.kind {
        MouseEventKind::ScrollDown if !state.viewer_content.is_empty() => {
            MouseRoute::ViewerScrollDown
        }
        MouseEventKind::ScrollUp if !state.viewer_content.is_empty() => MouseRoute::ViewerScrollUp,
        MouseEventKind::Down(MouseButton::Right) => MouseRoute::ContextMenu {
            column: mouse.column,
            row: mouse.row,
        },
        MouseEventKind::Drag(MouseButton::Left) => MouseRoute::DragSelect { pane, row },
        MouseEventKind::Down(_) => MouseRoute::ActivatePaneRow { pane, row },
        _ => MouseRoute::Ignore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> AppState {
        AppState {
            left_area: Some(Rect::new(10, 5, 20, 10)),
            right_area: Some(Rect::new(30, 5, 20, 10)),
            ..AppState::default()
        }
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn command_bar_hitbox_beats_pane_hit() {
        let mut state = state();
        state.command_hitboxes.push(arx::app::CommandHitbox {
            rect: Rect::new(12, 6, 4, 1),
            action: Action::Quit,
            available: true,
        });

        assert_eq!(
            classify(
                &state,
                mouse(MouseEventKind::Down(MouseButton::Left), 12, 6)
            ),
            MouseRoute::CommandBar {
                action: Action::Quit,
                available: true,
            }
        );
    }

    #[test]
    fn disabled_hitbox_is_still_a_command_bar_route() {
        let mut state = state();
        state.command_hitboxes.push(arx::app::CommandHitbox {
            rect: Rect::new(12, 6, 4, 1),
            action: Action::Quit,
            available: false,
        });

        assert_eq!(
            classify(
                &state,
                mouse(MouseEventKind::Down(MouseButton::Left), 12, 6)
            ),
            MouseRoute::CommandBar {
                action: Action::Quit,
                available: false,
            }
        );
    }

    #[test]
    fn outside_panes_is_ignored() {
        assert_eq!(
            classify(
                &state(),
                mouse(MouseEventKind::Down(MouseButton::Left), 1, 1)
            ),
            MouseRoute::Ignore
        );
    }

    #[test]
    fn left_and_right_panes_are_resolved() {
        let state = state();
        assert_eq!(
            classify(
                &state,
                mouse(MouseEventKind::Down(MouseButton::Left), 11, 7)
            ),
            MouseRoute::ActivatePaneRow {
                pane: Pane::Left,
                row: 1,
            }
        );
        assert_eq!(
            classify(
                &state,
                mouse(MouseEventKind::Down(MouseButton::Left), 31, 7)
            ),
            MouseRoute::ActivatePaneRow {
                pane: Pane::Right,
                row: 1,
            }
        );
    }

    #[test]
    fn row_calculation_saturates_above_content_start() {
        assert_eq!(
            classify(
                &state(),
                mouse(MouseEventKind::Down(MouseButton::Left), 11, 5)
            ),
            MouseRoute::ActivatePaneRow {
                pane: Pane::Left,
                row: 0,
            }
        );
    }

    #[test]
    fn viewer_wheel_inside_pane_is_classified() {
        let mut state = state();
        state.viewer_content.push("line".into());

        assert_eq!(
            classify(&state, mouse(MouseEventKind::ScrollDown, 11, 7)),
            MouseRoute::ViewerScrollDown
        );
        assert_eq!(
            classify(&state, mouse(MouseEventKind::ScrollUp, 31, 7)),
            MouseRoute::ViewerScrollUp
        );
    }

    #[test]
    fn viewer_wheel_outside_panes_is_ignored() {
        let mut state = state();
        state.viewer_content.push("line".into());

        assert_eq!(
            classify(&state, mouse(MouseEventKind::ScrollDown, 1, 1)),
            MouseRoute::Ignore
        );
    }

    #[test]
    fn right_click_opens_context_menu_at_pointer() {
        assert_eq!(
            classify(
                &state(),
                mouse(MouseEventKind::Down(MouseButton::Right), 11, 7)
            ),
            MouseRoute::ContextMenu { column: 11, row: 7 }
        );
    }

    #[test]
    fn left_drag_selects_resolved_pane_row() {
        assert_eq!(
            classify(
                &state(),
                mouse(MouseEventKind::Drag(MouseButton::Left), 31, 8)
            ),
            MouseRoute::DragSelect {
                pane: Pane::Right,
                row: 2,
            }
        );
    }

    #[test]
    fn generic_down_activates_resolved_pane_row() {
        assert_eq!(
            classify(
                &state(),
                mouse(MouseEventKind::Down(MouseButton::Middle), 11, 9)
            ),
            MouseRoute::ActivatePaneRow {
                pane: Pane::Left,
                row: 3,
            }
        );
    }
}
