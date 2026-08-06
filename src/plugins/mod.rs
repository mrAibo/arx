//! Plugin system hooks — stub.
//! ponytail: full Lua/WASM integration deferred until mlua added to deps.
//! When ready: load .lua files from ~/.config/arx/plugins/, expose hook points.

use std::path::Path;

pub type PreviewFn = dyn Fn(&Path) -> Option<Vec<String>> + Send;
pub type FilterFn = dyn Fn(&str, &str) -> bool + Send;
pub type ActionFn = dyn Fn(&str, &[String]) -> Option<String> + Send;

/// Plugin hook points available for Lua scripts.
#[allow(dead_code)]
pub enum Hook {
    OnPreview(Box<PreviewFn>),
    OnFilter(Box<FilterFn>),
    OnAction(Box<ActionFn>),
}

/// Plugin registry (empty — Lua backend not yet loaded).
#[derive(Default)]
pub struct PluginRegistry {
    pub preview_hooks: Vec<Box<PreviewFn>>,
}

impl PluginRegistry {
    pub fn load_plugins(&mut self) {
        // ponytail: walk ~/.config/arx/plugins/*.lua, load via mlua
    }

    pub fn run_preview_hooks(&self, path: &Path) -> Option<Vec<String>> {
        for hook in &self.preview_hooks {
            if let Some(result) = hook(path) {
                return Some(result);
            }
        }
        None
    }
}
