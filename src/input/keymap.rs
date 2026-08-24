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

/// Where a binding came from (#214). Never inferred from strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingSource {
    BuiltIn,
    User,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyBinding {
    pub context: InputContext,
    pub sequence: Vec<KeyStroke>,
    pub action: Action,
    pub discoverable: bool,
    pub source: BindingSource,
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
            source: BindingSource::BuiltIn,
        }
    }

    /// Compatibility binding accepted by the router but hidden from
    /// discoverability surfaces such as Which-Key.
    pub fn alias(context: InputContext, sequence: Vec<KeyStroke>, action: Action) -> Self {
        let mut binding = Self::new(context, sequence, action);
        binding.discoverable = false;
        binding
    }

    /// User-provided override binding (#214): always discoverable.
    pub fn user(context: InputContext, sequence: Vec<KeyStroke>, action: Action) -> Self {
        let mut binding = Self::new(context, sequence, action);
        binding.source = BindingSource::User;
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
                vec![KeyStroke::new(KeyCode::F(1), KeyModifiers::NONE)],
                Action::OpenHelp,
            ),
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
            KeyBinding::new(Browser, vec![ctrl('\\')], Action::ToggleSplitPane),
            // Ctrl+X T: embedded terminal (safe across emulators, unlike Ctrl+Shift+T)
            KeyBinding::new(
                Browser,
                vec![ctrl('x'), plain('t')],
                Action::ToggleEmbeddedTerminal,
            ),
            KeyBinding::new(Browser, vec![ctrl('b')], Action::OpenBookmarks),
            KeyBinding::new(Browser, vec![ctrl('j')], Action::OpenJobs),
            KeyBinding::new(
                Browser,
                vec![KeyStroke::new(F(9), KeyModifiers::NONE)],
                Action::OpenHosts,
            ),
            KeyBinding::new(
                Browser,
                vec![KeyStroke::new(F(12), KeyModifiers::NONE)],
                Action::OpenSshHosts,
            ),
            // Storage Inspector: fixed default binding (formerly hard-coded Alt+U)
            KeyBinding::new(
                Browser,
                vec![KeyStroke::new(Char('u'), KeyModifiers::ALT)],
                Action::OpenStorageInspector,
            ),
            KeyBinding::new(
                Browser,
                vec![KeyStroke::new(F(10), KeyModifiers::NONE)],
                Action::Quit,
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

// ── #214: key syntax parsing, effective-keymap construction, conflict rules ──

/// One parse failure with sanitized, injection-free text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeymapError {
    pub message: String,
}

impl KeymapError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: crate::app::sanitize_config_token(&message.into()),
        }
    }
}

