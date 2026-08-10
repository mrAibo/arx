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
        if !action_availability(action, &action_context).is_available() {
            continue;
        }

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
        });
    }

    hints.sort_by_key(|hint| hint.priority);
    hints
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
            (OpenHosts, Discovery),
            (OpenJobs, Discovery),
            (OpenBookmarks, Discovery),
            (OpenHelp, Discovery),
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

        assert_eq!(hints.len(), 7);
        assert_eq!(hints[0].action, ActionId::Mkdir);
        assert_eq!(hints[0].binding, "F7");
        assert_eq!(hints[1].action, ActionId::ToggleWorkspaceComparison);
        assert_eq!(hints[1].binding, "Ctrl+D");
        assert_eq!(hints[2].action, ActionId::OpenCommandCenter);
        assert_eq!(hints[2].binding, "Ctrl+P");
        assert_eq!(hints[3].action, ActionId::OpenJobs);
        assert_eq!(hints[3].binding, "Ctrl+J");
        assert_eq!(hints[4].action, ActionId::OpenBookmarks);
        assert_eq!(hints[4].binding, "Ctrl+B");
        assert_eq!(hints[5].action, ActionId::OpenHelp);
        assert_eq!(hints[5].binding, "?");
        assert_eq!(hints[6].action, ActionId::ListTmuxSessions);
        assert_eq!(hints[6].binding, "F9");
    }

    #[test]
    fn local_file_hints_include_f3_and_f4_when_editor_exists() {
        let hints = contextual_hints_with_file_context(
            &AppState::default(),
            &Keymap::default(),
            Some(EntryKind::File),
            true,
        );

        assert_eq!(hints[0].action, ActionId::ViewFile);
        assert_eq!(hints[0].binding, "F3");
        assert_eq!(hints[1].action, ActionId::EditFile);
        assert_eq!(hints[1].binding, "F4");
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
                .all(|hint| !matches!(hint.action, ActionId::ViewFile | ActionId::EditFile))
        );

        let no_editor = contextual_hints_with_file_context(
            &AppState::default(),
            &Keymap::default(),
            Some(EntryKind::File),
            false,
        );
        assert!(
            no_editor
                .iter()
                .any(|hint| hint.action == ActionId::ViewFile)
        );
        assert!(
            no_editor
                .iter()
                .all(|hint| hint.action != ActionId::EditFile)
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
    fn unavailable_actions_are_not_offered() {
        let mut state = AppState::default();
        state.remote_workspace.preview_open = true;
        state.remote_workspace.ux = WorkspaceSyncUxState::Finished {
            job_id: "sync-1".into(),
        };

        let hints = contextual_hints(&state, &Keymap::default());
        assert!(
            !hints
                .iter()
                .any(|hint| hint.action == ActionId::CancelWorkspaceSync)
        );
        assert!(
            hints
                .iter()
                .any(|hint| hint.action == ActionId::CloseWorkspaceSyncOverlay)
        );
    }

    #[test]
    fn actions_without_a_discoverable_binding_are_not_offered() {
        let hints = contextual_hints(&AppState::default(), &Keymap::new(Vec::new()));
        assert!(hints.is_empty());
    }
}
