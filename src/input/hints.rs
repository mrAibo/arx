use crate::app::{
    ActionContext, ActionId, AppState, InputContext, action_availability, action_meta,
};
use crate::vfs::EntryKind;

use super::Keymap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HintPriority {
    Primary,
    Secondary,
    Discovery,
}

/// One contextual, currently usable shortcut derived from ARX's shared
/// Action/Keymap/Availability truth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextHint {
    pub action: ActionId,
    pub binding: String,
    pub label: &'static str,
    pub priority: HintPriority,
    /// Whether this action is available in the current context.
    /// Unavailable actions are rendered dimmed, not hidden.
    pub available: bool,
}

/// Resolve a compact set of context-aware hints without duplicating shortcut
/// strings in the renderer.
///
/// Relevance is intentionally a presentation policy over stable `ActionId`s.
/// The physical binding always comes from `Keymap`, and unavailable actions
/// are filtered through the same `ActionAvailability` gate used by direct
/// keyboard actions and Command Center.
pub fn contextual_hints(state: &AppState, keymap: &Keymap) -> Vec<ContextHint> {
    contextual_hints_with_file_context(state, keymap, None, false)
}

pub fn contextual_hints_with_file_context(
    state: &AppState,
    keymap: &Keymap,
    focused_kind: Option<EntryKind>,
    editor_available: bool,
) -> Vec<ContextHint> {
    let context = state.input_context();
    let action_context =
        ActionContext::from_state(state).with_file_context(focused_kind, editor_available);
    let mut hints = Vec::new();

    for (action, priority) in candidate_actions(state) {
        let available = action_availability(action, &action_context).is_available();

        let Some(binding) = keymap.bindings().iter().find(|binding| {
            binding.context == context && binding.discoverable && binding.action.id() == action
        }) else {
            continue;
        };
        let Some(meta) = action_meta(action) else {
            continue;
        };

        let binding = binding
            .sequence
            .iter()
            .map(|stroke| stroke.label())
            .collect::<Vec<_>>()
            .join(" ");
        hints.push(ContextHint {
            action,
            binding,
            label: meta.label,
            priority,
            available,
        });
    }

    // Stable order: by priority, with unavailable trailing their available peers
    hints.sort_by_key(|hint| (hint.priority, !hint.available));
    hints
}

/// Returns two rows of command-bar chips.
///
/// Row A — Commander core actions: always listed. The displayed physical keys
/// come from the effective Keymap (#214); rows dim when unavailable.
/// Row B — Discovery: Ctrl+P Commands, Ctrl+D Compare, Ctrl+X P Sync,
///         Ctrl+X T Terminal, F1 Help, F10 Quit.
pub fn command_bar_rows(
    state: &AppState,
    keymap: &Keymap,
    focused_kind: Option<EntryKind>,
    editor_available: bool,
) -> (Vec<ContextHint>, Vec<ContextHint>) {
    let hints = contextual_hints_with_file_context(state, keymap, focused_kind, editor_available);

    let row_a_actions = [
        ActionId::ViewFile,
        ActionId::EditFile,
        ActionId::Copy,
        ActionId::Move,
        ActionId::Mkdir,
        ActionId::Delete,
        ActionId::OpenHosts,
    ];
    let row_b_actions = [
        ActionId::OpenCommandCenter,
        ActionId::ToggleWorkspaceComparison,
        ActionId::PreviewWorkspaceSync,
        ActionId::ToggleEmbeddedTerminal,
        ActionId::OpenHelp,
        ActionId::Quit,
    ];

    let row_a: Vec<_> = row_a_actions
        .iter()
        .filter_map(|action| hints.iter().find(|h| &h.action == action).cloned())
        .collect();

    let row_b: Vec<_> = row_b_actions
        .iter()
        .filter_map(|action| hints.iter().find(|h| &h.action == action).cloned())
        .collect();

    (row_a, row_b)
}