impl std::fmt::Display for KeymapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// Parse one stroke token like `Ctrl+Shift+S`, `Alt+U`, `F11`, `Space`, `Plus`.
///
/// Case-insensitive modifier/key names; canonical modifier order is
/// Ctrl+Alt+Shift+KEY. Exactly one non-modifier key is required.
pub fn parse_stroke(token: &str) -> Result<KeyStroke, KeymapError> {
    let token = token.trim();
    if token.is_empty() {
        return Err(KeymapError::new("empty key token"));
    }
    let mut ctrl = false;
    let mut alt = false;
    let mut shift = false;
    let mut parts: Vec<&str> = token.split('+').collect();
    // Literal Plus: "Ctrl+Plus" has 2 parts where last is the key name.
    let key_token = parts.pop().expect("non-empty split");
    for part in &parts {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => {
                if ctrl {
                    return Err(KeymapError::new(format!("duplicate Ctrl in {token}")));
                }
                ctrl = true;
            }
            "alt" => {
                if alt {
                    return Err(KeymapError::new(format!("duplicate Alt in {token}")));
                }
                alt = true;
            }
            "shift" => {
                if shift {
                    return Err(KeymapError::new(format!("duplicate Shift in {token}")));
                }
                shift = true;
            }
            other => {
                return Err(KeymapError::new(format!(
                    "{other:?} is not a modifier in {token}"
                )));
            }
        }
    }
    if parts.len() > 3 {
        return Err(KeymapError::new(format!("too many modifiers in {token}")));
    }

    let lower = key_token.to_ascii_lowercase();
    let (code, needs_shift_from_name) = match lower.as_str() {
        "space" => (KeyCode::Char(' '), false),
        "plus" => (KeyCode::Char('+'), false),
        "enter" => (KeyCode::Enter, false),
        "esc" | "escape" => (KeyCode::Esc, false),
        "tab" => (KeyCode::Tab, false),
        "backtab" => (KeyCode::BackTab, false),
        "backspace" => (KeyCode::Backspace, false),
        "delete" => (KeyCode::Delete, false),
        "insert" => (KeyCode::Insert, false),
        "up" => (KeyCode::Up, false),
        "down" => (KeyCode::Down, false),
        "left" => (KeyCode::Left, false),
        "right" => (KeyCode::Right, false),
        "home" => (KeyCode::Home, false),
        "end" => (KeyCode::End, false),
        "pageup" => (KeyCode::PageUp, false),
        "pagedown" => (KeyCode::PageDown, false),
        f if f.len() >= 2 && f.starts_with('f') && f[1..].parse::<u8>().is_ok() => {
            let n: u8 = f[1..].parse().map_err(|_| KeymapError::new("bad F-key"))?;
            if !(1..=12).contains(&n) {
                return Err(KeymapError::new(format!("unsupported function key F{n}")));
            }
            (KeyCode::F(n), false)
        }
        c if c.chars().count() == 1 => {
            let ch = c.chars().next().expect("single char");
            if ch.is_control() {
                return Err(KeymapError::new("control characters are not bindable"));
            }
            // ASCII letters normalize to uppercase so parser output matches
            // the built-in defaults' KeyStroke shape.
            if ch.is_ascii_alphabetic() {
                (KeyCode::Char(ch.to_ascii_uppercase()), true)
            } else {
                (KeyCode::Char(ch), false)
            }
        }
        _ => {
            return Err(KeymapError::new(format!(
                "unsupported key name {key_token}"
            )));
        }
    };
    let _ = needs_shift_from_name;
    let mut modifiers = KeyModifiers::NONE;
    if ctrl {
        modifiers |= KeyModifiers::CONTROL;
    }
    if alt {
        modifiers |= KeyModifiers::ALT;
    }
    if shift {
        modifiers |= KeyModifiers::SHIFT;
    }
    Ok(KeyStroke::new(code, modifiers))
}

/// Canonical label for one stroke: Ctrl+Alt+Shift+KEY (KeyStroke::label order).
#[allow(dead_code)]
fn stroke_canonical(stroke: KeyStroke) -> String {
    stroke.label()
}

/// Parse a whitespace-separated chord like `Ctrl+X P` or `Ctrl+X Ctrl+C`.
pub fn parse_chord(value: &str) -> Result<Vec<KeyStroke>, KeymapError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(KeymapError::new("empty key sequence"));
    }
    value.split_whitespace().map(parse_stroke).collect()
}

// ── #214: effective keymap construction (one builder) ──

/// User-configurable input contexts in #214 v1. Everything else stays owned
/// by its direct controller / overlay handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigContext {
    Browser,
    SyncPreview,
    SyncConfirmation,
    SyncJob,
}

impl ConfigContext {
    pub fn input_context(self) -> InputContext {
        match self {
            ConfigContext::Browser => InputContext::Browser,
            ConfigContext::SyncPreview => InputContext::SyncPreview,
            ConfigContext::SyncConfirmation => InputContext::SyncConfirmation,
            ConfigContext::SyncJob => InputContext::SyncJob,
        }
    }

    pub fn config_name(self) -> &'static str {
        match self {
            ConfigContext::Browser => "browser",
            ConfigContext::SyncPreview => "sync_preview",
            ConfigContext::SyncConfirmation => "sync_confirmation",
            ConfigContext::SyncJob => "sync_job",
        }
    }

    pub fn parse(value: &str) -> Result<Self, KeymapError> {
        match value.trim() {
            "browser" => Ok(ConfigContext::Browser),
            "sync_preview" => Ok(ConfigContext::SyncPreview),
            "sync_confirmation" => Ok(ConfigContext::SyncConfirmation),
            "sync_job" => Ok(ConfigContext::SyncJob),
            other => Err(KeymapError::new(format!(
                "unsupported context {:?}: configurable contexts are browser, sync_preview, sync_confirmation, sync_job",
                crate::app::sanitize_config_token(other)
            ))),
        }
    }
}

