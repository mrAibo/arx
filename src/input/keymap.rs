use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{Action, ActionId, InputContext};

/// The physical portion of a keyboard shortcut that ARX cares about.
///
/// Crossterm's key kind/state are intentionally ignored so key bindings stay
/// stable across terminals that report press/repeat/release differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyStroke {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyStroke {
    pub const fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self { code, modifiers }
    }

    pub fn label(self) -> String {
        let mut parts = Vec::new();
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            parts.push("Ctrl".to_string());
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            parts.push("Alt".to_string());
        }
        if self.modifiers.contains(KeyModifiers::SHIFT) {
            parts.push("Shift".to_string());
        }

        let key = match self.code {
            KeyCode::Char(' ') => "Space".to_string(),
            KeyCode::Char(c) => c.to_ascii_uppercase().to_string(),
            KeyCode::Enter => "Enter".to_string(),
            KeyCode::Esc => "Esc".to_string(),
            KeyCode::Tab => "Tab".to_string(),
            KeyCode::BackTab => "BackTab".to_string(),
            KeyCode::Backspace => "Backspace".to_string(),
            KeyCode::Delete => "Delete".to_string(),
            KeyCode::Insert => "Insert".to_string(),
            KeyCode::Up => "Up".to_string(),
            KeyCode::Down => "Down".to_string(),
            KeyCode::Left => "Left".to_string(),
            KeyCode::Right => "Right".to_string(),
            KeyCode::Home => "Home".to_string(),
            KeyCode::End => "End".to_string(),
            KeyCode::PageUp => "PageUp".to_string(),
            KeyCode::PageDown => "PageDown".to_string(),
            KeyCode::F(n) => format!("F{n}"),
            other => format!("{other:?}"),
        };
        parts.push(key);
        parts.join("+")
    }
}

impl From<KeyEvent> for KeyStroke {
    fn from(value: KeyEvent) -> Self {
        Self::new(value.code, value.modifiers)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyBinding {
    pub context: InputContext,
    pub sequence: Vec<KeyStroke>,
    pub action: Action,
    pub discoverable: bool,
}

impl KeyBinding {
    pub fn new(context: InputContext, sequence: Vec<KeyStroke>, action: Action) -> Self {
        assert!(
            !sequence.is_empty(),
            "key binding sequence must not be empty"
        );
        Self {
            context,
            sequence,
            action,
            discoverable: true,
        }
    }

    /// Compatibility binding accepted by the router but hidden from
    /// discoverability surfaces such as Which-Key.
    pub fn alias(context: InputContext, sequence: Vec<KeyStroke>, action: Action) -> Self {
        let mut binding = Self::new(context, sequence, action);
        binding.discoverable = false;
        binding
    }
}

/// Data needed by the upcoming Which-Key renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyContinuation {
    pub stroke: KeyStroke,
    pub action: ActionId,
}

#[derive(Debug, Clone)]
pub struct Keymap {
    bindings: Vec<KeyBinding>,
}

impl Keymap {
    pub fn new(bindings: Vec<KeyBinding>) -> Self {
        Self { bindings }
    }

    pub fn bindings(&self) -> &[KeyBinding] {
        &self.bindings
    }

    fn candidates<'a>(
        &'a self,
        context: InputContext,
        sequence: &'a [KeyStroke],
    ) -> impl Iterator<Item = &'a KeyBinding> + 'a {
        self.bindings.iter().filter(move |binding| {
            binding.context == context && binding.sequence.starts_with(sequence)
        })
    }
}

