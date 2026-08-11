//! Input routing for ARX.
//!
//! This module translates physical keyboard events into stable application
//! actions. It deliberately does not mutate `AppState` and does not perform
//! I/O. That separation lets keyboard shortcuts, Command Center, mouse
//! actions, Which-Key, recipes, and future plugins share the same action
//! vocabulary.

mod hints;
mod keymap;

pub use hints::{
    ContextHint, HintPriority, command_bar_rows, contextual_hints,
    contextual_hints_with_file_context,
};
pub use keymap::{KeyBinding, KeyContinuation, KeyResolution, KeyRouter, KeyStroke, Keymap};