/// V1 bindability policy (#214): capability/policy list, NOT a shortcut table.
///
/// Browser v1 binds actions genuinely owned by KeyRouter dispatch plus safe
/// typed/registered-controller actions. Legacy navigation whose semantics live
/// in BrowserRoute (Up/Down/Enter/Back/SwitchPane/Refresh) and confirmation
/// flows that own their own input are deliberately excluded.
fn is_bindable(context: ConfigContext, id: ActionId) -> bool {
    use ActionId::*;
    match context {
        ConfigContext::Browser => matches!(
            id,
            Quit | ToggleSelect
                | OpenHelp
                | ViewFile
                | EditFile
                | Copy
                | Move
                | Mkdir
                | Delete
                | OpenCommandCenter
                | ToggleSplitPane
                | ToggleEmbeddedTerminal
                | OpenBookmarks
                | OpenJobs
                | OpenHosts
                | OpenSshHosts
                | OpenStorageInspector
                | ToggleWorkspaceComparison
                | PreviewWorkspaceSync
                | BeginSymlink
                | BeginChmod
                | BeginHardLink
                | BeginChown
                | ComputeSha256
                | TouchFile
                | CompressTarGz
                | ListTmuxSessions
                | OpenSmartTree
                | OpenInfrastructureCenter
                | OpenHotlist
                | OpenInFileManager
        ),
        ConfigContext::SyncPreview => matches!(
            id,
            ReverseWorkspaceDirection
                | ToggleWorkspaceSyncMode
                | CloseWorkspaceSyncOverlay
                | ExecuteWorkspaceSync
        ),
        ConfigContext::SyncConfirmation => {
            matches!(id, ConfirmWorkspaceSync | ReturnToWorkspaceSyncPreview)
        }
        ConfigContext::SyncJob => matches!(
            id,
            CancelWorkspaceSync
                | ShowWorkspaceVerificationDiff
                | CloseWorkspaceSyncOverlay
                | ReturnToWorkspaceSyncPreview
        ),
    }
}

impl Keymap {
    /// Build ONE effective keymap from built-in defaults + validated user
    /// overrides (#214). No overrides => behaviorally identical to `default()`.
    pub fn effective(overrides: &[crate::config::KeybindingConfig]) -> Result<Keymap, KeymapError> {
        let mut bindings: Vec<KeyBinding> = Keymap::default().bindings.to_vec();
        let mut seen_pairs: std::collections::HashSet<(String, String)> =
            std::collections::HashSet::new();

        for override_row in overrides {
            let context = ConfigContext::parse(&override_row.context)?;
            let action_id: ActionId = override_row
                .action
                .parse()
                .map_err(|e: String| KeymapError::new(e))?;

            if !is_bindable(context, action_id) {
                return Err(KeymapError::new(format!(
                    "action {} is not configurable in context {}",
                    crate::app::sanitize_config_token(&override_row.action),
                    context.config_name(),
                )));
            }
            if !seen_pairs.insert((override_row.context.clone(), override_row.action.clone())) {
                return Err(KeymapError::new(format!(
                    "duplicate override for {}:{}",
                    crate::app::sanitize_config_token(&override_row.context),
                    crate::app::sanitize_config_token(&override_row.action),
                )));
            }

            // keys XOR disabled
            let keys_trimmed = override_row.keys.as_deref().map(str::trim);
            let has_keys = keys_trimmed.is_some_and(|k| !k.is_empty());
            if override_row.disabled && has_keys {
                return Err(KeymapError::new(format!(
                    "{}:{} must set exactly one of keys or disabled = true",
                    context.config_name(),
                    crate::app::sanitize_config_token(&override_row.action),
                )));
            }
            if !override_row.disabled && !has_keys {
                return Err(KeymapError::new(format!(
                    "{}:{} requires either non-empty keys or disabled = true",
                    context.config_name(),
                    crate::app::sanitize_config_token(&override_row.action),
                )));
            }
            if override_row.keys.is_some() && !has_keys {
                return Err(KeymapError::new(format!(
                    "{}:{} keys must not be empty",
                    context.config_name(),
                    crate::app::sanitize_config_token(&override_row.action),
                )));
            }

            // Remove ALL built-in bindings for exactly this pair (incl. aliases).
            let ctx_input = context.input_context();
            bindings.retain(|binding| {
                !(binding.context == ctx_input && binding.action.id() == action_id)
            });

            if override_row.disabled {
                continue;
            }
            let sequence = parse_chord(keys_trimmed.expect("checked above"))?;
            bindings.push(KeyBinding::user(
                ctx_input,
                sequence,
                crate::app::action_for_id(action_id)
                    .ok_or_else(|| KeymapError::new("unregistered action"))?,
            ));
        }

        let effective = Keymap::new(bindings);
        effective.validate_conflicts()?;
        Ok(effective.into_sorted())
    }