impl Default for Keymap {
    fn default() -> Self {
        use InputContext::{Browser, Help, SyncConfirmation, SyncJob, SyncPreview};
        use KeyCode::{Char, F};
        const CONTROL: KeyModifiers = KeyModifiers::CONTROL;
        const NONE: KeyModifiers = KeyModifiers::NONE;

        let ctrl = |c| KeyStroke::new(Char(c), CONTROL);
        let plain = |c| KeyStroke::new(Char(c), NONE);

        let mut bindings = vec![
            KeyBinding::new(Browser, vec![plain('q')], Action::Quit),
            KeyBinding::new(Browser, vec![plain(' ')], Action::ToggleSelect),
            KeyBinding::new(
                Browser,
                vec![KeyStroke::new(KeyCode::F(3), KeyModifiers::NONE)],
                Action::ViewFile,
            ),
            KeyBinding::new(
                Browser,
                vec![KeyStroke::new(KeyCode::F(4), KeyModifiers::NONE)],
                Action::EditFile,
            ),
            KeyBinding::new(
                Browser,
                vec![KeyStroke::new(KeyCode::F(5), KeyModifiers::NONE)],
                Action::Copy,
            ),
            KeyBinding::new(
                Browser,
                vec![KeyStroke::new(KeyCode::F(6), KeyModifiers::NONE)],
                Action::Move,
            ),
            KeyBinding::new(
                Browser,
                vec![KeyStroke::new(KeyCode::F(7), KeyModifiers::NONE)],
                Action::Mkdir,
            ),
            KeyBinding::new(
                Browser,
                vec![KeyStroke::new(KeyCode::F(8), KeyModifiers::NONE)],
                Action::Delete,
            ),
            KeyBinding::new(Browser, vec![ctrl('p')], Action::OpenCommandCenter),
            KeyBinding::new(Browser, vec![ctrl('b')], Action::OpenBookmarks),
            KeyBinding::new(Browser, vec![ctrl('j')], Action::OpenJobs),
            KeyBinding::new(
                Browser,
                vec![KeyStroke::new(F(9), KeyModifiers::NONE)],
                Action::OpenHosts,
            ),
            // Tmux sessions: Command Center only (Ctrl+P → "List tmux sessions")
            KeyBinding::new(Browser, vec![plain('?')], Action::OpenHelp),
            KeyBinding::new(Browser, vec![ctrl('d')], Action::ToggleWorkspaceComparison),
            // Ctrl+X P: workspace preview (safe across terminal emulators)
            KeyBinding::new(
                Browser,
                vec![ctrl('x'), plain('p')],
                Action::PreviewWorkspaceSync,
            ),
            // Ctrl+Shift+S: compatibility alias (may be intercepted by terminal)
            KeyBinding::alias(
                Browser,
                vec![KeyStroke::new(Char('s'), CONTROL | KeyModifiers::SHIFT)],
                Action::PreviewWorkspaceSync,
            ),
            // Preserve the existing behavior while also accepting the
            // conventional MC spelling: Ctrl+X followed by a plain letter.
            KeyBinding::new(Browser, vec![ctrl('x'), plain('s')], Action::BeginSymlink),
            KeyBinding::alias(Browser, vec![ctrl('x'), ctrl('s')], Action::BeginSymlink),
            KeyBinding::new(Browser, vec![ctrl('x'), plain('c')], Action::BeginChmod),
            KeyBinding::alias(Browser, vec![ctrl('x'), ctrl('c')], Action::BeginChmod),
            KeyBinding::new(Browser, vec![ctrl('x'), plain('l')], Action::BeginHardLink),
            KeyBinding::alias(Browser, vec![ctrl('x'), ctrl('l')], Action::BeginHardLink),
            KeyBinding::new(Browser, vec![ctrl('x'), plain('o')], Action::BeginChown),
            KeyBinding::alias(Browser, vec![ctrl('x'), ctrl('o')], Action::BeginChown),
            KeyBinding::new(Help, vec![plain('?')], Action::OpenHelp),
            // Current ARX behavior lets q quit while the help overlay is open.
            KeyBinding::new(Help, vec![plain('q')], Action::Quit),
            KeyBinding::new(
                SyncPreview,
                vec![plain('d')],
                Action::ReverseWorkspaceDirection,
            ),
            KeyBinding::new(
                SyncPreview,
                vec![plain('m')],
                Action::ToggleWorkspaceSyncMode,
            ),
            KeyBinding::new(
                SyncPreview,
                vec![KeyStroke::new(KeyCode::Esc, NONE)],
                Action::CloseWorkspaceSyncOverlay,
            ),
            KeyBinding::new(
                SyncPreview,
                vec![KeyStroke::new(KeyCode::Enter, NONE)],
                Action::ExecuteWorkspaceSync,
            ),
            KeyBinding::new(
                SyncConfirmation,
                vec![KeyStroke::new(KeyCode::Enter, NONE)],
                Action::ConfirmWorkspaceSync,
            ),
            KeyBinding::new(
                SyncConfirmation,
                vec![KeyStroke::new(KeyCode::Esc, NONE)],
                Action::ReturnToWorkspaceSyncPreview,
            ),
            KeyBinding::new(SyncJob, vec![plain('c')], Action::CancelWorkspaceSync),
            KeyBinding::new(
                SyncJob,
                vec![plain('v')],
                Action::ShowWorkspaceVerificationDiff,
            ),
            KeyBinding::new(
                SyncJob,
                vec![KeyStroke::new(KeyCode::Esc, NONE)],
                Action::CloseWorkspaceSyncOverlay,
            ),
            KeyBinding::new(
                SyncJob,
                vec![plain('b')],
                Action::ReturnToWorkspaceSyncPreview,
            ),
        ];

        // Deterministic order keeps Which-Key rendering and tests stable.
        bindings.sort_by_key(|binding| {
            (
                format!("{:?}", binding.context),
                binding
                    .sequence
                    .iter()
                    .map(|stroke| stroke.label())
                    .collect::<Vec<_>>()
                    .join(" "),
            )
        });

        Self::new(bindings)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyResolution {
    Action(Action),
    Pending,
    Unhandled,
}

/// Stateful resolver for both one-key shortcuts and arbitrary-length chords.
#[derive(Debug, Clone)]
pub struct KeyRouter {
    keymap: Keymap,
    pending: Vec<KeyStroke>,
    pending_context: Option<InputContext>,
}

impl KeyRouter {
    pub fn new(keymap: Keymap) -> Self {
        Self {
            keymap,
            pending: Vec::new(),
            pending_context: None,
        }
    }

    pub fn keymap(&self) -> &Keymap {
        &self.keymap
    }

    pub fn pending(&self) -> &[KeyStroke] {
        &self.pending
    }

    pub fn clear_pending(&mut self) {
        self.pending.clear();
        self.pending_context = None;
    }

    /// Return possible next strokes for the active chord.
    ///
    /// PR #4 will render these entries directly as Which-Key hints.
    pub fn continuations(&self, context: InputContext) -> Vec<KeyContinuation> {
        if self.pending.is_empty() || self.pending_context != Some(context) {
            return Vec::new();
        }

        let prefix_len = self.pending.len();
        let mut result = Vec::new();
        for binding in self.keymap.candidates(context, &self.pending) {
            if !binding.discoverable {
                continue;
            }
            if let Some(stroke) = binding.sequence.get(prefix_len).copied() {
                let continuation = KeyContinuation {
                    stroke,
                    action: binding.action.id(),
                };
                if !result.contains(&continuation) {
                    result.push(continuation);
                }
            }
        }
        result.sort_by_key(|continuation| continuation.stroke.label());
        result
    }

    pub fn resolve(&mut self, context: InputContext, event: KeyEvent) -> KeyResolution {
        self.resolve_stroke(context, event.into())
    }

    pub fn resolve_stroke(&mut self, context: InputContext, stroke: KeyStroke) -> KeyResolution {
        if self
            .pending_context
            .is_some_and(|pending| pending != context)
        {
            self.clear_pending();
        }

        if stroke.code == KeyCode::Esc && !self.pending.is_empty() {
            self.clear_pending();
            return KeyResolution::Unhandled;
        }

        self.resolve_candidate(context, stroke, true)
    }

    fn resolve_candidate(
        &mut self,
        context: InputContext,
        stroke: KeyStroke,
        retry_as_fresh_key: bool,
    ) -> KeyResolution {
        let had_pending = !self.pending.is_empty();
        let mut candidate = self.pending.clone();
        candidate.push(stroke);

        let matches: Vec<&KeyBinding> = self.keymap.candidates(context, &candidate).collect();
        let exact = matches
            .iter()
            .find(|binding| binding.sequence.len() == candidate.len())
            .map(|binding| binding.action);
        let has_longer = matches
            .iter()
            .any(|binding| binding.sequence.len() > candidate.len());

        if has_longer {
            self.pending = candidate;
            self.pending_context = Some(context);
            return KeyResolution::Pending;
        }

        if let Some(action) = exact {
            self.clear_pending();
            return KeyResolution::Action(action);
        }

        self.clear_pending();
        if had_pending && retry_as_fresh_key {
            return self.resolve_candidate(context, stroke, false);
        }

        KeyResolution::Unhandled
    }
}

impl Default for KeyRouter {
    fn default() -> Self {
        Self::new(Keymap::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctrl(c: char) -> KeyStroke {
        KeyStroke::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn plain(c: char) -> KeyStroke {
        KeyStroke::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn resolves_single_key_action() {
        let mut router = KeyRouter::default();
        assert_eq!(
            router.resolve_stroke(InputContext::Browser, plain('q')),
            KeyResolution::Action(Action::Quit)
        );
    }

    #[test]
    fn space_resolves_toggle_selection() {
        let mut router = KeyRouter::default();
        assert_eq!(
            router.resolve_stroke(InputContext::Browser, plain(' ')),
            KeyResolution::Action(Action::ToggleSelect)
        );
    }

    #[test]
    fn f3_resolves_view_file() {
        let mut router = KeyRouter::default();
        assert_eq!(
            router.resolve_stroke(
                InputContext::Browser,
                KeyStroke::new(KeyCode::F(3), KeyModifiers::NONE),
            ),
            KeyResolution::Action(Action::ViewFile)
        );
    }

    #[test]
    fn f4_resolves_edit_file() {
        let mut router = KeyRouter::default();
        assert_eq!(
            router.resolve_stroke(
                InputContext::Browser,
                KeyStroke::new(KeyCode::F(4), KeyModifiers::NONE),
            ),
            KeyResolution::Action(Action::EditFile)
        );
    }

    #[test]
    fn ctrl_x_enters_pending_chord_state() {
        let mut router = KeyRouter::default();
        assert_eq!(
            router.resolve_stroke(InputContext::Browser, ctrl('x')),
            KeyResolution::Pending
        );
        assert_eq!(router.pending(), &[ctrl('x')]);
        assert!(!router.continuations(InputContext::Browser).is_empty());
    }

    #[test]
    fn ctrl_x_plain_c_resolves_chmod() {
        let mut router = KeyRouter::default();
        assert_eq!(
            router.resolve_stroke(InputContext::Browser, ctrl('x')),
            KeyResolution::Pending
        );
        assert_eq!(
            router.resolve_stroke(InputContext::Browser, plain('c')),
            KeyResolution::Action(Action::BeginChmod)
        );
        assert!(router.pending().is_empty());
    }

    #[test]
    fn legacy_ctrl_x_ctrl_c_still_resolves_chmod() {
        let mut router = KeyRouter::default();
        assert_eq!(
            router.resolve_stroke(InputContext::Browser, ctrl('x')),
            KeyResolution::Pending
        );
        assert_eq!(
            router.resolve_stroke(InputContext::Browser, ctrl('c')),
            KeyResolution::Action(Action::BeginChmod)
        );
    }

    #[test]
    fn unknown_second_key_cancels_chord_and_retries_as_fresh_key() {
        let mut router = KeyRouter::default();
        assert_eq!(
            router.resolve_stroke(InputContext::Browser, ctrl('x')),
            KeyResolution::Pending
        );
        assert_eq!(
            router.resolve_stroke(InputContext::Browser, plain('q')),
            KeyResolution::Action(Action::Quit)
        );
        assert!(router.pending().is_empty());
    }

    #[test]
    fn changing_context_cancels_pending_chord() {
        let mut router = KeyRouter::default();
        assert_eq!(
            router.resolve_stroke(InputContext::Browser, ctrl('x')),
            KeyResolution::Pending
        );
        assert_eq!(
            router.resolve_stroke(InputContext::Help, plain('?')),
            KeyResolution::Action(Action::OpenHelp)
        );
        assert!(router.pending().is_empty());
    }

    #[test]
    fn ctrl_p_is_a_real_command_center_binding() {
        let mut router = KeyRouter::default();
        assert_eq!(
            router.resolve_stroke(InputContext::Browser, ctrl('p')),
            KeyResolution::Action(Action::OpenCommandCenter)
        );
    }

    #[test]
    fn continuation_labels_are_ready_for_which_key() {
        let mut router = KeyRouter::default();
        assert_eq!(
            router.resolve_stroke(InputContext::Browser, ctrl('x')),
            KeyResolution::Pending
        );
        let labels: Vec<String> = router
            .continuations(InputContext::Browser)
            .into_iter()
            .map(|continuation| continuation.stroke.label())
            .collect();

        assert!(labels.contains(&"C".to_string()));
        assert!(labels.contains(&"L".to_string()));
        assert!(labels.contains(&"O".to_string()));
        assert!(labels.contains(&"S".to_string()));
    }

    #[test]
    fn f8_resolves_to_delete() {
        let mut router = KeyRouter::default();
        assert_eq!(
            router.resolve_stroke(
                InputContext::Browser,
                KeyStroke::new(KeyCode::F(8), KeyModifiers::NONE)
            ),
            KeyResolution::Action(Action::Delete)
        );
    }

    #[test]
    fn f9_resolves_to_hosts() {
        let mut router = KeyRouter::default();
        assert_eq!(
            router.resolve_stroke(
                InputContext::Browser,
                KeyStroke::new(KeyCode::F(9), KeyModifiers::NONE)
            ),
            KeyResolution::Action(Action::OpenHosts)
        );
    }
}
