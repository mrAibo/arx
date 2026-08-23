use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BrowserRoute {
    Unhandled,
    #[cfg(target_os = "linux")]
    OpenStorageInspector,
    #[cfg(target_os = "linux")]
    OpenFilesystems,
    TreeFilterBackspace,
    SwitchPane,
    MoveUp,
    MoveDown,
    InspectFocusedEntry,
    BeginRecursiveSearch,
    BeginFilter,
    MeasureDirectoryChildren,
    ActivateEntry,
    OpenParent,
    Refresh,
    SwapPanes,
    BeginRename,
    FileAttributes,
    FileInfo,
    SyncOtherPane,
    HistoryBack,
    ResizePanelLeft,
    ResizePanelRight,
    ToggleTabSwitcher,
    ToggleHistory,
    OpenSubshell,
    BeginGoTo,
    ToggleHidden,
    InvertSelection,
    BeginGlob,
    UserMenuOrSort,
    PageWithBat,
    CopyPathToClipboard,
    SaveWorkspace,
    ToggleTransferCenter,
    TreeClose,
    CloseInfrastructure,
    TogglePanelMode,
    TreeFilterChar(char),
    BeginCommand,
    SwitchTabNumber(usize),
    NewTab,
    CloseTab,
    PreviousTab,
    NextTab,
}