    /// Deterministic presentation order (context, sequence labels, source).
    pub fn into_sorted(mut self) -> Keymap {
        self.bindings.sort_by_key(|binding| {
            (
                format!("{:?}", binding.context),
                binding
                    .sequence
                    .iter()
                    .map(|s| s.label())
                    .collect::<Vec<_>>()
                    .join(" "),
                format!("{:?}", binding.source),
            )
        });
        self
    }

    /// Conflict rules within ONE context (#214 §11):
    /// exact duplicates and prefix ambiguity in either direction are errors;
    /// cross-context reuse of a sequence is valid.
    fn validate_conflicts(&self) -> Result<(), KeymapError> {
        for i in 0..self.bindings.len() {
            for j in (i + 1)..self.bindings.len() {
                let a = &self.bindings[i];
                let b = &self.bindings[j];
                if a.context != b.context {
                    continue;
                }
                let shorter = a.sequence.len().min(b.sequence.len());
                let prefix_equal = a.sequence[..shorter] == b.sequence[..shorter];
                if !prefix_equal {
                    continue;
                }
                let label = |b: &KeyBinding| -> String {
                    b.sequence
                        .iter()
                        .map(|s| s.label())
                        .collect::<Vec<_>>()
                        .join(" ")
                };
                let kind = if a.sequence.len() == b.sequence.len() {
                    "conflicts with an existing binding"
                } else if a.sequence.len() < b.sequence.len() {
                    "is a prefix of another binding"
                } else {
                    "would be shadowed by a longer binding"
                };
                let context_name = format!("{:?}", a.context);
                return Err(KeymapError::new(format!(
                    "{context_name}: {} -> {} {kind} {} -> {}",
                    label(a),
                    a.action.id().config_name(),
                    label(b),
                    b.action.id().config_name(),
                )));
            }
        }
        Ok(())
    }

