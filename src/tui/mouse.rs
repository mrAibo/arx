use super::*;

// ── #10: private TUI mouse UI state (not a runtime/service/channel) ──

/// One context-menu entry: canonical action + metadata + availability snapshot.
pub(crate) struct ContextMenuItem {
    pub action: Action,
    pub label: &'static str,
    pub availability: arx::app::ActionAvailability,
}

/// Frozen right-click target: exact pane, cloned location, cloned ListedEntry
/// (native identity preserved — never reconstructed from presentation data).
pub(crate) struct ContextMenuTarget {
    pub pane: Pane,
    pub location: Location,
    pub listed: ListedEntry,
}

pub(crate) struct ContextMenuState {
    pub anchor: (u16, u16),
    pub items: Vec<ContextMenuItem>,
    pub target: ContextMenuTarget,
}

#[derive(Default)]
pub(crate) struct MouseUiState {
    /// Last seen frame area, used for popup clamping/hit-testing.
    pub frame_area: Option<Rect>,
    pub context_menu: Option<ContextMenuState>,
}

impl MouseUiState {
    pub fn close_context_menu(&mut self) {
        self.context_menu = None;
    }

    /// Deterministic popup rect anchored at the pointer, clamped inside `area`.
    pub fn context_menu_rect(&self) -> Option<Rect> {
        let menu = self.context_menu.as_ref()?;
        let area = self.frame_area?;
        Some(context_menu_rect(menu.anchor, menu.items.len(), area))
    }
}

/// Anchor-based menu rectangle: wide enough for the longest label, tall enough
/// for every item, clamped so it never crosses the right/bottom edge.
pub(super) fn context_menu_rect(anchor: (u16, u16), item_count: usize, area: Rect) -> Rect {
    let width = 18u16.min(area.width);
    let height = (item_count as u16 + 2).min(area.height); // +2 border rows
    let max_x = area.x + area.width.saturating_sub(width);
    let max_y = area.y + area.height.saturating_sub(height);
    let x = anchor.0.min(max_x).max(area.x);
    let y = anchor.1.min(max_y).max(area.y);
    Rect::new(x, y, width, height)
}

/// Canonical #10 menu actions. Labels/availability always come from the
/// canonical registration + availability seam — never duplicated here.
const CONTEXT_MENU_ACTIONS: [Action; 6] = [
    Action::ViewFile,
    Action::EditFile,
    Action::Copy,
    Action::Move,
    Action::Mkdir,
    Action::Delete,
];

pub(super) fn build_context_menu_items(
    state: &AppState,
    target_kind: arx::vfs::EntryKind,
    configured_editor: Option<&str>,
) -> Vec<ContextMenuItem> {
    let context = arx::app::ActionContext::from_state(state)
        .with_file_context(Some(target_kind), configured_editor.is_some());
    CONTEXT_MENU_ACTIONS
        .iter()
        .filter_map(|action| {
            let id = action.id();
            let meta = arx::app::action_meta(id)?;
            let availability = arx::app::action_availability(id, &context);
            let visible_item = !matches!(availability, arx::app::ActionAvailability::Hidden);
            visible_item.then_some(ContextMenuItem {
                action: *action,
                label: meta.label,
                availability,
            })
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MouseRoute {
    Ignore,
    CommandBar {
        action: Action,
        available: bool,
    },
    ViewerScrollDown,
    ViewerScrollUp,
    // #16: section = which same-location subview the pointer is in.
    PaneScrollDown {
        pane: Pane,
        section: SplitSection,
    },
    PaneScrollUp {
        pane: Pane,
        section: SplitSection,
    },
    ContextMenu {
        column: u16,
        row: u16,
        pane: Pane,
        section: SplitSection,
    },
    RangeSelect {
        pane: Pane,
        section: SplitSection,
        row: usize,
    },
    DragSelect {
        pane: Pane,
        section: SplitSection,
        row: usize,
    },
    ActivatePaneRow {
        pane: Pane,
        section: SplitSection,
        row: usize,
    },
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
    // #16: resolve WHICH subview of this outer pane holds the pointer and
    // compute the row relative to THAT subview's own origin (horizontal
    // secondary has a different y-origin).
    let pane_state = match pane {
        Pane::Left => &state.left,
        Pane::Right => &state.right,
    };
    let rects = super::split_layout::split_rects(
        area,
        pane_state.split,
        pane_state.split_orientation,
        pane_state.split_ratio,
    );
    let Some((section, section_rect)) =
        super::split_layout::section_at_point(&rects, mouse.column, mouse.row)
    else {
        return MouseRoute::Ignore;
    };
    let row = mouse.row.saturating_sub(section_rect.y + 1) as usize;

    // #10: wheel over the viewer stays viewer-owned regardless of position;
    // otherwise it becomes a pane scroll for the pane under the pointer.
    match mouse.kind {
        MouseEventKind::ScrollDown if !state.viewer_content.is_empty() => {
            MouseRoute::ViewerScrollDown
        }
        MouseEventKind::ScrollUp if !state.viewer_content.is_empty() => MouseRoute::ViewerScrollUp,
        MouseEventKind::ScrollDown => MouseRoute::PaneScrollDown { pane, section },
        MouseEventKind::ScrollUp => MouseRoute::PaneScrollUp { pane, section },
        MouseEventKind::Down(MouseButton::Right) => MouseRoute::ContextMenu {
            column: mouse.column,
            row: mouse.row,
            pane,
            section,
        },
        // Shift+Click is an explicit inclusive range selection.
        MouseEventKind::Down(MouseButton::Left)
            if mouse.modifiers.contains(KeyModifiers::SHIFT) =>
        {
            MouseRoute::RangeSelect { pane, section, row }
        }
        MouseEventKind::Drag(MouseButton::Left) => MouseRoute::DragSelect { pane, section, row },
        MouseEventKind::Down(_) => MouseRoute::ActivatePaneRow { pane, section, row },
        _ => MouseRoute::Ignore,
    }
}

/// Hit-test a rendered context menu. Returns Some(item_index) for a click on a
/// visible item row (border offset 1), None when the click is outside the menu.
pub(super) fn context_menu_hit(menu_rect: Rect, column: u16, row: u16) -> Option<usize> {
    if column <= menu_rect.x
        || column >= menu_rect.x + menu_rect.width
        || row <= menu_rect.y
        || row >= menu_rect.y + menu_rect.height
    {
        return None;
    }
    Some((row - menu_rect.y - 1) as usize)
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
                section: SplitSection::Primary,
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
                section: SplitSection::Primary,
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
                section: SplitSection::Primary,
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
            MouseRoute::ContextMenu {
                column: 11,
                row: 7,
                pane: Pane::Left,
                section: SplitSection::Primary
            }
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
                section: SplitSection::Primary,
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
                section: SplitSection::Primary,
                row: 3,
            }
        );
    }
}
