use arx::app::{
    Action, ActionCategory, ActionId, AppState, CommandTarget, OverlayKind, action_meta,
    build_command_items,
};
use arx::input::Keymap;

#[test]
fn action_registration_contains_routing_truth_panels() {
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
    let browser_input = include_str!("../src/tui/browser_input.rs");
    let dispatch = include_str!("../src/tui/input_dispatch.rs");
    let overlays = include_str!("../src/tui/overlays.rs");

    assert!(!dispatch.contains("// Ctrl+T: toggle Smart Tree"));
    assert_eq!(
        browser_input
            .matches(
                "KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => NewTab"
            )
            .count(),
        1,
        "Ctrl+T must have one physical owner"
    );
    assert!(dispatch.contains("browser_input::BrowserRoute::NewTab => {"));
    assert!(dispatch.contains("state.active_pane_mut().new_tab();"));
    assert!(dispatch.contains("state.message = Some(format!(\"Tab {tabs}/{tabs}\"));"));

    let alt_t =
        "KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::ALT) => TogglePanelMode";
    assert!(
        browser_input.contains(alt_t),
        "Alt+T must have a classifier route"
    );
    assert!(dispatch.contains("browser_input::BrowserRoute::TogglePanelMode => {"));
    assert!(!dispatch.contains("// Alt+T: toggle panel mode (Full ↔ Brief) KeyCode::"));

    assert_eq!(
        browser_input
            .matches(
                "KeyCode::Char('i') if key.modifiers.contains(KeyModifiers::CONTROL) => FileInfo"
            )
            .count(),
        1,
        "Ctrl+I must have one physical owner"
    );
    assert!(dispatch.contains("browser_input::BrowserRoute::FileInfo => {"));
    assert!(dispatch.contains("FileInfoService::metadata_summary("));
    assert!(!dispatch.contains("// Ctrl+I: toggle Infrastructure Center"));
    assert!(!dispatch.contains("Effect::InfrastructureSnapshot"));

    assert!(!overlays.contains("Ctrl+T toggle"));
    assert!(!overlays.contains("Ctrl+I toggle"));
    assert!(overlays.contains("ARX Smart Tree"));
    assert!(overlays.contains("Infrastructure Center — Esc close"));
}

#[test]
fn action_infrastructure_escape_closes_through_overlay_state() {
    let browser_input = include_str!("../src/tui/browser_input.rs");
    let dispatch = include_str!("../src/tui/input_dispatch.rs");
    assert!(browser_input.contains("KeyCode::Esc if state.show_infra => CloseInfrastructure"));
    assert!(dispatch.contains("browser_input::BrowserRoute::CloseInfrastructure => {"));
    assert!(dispatch.contains("state.close_overlay(OverlayKind::Infrastructure);"));

    let mut state = AppState::default();
    state.open_overlay(OverlayKind::Infrastructure);
    assert_eq!(state.active_overlay(), Some(OverlayKind::Infrastructure));
    state.close_overlay(OverlayKind::Infrastructure);
    assert_eq!(state.active_overlay(), None);
}

#[test]
#[cfg(target_os = "linux")]
fn storage_input_ownership_is_separate_from_launch_route() {
    let dispatch = include_str!("../src/tui/input_dispatch.rs");
    let browser_input = include_str!("../src/tui/browser_input.rs");

    // Active-overlay key owner must exist and consume via the real handler.
    assert!(
        dispatch.contains("if state.show_storage_inspector"),
        "input_dispatch must keep the active-overlay storage input owner"
    );
    assert!(
        dispatch.contains("arx::storage_inspector_ui::handle_storage_inspector_key"),
        "overlay input must route through handle_storage_inspector_key"
    );

    // The launch route belongs to the registered action path only.
    assert!(
        !browser_input.contains("OpenStorageInspector"),
        "browser_input must not own the Storage launch route"
    );
    assert!(
        !dispatch.contains("BrowserRoute::OpenStorageInspector"),
        "input_dispatch must not keep the legacy direct-launch branch"
    );
}

#[test]
fn sync_contexts_fail_closed_after_keyrouter_unhandled() {
    let dispatch = include_str!("../src/tui/input_dispatch.rs");
    // The routed context must be captured BEFORE resolve...
    assert!(dispatch.contains("let routed_context = state.input_context();"));
    assert!(dispatch.contains("key_router.resolve(routed_context, key)"));
    // ...and Sync contexts must consume Unhandled instead of falling through.
    for context in ["SyncPreview", "SyncConfirmation", "SyncJob"] {
        assert!(
            dispatch.contains(&format!("InputContext::{context}")),
            "{context} fail-closed guard missing"
        );
    }
}