pub(super) fn classify(state: &AppState, key: KeyEvent) -> BrowserRoute {
    use BrowserRoute::*;

    #[cfg(target_os = "linux")]
    if key.code == KeyCode::Char('u') && key.modifiers.contains(KeyModifiers::ALT) {
        return OpenStorageInspector;
    }

    #[cfg(target_os = "linux")]
    if key.code == KeyCode::Char('d') && key.modifiers.contains(KeyModifiers::ALT) {
        return OpenFilesystems;
    }

    if key.code == KeyCode::Backspace && state.show_tree {
        return TreeFilterBackspace;
    }

    match key.code {
        KeyCode::Tab => SwitchPane,
        KeyCode::Up | KeyCode::Char('k') => MoveUp,
        KeyCode::Down | KeyCode::Char('j') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            MoveDown
        }
        KeyCode::Char(' ') if key.modifiers.contains(KeyModifiers::CONTROL) => InspectFocusedEntry,
        KeyCode::Char('/') if key.modifiers.contains(KeyModifiers::ALT) => BeginRecursiveSearch,
        KeyCode::Char('/') => BeginFilter,
        KeyCode::Enter
            if key.modifiers.contains(KeyModifiers::ALT)
                && key.modifiers.contains(KeyModifiers::SHIFT) =>
        {
            MeasureDirectoryChildren
        }
        KeyCode::Enter => ActivateEntry,
        KeyCode::Backspace => OpenParent,
        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => Refresh,
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => SwapPanes,
        KeyCode::F(6) if key.modifiers.contains(KeyModifiers::SHIFT) => BeginRename,
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => FileAttributes,
        KeyCode::Char('i') if key.modifiers.contains(KeyModifiers::CONTROL) => FileInfo,
        KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::ALT) => SyncOtherPane,
        KeyCode::Down if key.modifiers.contains(KeyModifiers::ALT) => HistoryBack,
        KeyCode::Left
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && key.modifiers.contains(KeyModifiers::SHIFT) =>
        {
            ResizePanelLeft
        }
        KeyCode::Right
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && key.modifiers.contains(KeyModifiers::SHIFT) =>
        {
            ResizePanelRight
        }
        KeyCode::Char('`') if key.modifiers.contains(KeyModifiers::ALT) => ToggleTabSwitcher,
        KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::ALT) => ToggleHistory,
        KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => OpenSubshell,
        KeyCode::Char('g') if key.modifiers.contains(KeyModifiers::CONTROL) => BeginGoTo,
        KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => ToggleHidden,
        KeyCode::Char('*') => InvertSelection,
        KeyCode::Char('+') => BeginGlob,
        KeyCode::F(2) => UserMenuOrSort,
        KeyCode::F(3) if key.modifiers.contains(KeyModifiers::SHIFT) => PageWithBat,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => CopyPathToClipboard,
        KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => SaveWorkspace,
        KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => ToggleTransferCenter,
        KeyCode::Esc if state.show_tree => TreeClose,
        KeyCode::Esc if state.show_infra => CloseInfrastructure,
        // #212 authoritative route: Alt+T must remain panel-mode toggle even
        // while Smart Tree is open. Tree filter text owns only non-Control
        // characters that are not an explicit higher-priority browser route.
        KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::ALT) => TogglePanelMode,
        KeyCode::Char(c) if state.show_tree && !key.modifiers.contains(KeyModifiers::CONTROL) => {
            TreeFilterChar(c)
        }
        KeyCode::Char(':') => BeginCommand,
        KeyCode::Char(c)
            if key.modifiers.contains(KeyModifiers::ALT) && ('1'..='9').contains(&c) =>
        {
            SwitchTabNumber((c as u8 - b'1') as usize)
        }
        KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => NewTab,
        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => CloseTab,
        KeyCode::Left
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && !state.active_pane().tabs.is_empty() =>
        {
            PreviousTab
        }
        KeyCode::Right
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && state.active_pane().tabs.len() >= 2 =>
        {
            NextTab
        }
        _ => Unhandled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn route(code: KeyCode, modifiers: KeyModifiers) -> BrowserRoute {
        classify(&AppState::default(), key(code, modifiers))
    }

    #[test]
    fn corrected_tab_and_info_routes_are_deterministic() {
        assert_eq!(
            route(KeyCode::Char('t'), KeyModifiers::CONTROL),
            BrowserRoute::NewTab
        );
        assert_eq!(
            route(KeyCode::Char('t'), KeyModifiers::ALT),
            BrowserRoute::TogglePanelMode
        );
        assert_eq!(
            route(KeyCode::Char('i'), KeyModifiers::CONTROL),
            BrowserRoute::FileInfo
        );
    }

    #[test]
    fn corrected_alt_t_precedes_smart_tree_text_filter() {
        let state = AppState {
            show_tree: true,
            ..AppState::default()
        };
        assert_eq!(
            classify(&state, key(KeyCode::Char('t'), KeyModifiers::ALT)),
            BrowserRoute::TogglePanelMode
        );
    }

    #[test]
    fn tree_filter_precedes_generic_browser_routes() {
        let state = AppState {
            show_tree: true,
            ..AppState::default()
        };
        assert_eq!(
            classify(&state, key(KeyCode::Backspace, KeyModifiers::NONE)),
            BrowserRoute::TreeFilterBackspace
        );
        assert_eq!(
            classify(&state, key(KeyCode::Char('z'), KeyModifiers::NONE)),
            BrowserRoute::TreeFilterChar('z')
        );
        assert_eq!(
            classify(&state, key(KeyCode::Char('z'), KeyModifiers::CONTROL)),
            BrowserRoute::Unhandled
        );
        assert_eq!(
            route(KeyCode::Backspace, KeyModifiers::NONE),
            BrowserRoute::OpenParent
        );
    }

    #[test]
    fn alt_digits_carry_zero_based_tab_payloads() {
        for (digit, index) in ('1'..='9').zip(0..) {
            assert_eq!(
                route(KeyCode::Char(digit), KeyModifiers::ALT),
                BrowserRoute::SwitchTabNumber(index)
            );
        }
    }

    #[test]
    fn control_arrows_honor_existing_tab_guards() {
        let mut state = AppState::default();
        assert_eq!(
            classify(&state, key(KeyCode::Left, KeyModifiers::CONTROL)),
            BrowserRoute::Unhandled
        );
        assert_eq!(
            classify(&state, key(KeyCode::Right, KeyModifiers::CONTROL)),
            BrowserRoute::Unhandled
        );

        state.active_pane_mut().new_tab();
        assert_eq!(
            classify(&state, key(KeyCode::Left, KeyModifiers::CONTROL)),
            BrowserRoute::PreviousTab
        );
        assert_eq!(
            classify(&state, key(KeyCode::Right, KeyModifiers::CONTROL)),
            BrowserRoute::Unhandled
        );

        state.active_pane_mut().new_tab();
        assert_eq!(
            classify(&state, key(KeyCode::Right, KeyModifiers::CONTROL)),
            BrowserRoute::NextTab
        );
    }

    #[test]
    fn unknown_and_migrated_keys_have_no_legacy_route() {
        assert_eq!(
            route(KeyCode::F(11), KeyModifiers::NONE),
            BrowserRoute::Unhandled
        );
        for event in [
            key(KeyCode::Char('b'), KeyModifiers::CONTROL),
            key(KeyCode::Char('j'), KeyModifiers::CONTROL),
            key(KeyCode::Char('d'), KeyModifiers::CONTROL),
            key(KeyCode::Char('?'), KeyModifiers::NONE),
        ] {
            assert_eq!(
                classify(&AppState::default(), event),
                BrowserRoute::Unhandled
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_inspector_routes_precede_generic_fallbacks() {
        let state = AppState {
            show_tree: true,
            ..AppState::default()
        };
        assert_eq!(
            classify(&state, key(KeyCode::Char('u'), KeyModifiers::ALT)),
            BrowserRoute::OpenStorageInspector
        );
        assert_eq!(
            classify(&state, key(KeyCode::Char('d'), KeyModifiers::ALT)),
            BrowserRoute::OpenFilesystems
        );
    }

    #[test]
    fn parent_fallback_is_typed_and_has_no_physical_key_matcher() {
        const TUI_SOURCE: &str = include_str!("../tui.rs");
        let fallback = TUI_SOURCE
            .split_once("KeyResolution::Unhandled => {}")
            .expect("KeyRouter fallback seam")
            .1
            .split_once("// ── Deferred editor launch")
            .expect("event-loop tail seam")
            .0;
        assert!(fallback.contains("match browser_input::classify(&state, key) {"));
        assert!(!fallback.contains("match key.code {"));
    }

    #[test]
    fn classifier_does_not_embed_a_second_static_keymap() {
        const SOURCE: &str = include_str!("browser_input.rs");
        assert!(!SOURCE.contains("fn key_router_owns"));
        assert!(!SOURCE.contains("KeyResolution::"));
        assert!(!SOURCE.contains("KeyBinding::"));
    }
}
