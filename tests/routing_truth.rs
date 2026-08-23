use arx::app::{
    ACTION_CATALOG, Action, ActionCategory, ActionId, AppState, CommandTarget, OverlayKind,
    action_meta, build_command_items,
};
use arx::input::Keymap;

#[test]
fn action_catalog_contains_routing_truth_panels() {
    let cases = [
        (
            ActionId::OpenSmartTree,
            "Smart Tree",
            "Open the filtered directory tree",
        ),
        (
            ActionId::OpenInfrastructureCenter,
            "Infrastructure Center",
            "Open infrastructure and SSH host status",
        ),
    ];

    for (id, label, description) in cases {
        let meta = action_meta(id).unwrap_or_else(|| panic!("missing action metadata for {id:?}"));
        assert_eq!(meta.label, label);
        assert_eq!(meta.description, description);
        assert_eq!(meta.category, ActionCategory::Panels);
        assert!(!meta.destructive);
        assert!(ACTION_CATALOG.iter().any(|candidate| candidate.id == id));
    }
}

#[test]
fn command_center_returns_typed_routing_truth_panels() {
    for (query, action) in [
        ("smart tree", Action::OpenSmartTree),
        ("infrastructure center", Action::OpenInfrastructureCenter),
    ] {
        let items = build_command_items(query, &AppState::default());
        assert!(
            items
                .iter()
                .any(|item| item.target == CommandTarget::Action(action)),
            "{action:?} missing from Command Center"
        );
    }
}

#[test]
fn keymap_has_no_routing_truth_panel_shortcuts() {
    let keymap = Keymap::default();
    for action in [Action::OpenSmartTree, Action::OpenInfrastructureCenter] {
        assert!(
            keymap
                .bindings()
                .iter()
                .all(|binding| binding.action != action),
            "{action:?} must remain Command-Center-only"
        );
    }
}

#[test]
fn legacy_action_routing_truth_is_reconciled() {
    let tui = include_str!("../src/tui.rs");
    let overlays = include_str!("../src/tui/overlays.rs");
    let event_loop = tui
        .split_once("async fn dispatch_ui_action(")
        .expect("dispatch_ui_action seam")
        .0;

    assert!(!event_loop.contains("// Ctrl+T: toggle Smart Tree"));
    assert_eq!(
        event_loop
            .matches("KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {")
            .count(),
        1,
        "Ctrl+T must have one physical owner"
    );
    assert!(event_loop.contains("// Ctrl+T: new tab in active pane"));
    assert!(event_loop.contains("state.active_pane_mut().new_tab();"));
    assert!(event_loop.contains("state.message = Some(format!(\"Tab {tabs}/{tabs}\"));"));

    let alt_t = "KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::ALT) => {\n                            state.panel_mode = match state.panel_mode {";
    assert!(
        event_loop.contains(alt_t),
        "Alt+T must be an executable arm"
    );
    assert!(!event_loop.contains("// Alt+T: toggle panel mode (Full ↔ Brief) KeyCode::"));

    assert_eq!(
        event_loop
            .matches("KeyCode::Char('i') if key.modifiers.contains(KeyModifiers::CONTROL) => {")
            .count(),
        1,
        "Ctrl+I must have one physical owner"
    );
    assert!(event_loop.contains("// Ctrl+I: file info (stat)"));
    assert!(event_loop.contains("FileInfoService::metadata_summary("));
    assert!(!event_loop.contains("// Ctrl+I: toggle Infrastructure Center"));
    assert!(!event_loop.contains("Effect::InfrastructureSnapshot"));

    assert!(!overlays.contains("Ctrl+T toggle"));
    assert!(!overlays.contains("Ctrl+I toggle"));
    assert!(overlays.contains("ARX Smart Tree"));
    assert!(overlays.contains("Infrastructure Center — Esc close"));
}

#[test]
fn action_infrastructure_escape_closes_through_overlay_state() {
    let tui = include_str!("../src/tui.rs");
    assert!(tui.contains(
        "KeyCode::Esc if state.show_infra => {\n                            state.close_overlay(OverlayKind::Infrastructure);"
    ));

    let mut state = AppState::default();
    state.open_overlay(OverlayKind::Infrastructure);
    assert_eq!(state.active_overlay(), Some(OverlayKind::Infrastructure));
    state.close_overlay(OverlayKind::Infrastructure);
    assert_eq!(state.active_overlay(), None);
}