fn candidate_actions(state: &AppState) -> Vec<(ActionId, HintPriority)> {
    use ActionId::*;
    use HintPriority::{Discovery, Primary, Secondary};

    match state.input_context() {
        InputContext::Browser => vec![
            (ViewFile, Primary),
            (EditFile, Primary),
            (Copy, Primary),
            (Move, Primary),
            (Mkdir, Primary),
            (Delete, Primary),
            (
                if state.remote_workspace.plan.is_some() {
                    PreviewWorkspaceSync
                } else {
                    ToggleWorkspaceComparison
                },
                Primary,
            ),
            (OpenCommandCenter, Secondary),
            (ToggleEmbeddedTerminal, Discovery),
            (OpenHosts, Discovery),
            (OpenSshHosts, Discovery),
            (OpenJobs, Discovery),
            (OpenBookmarks, Discovery),
            (OpenHelp, Discovery),
            (Quit, Discovery),
            (ListTmuxSessions, Discovery),
        ],
        InputContext::SyncPreview => vec![
            (ExecuteWorkspaceSync, Primary),
            (ReverseWorkspaceDirection, Secondary),
            (ToggleWorkspaceSyncMode, Secondary),
            (CloseWorkspaceSyncOverlay, Discovery),
        ],
        InputContext::SyncConfirmation => vec![
            (ConfirmWorkspaceSync, Primary),
            (ReturnToWorkspaceSyncPreview, Secondary),
        ],
        InputContext::SyncJob => vec![
            (CancelWorkspaceSync, Primary),
            (ShowWorkspaceVerificationDiff, Secondary),
            (ReturnToWorkspaceSyncPreview, Secondary),
            (CloseWorkspaceSyncOverlay, Discovery),
        ],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyModifiers};

    use super::*;
    use crate::app::{Action, WorkspaceSyncUxState};
    use crate::input::{KeyBinding, KeyStroke};

    #[test]
    fn browser_hints_use_shared_action_and_keymap_truth() {
        let state = AppState::default();
        let hints = contextual_hints(&state, &Keymap::default());

        // Hint count includes all candidate actions, including ones with available=false
        assert_eq!(hints.len(), 15);
        // Verify actions with known bindings are present
        let mkdir = hints.iter().find(|h| h.action == ActionId::Mkdir).unwrap();
        assert_eq!(mkdir.binding, "F7");
        let compare = hints
            .iter()
            .find(|h| h.action == ActionId::ToggleWorkspaceComparison)
            .unwrap();
        assert_eq!(compare.binding, "Ctrl+D");
        let cmd = hints
            .iter()
            .find(|h| h.action == ActionId::OpenCommandCenter)
            .unwrap();
        assert_eq!(cmd.binding, "Ctrl+P");
    }

    #[test]
    fn command_bar_rows_always_include_commander_core() {
        let (_row_a, _row_b) =
            command_bar_rows(&AppState::default(), &Keymap::default(), None, false);
        assert_eq!(_row_a.len(), 7, "F3-F9 must always be present");
        assert!(
            _row_b.len() >= 3,
            "discovery row must have at least Commands + Compare + Help"
        );
    }

    #[test]
    fn local_file_hints_include_f3_and_f4_when_editor_exists() {
        let hints = contextual_hints_with_file_context(
            &AppState::default(),
            &Keymap::default(),
            Some(EntryKind::File),
            true,
        );

        let has_view = hints.iter().any(|h| matches!(h.action, ActionId::ViewFile));
        let has_edit = hints.iter().any(|h| matches!(h.action, ActionId::EditFile));
        assert!(has_view, "F3 (ViewFile) should be present");
        assert!(has_edit, "F4 (EditFile) should be present");
    }

    #[test]
    fn file_hints_follow_remote_and_editor_availability() {
        let mut remote = AppState::default();
        remote.left.location = crate::vfs::Location::Sftp {
            host: "example".into(),
            path: "/srv".into(),
        };
        let remote_hints = contextual_hints_with_file_context(
            &remote,
            &Keymap::default(),
            Some(EntryKind::File),
            true,
        );
        assert!(
            remote_hints
                .iter()
                .any(|hint| matches!(hint.action, ActionId::ViewFile))
        );
        assert!(
            remote_hints
                .iter()
                .any(|hint| matches!(hint.action, ActionId::EditFile)),
            "F4 should be available when SFTP has Read+Write"
        );

        let no_editor = contextual_hints_with_file_context(
            &AppState::default(),
            &Keymap::default(),
            Some(EntryKind::File),
            false,
        );
        // EditFile is returned but marked unavailable when editor is missing
        let edit = no_editor
            .iter()
            .find(|h| h.action == ActionId::EditFile)
            .unwrap();
        assert!(
            !edit.available,
            "EditFile should be unavailable without editor"
        );
    }

    #[test]
    fn changing_keymap_changes_hint_without_a_second_shortcut_table() {
        let keymap = Keymap::new(vec![KeyBinding::new(
            InputContext::Browser,
            vec![KeyStroke::new(KeyCode::F(12), KeyModifiers::NONE)],
            Action::ToggleWorkspaceComparison,
        )]);

        let hints = contextual_hints(&AppState::default(), &keymap);
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].binding, "F12");
    }

    #[test]
    fn unavailable_actions_are_returned_but_not_available() {
        let mut state = AppState::default();
        state.remote_workspace.preview_open = true;
        state.remote_workspace.ux = WorkspaceSyncUxState::Finished {
            job_id: "sync-1".into(),
        };

        let hints = contextual_hints(&state, &Keymap::default());
        // CancelWorkspaceSync is returned but marked unavailable
        let cancel = hints
            .iter()
            .find(|h| h.action == ActionId::CancelWorkspaceSync)
            .unwrap();
        assert!(
            !cancel.available,
            "CancelWorkspaceSync should be unavailable in Finished state"
        );
        // CloseWorkspaceSyncOverlay is available
        let close = hints
            .iter()
            .find(|h| h.action == ActionId::CloseWorkspaceSyncOverlay)
            .unwrap();
        assert!(close.available);
    }

    #[test]
    fn actions_without_a_discoverable_binding_are_not_offered() {
        let hints = contextual_hints(&AppState::default(), &Keymap::new(Vec::new()));
        assert!(hints.is_empty());
    }
}