    /// Primary user-facing shortcut label for (context, action): the first
    /// DISCOVERABLE binding; aliases never become the displayed shortcut.
    /// None when unbound/disabled.
    pub fn primary_binding_label(
        &self,
        context: InputContext,
        action_id: ActionId,
    ) -> Option<String> {
        self.bindings
            .iter()
            .find(|binding| {
                binding.context == context
                    && binding.discoverable
                    && binding.action.id() == action_id
            })
            .map(|binding| {
                binding
                    .sequence
                    .iter()
                    .map(|s| s.label())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
    }

    /// All discoverable binding labels for (context, action), deterministic.
    pub fn discoverable_bindings(&self, context: InputContext, action_id: ActionId) -> Vec<String> {
        self.bindings
            .iter()
            .filter(|binding| {
                binding.context == context
                    && binding.discoverable
                    && binding.action.id() == action_id
            })
            .map(|binding| {
                binding
                    .sequence
                    .iter()
                    .map(|s| s.label())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect()
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

    #[cfg(target_os = "linux")]
    #[test]
    fn r_browser_alt_u_resolves_open_storage_inspector() {
        let mut router = KeyRouter::default();
        assert_eq!(
            router.resolve_stroke(
                InputContext::Browser,
                KeyStroke::new(KeyCode::Char('u'), KeyModifiers::ALT)
            ),
            KeyResolution::Action(Action::OpenStorageInspector)
        );
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
    fn ctrl_backslash_resolves_toggle_split_pane() {
        let mut router = KeyRouter::default();
        assert_eq!(
            router.resolve_stroke(InputContext::Browser, ctrl('\\')),
            KeyResolution::Action(Action::ToggleSplitPane)
        );
    }

    #[test]
    fn browser_has_exactly_one_ctrl_backslash_binding() {
        let keymap = Keymap::default();
        let count = keymap
            .bindings()
            .iter()
            .filter(|binding| {
                binding.context == InputContext::Browser && binding.sequence == [ctrl('\\')]
            })
            .count();
        assert_eq!(count, 1);
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

// ── #214 test matrix ──
#[cfg(test)]
mod r214_tests {
    use super::*;
    use crate::app::{Action, ActionId};
    use crate::config::KeybindingConfig;
    use crossterm::event::{KeyCode, KeyModifiers};

    fn kb(context: &str, action: &str, keys: Option<&str>, disabled: bool) -> KeybindingConfig {
        KeybindingConfig {
            context: context.into(),
            action: action.into(),
            keys: keys.map(|k| k.into()),
            disabled,
        }
    }

    // ── ACTION IDENTITY ──
    #[test]
    fn r214_every_registered_action_has_unique_stable_config_name() {
        let mut names = Vec::new();
        for registration in crate::app::registrations_for_test() {
            let name = registration.meta.id.config_name();
            assert!(!name.is_empty());
            names.push(name);
        }
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "config names must be unique");
    }

    #[test]
    fn r214_config_name_round_trip_and_unknown_rejected() {
        for registration in crate::app::registrations_for_test() {
            let id = registration.meta.id;
            let parsed: ActionId = id.config_name().parse().expect("round trip");
            assert_eq!(parsed, id);
        }
        assert!("no_such_action".parse::<ActionId>().is_err());
        assert!("Quit; rm -rf /".parse::<ActionId>().is_err());
    }

    #[test]
    fn r214_action_for_id_backed_by_registration() {
        assert_eq!(
            crate::app::action_for_id(ActionId::OpenStorageInspector),
            Some(Action::OpenStorageInspector)
        );
    }

    // ── KEY PARSER ──
    #[test]
    fn r214_parser_shapes() {
        let s = parse_stroke("Ctrl+P").unwrap();
        assert_eq!(s.code, KeyCode::Char('P'));
        assert_eq!(s.modifiers, KeyModifiers::CONTROL);

        let s = parse_stroke("alt+u").unwrap(); // case-insensitive
        assert_eq!(s.code, KeyCode::Char('U'));
        assert_eq!(s.modifiers, KeyModifiers::ALT);

        let s = parse_stroke("Ctrl+Shift+S").unwrap();
        assert_eq!(s.code, KeyCode::Char('S'));
        assert_eq!(s.modifiers, KeyModifiers::CONTROL | KeyModifiers::SHIFT);

        let s = parse_stroke("F12").unwrap();
        assert_eq!(s.code, KeyCode::F(12));
        let s = parse_stroke("f1").unwrap();
        assert_eq!(s.code, KeyCode::F(1));

        assert!(parse_chord("Down").is_ok());
        assert!(parse_chord("Space").is_ok());
        assert_eq!(parse_stroke("Plus").unwrap().code, KeyCode::Char('+'));
        assert_eq!(parse_stroke("ctrl+plus").unwrap().code, KeyCode::Char('+'));
        assert_eq!(parse_stroke("\\").unwrap().code, KeyCode::Char('\\'));

        // canonical round trip
        for token in ["Ctrl+Shift+S", "Alt+U", "Ctrl+X P", "F11", "Space"] {
            for stroke in parse_chord(token).unwrap() {
                let label = stroke.label();
                let reparsed: Vec<KeyStroke> = label
                    .split_whitespace()
                    .map(parse_stroke)
                    .collect::<Result<_, _>>()
                    .unwrap();
                assert_eq!(reparsed, vec![stroke], "round trip failed for {token}");
            }
        }
    }

    #[test]
    fn r214_parser_rejects_malformed() {
        assert!(parse_stroke("").is_err());
        assert!(parse_stroke("Ctrl").is_err()); // modifier without key
        assert!(parse_stroke("Ctrl+Alt+X P").is_err()); // two keys in one token? split by whitespace -> separate strokes; single token has one key only
        assert!(parse_stroke("Ctrl+Ctrl+P").is_err()); // duplicate modifier
        assert!(parse_stroke("NotAKey").is_err());
        assert!(parse_stroke("F99").is_err());
        assert!(parse_chord("   ").is_err()); // empty chord
    }

    // ── CONFIG / EFFECTIVE KEYMAP ──
    #[test]
    fn r214_no_overrides_equals_defaults() {
        let defaults = Keymap::default().into_sorted();
        let effective = Keymap::effective(&[]).unwrap();
        assert_eq!(
            format!("{:?}", defaults.bindings()),
            format!("{:?}", effective.bindings())
        );
    }

    #[test]
    fn r214_replacement_removes_all_builtins_and_records_user_source() {
        let overrides = [kb("browser", "open_storage_inspector", Some("F11"), false)];
        let km = Keymap::effective(&overrides).unwrap();
        let alt_u = parse_stroke("Alt+U").unwrap();
        assert!(
            !km.bindings()
                .iter()
                .any(|b| b.context == InputContext::Browser
                    && b.action.id() == ActionId::OpenStorageInspector
                    && b.sequence == vec![alt_u]),
            "old Alt+U binding must be gone"
        );
        let user_bindings: Vec<_> = km
            .bindings()
            .iter()
            .filter(|b| b.action.id() == ActionId::OpenStorageInspector)
            .collect();
        assert_eq!(user_bindings.len(), 1);
        assert_eq!(user_bindings[0].source, BindingSource::User);
        assert_eq!(
            user_bindings[0].sequence,
            vec![parse_stroke("F11").unwrap()]
        );
    }

    #[test]
    fn r214_disabled_removes_pair_completely() {
        let overrides = [kb("browser", "open_storage_inspector", None, true)];
        let km = Keymap::effective(&overrides).unwrap();
        assert!(
            !km.bindings()
                .iter()
                .any(|b| b.action.id() == ActionId::OpenStorageInspector
                    && b.context == InputContext::Browser)
        );
        assert_eq!(
            km.primary_binding_label(InputContext::Browser, ActionId::OpenStorageInspector),
            None,
            "unbound action must not fabricate the old shortcut"
        );
    }

    #[test]
    fn r214_duplicate_override_pair_rejected() {
        let overrides = [
            kb("browser", "open_smart_tree", Some("F11"), false),
            kb("browser", "open_smart_tree", Some("F12"), false),
        ];
        assert!(Keymap::effective(&overrides).is_err());
    }

    #[test]
    fn r214_invalid_context_action_xor_rules() {
        assert!(Keymap::effective(&[kb("help", "quit", Some("Q"), false)]).is_err());
        assert!(Keymap::effective(&[kb("browser", "not_an_action", Some("Q"), false)]).is_err());
        assert!(Keymap::effective(&[kb("browser", "quit", None, false)]).is_err());
        assert!(Keymap::effective(&[kb("browser", "quit", None, true)]).is_ok());
        assert!(Keymap::effective(&[kb("browser", "quit", Some(""), true)]).is_err());
        assert!(Keymap::effective(&[kb("browser", "quit", Some("Q"), true)]).is_err());
    }

    #[test]
    fn r214_non_bindable_browser_actions_rejected() {
        for action in ["up", "down", "enter", "back", "switch_pane", "refresh"] {
            assert!(
                Keymap::effective(&[kb("browser", action, Some("F9"), false)]).is_err(),
                "{action} must not be bindable"
            );
        }
        assert!(
            Keymap::effective(&[kb("browser", "confirm_remote_delete", Some("F9"), false)])
                .is_err()
        );
        assert!(Keymap::effective(&[kb("sync_preview", "quit", Some("Q"), false)]).is_err());
    }

    // ── CONFLICTS ──
    #[test]
    fn r214_exact_collision_same_context_rejected() {
        let overrides = [
            kb("browser", "open_jobs", Some("F11"), false),
            kb("browser", "open_smart_tree", Some("F11"), false),
        ];
        assert!(Keymap::effective(&overrides).is_err());
    }

    #[test]
    fn r214_prefix_collisions_rejected_both_directions() {
        let overrides = [
            kb("browser", "open_smart_tree", Some("Ctrl+X"), false),
            kb("browser", "preview_workspace_sync", Some("Ctrl+X P"), false),
        ];
        assert!(Keymap::effective(&overrides).is_err());

        let reversed = [overrides[1].clone(), overrides[0].clone()];
        assert!(Keymap::effective(&reversed).is_err());
    }

    #[test]
    fn r214_cross_context_sequence_reuse_allowed() {
        let overrides = [
            kb("browser", "open_smart_tree", Some("F11"), false),
            kb(
                "sync_preview",
                "close_workspace_sync_overlay",
                Some("F11"),
                false,
            ),
        ];
        assert!(Keymap::effective(&overrides).is_ok());
    }

    // ── BROWSER LEGACY SAFETY: behavioral tests live in src/tui.rs (binary-side). ──

    // ── SYNC FAIL-CLOSED + CTRL+S GUARD are behavioral in input_dispatch /
    //    browser_input and covered by the source-contract + unit tests there. ──

    #[test]
    fn r214_lookup_helpers_deterministic() {
        let km = Keymap::default();
        let label = km.primary_binding_label(InputContext::Browser, ActionId::ViewFile);
        assert_eq!(label.as_deref(), Some("F3"));
        // alias must never become the primary label:
        let labels = km.discoverable_bindings(InputContext::Browser, ActionId::BeginSymlink);
        assert!(!labels.is_empty());
        assert!(
            labels
                .iter()
                .all(|l| l.starts_with("Ctrl+X S") || !l.contains("Ctrl"))
        );
    }
}
