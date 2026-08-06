//! Plugin system — Lua scripts loaded from ~/.config/arx/plugins/.
use std::path::Path;

pub type PreviewFn = dyn Fn(&Path) -> Option<Vec<String>> + Send;
pub type FilterFn = dyn Fn(&str, &str) -> bool + Send;

pub enum Hook {
    OnPreview(Box<PreviewFn>),
    OnFilter(Box<FilterFn>),
}

pub struct PluginRegistry {
    pub preview_hooks: Vec<Box<PreviewFn>>,
    pub filter_hooks: Vec<Box<FilterFn>>,
    lua: mlua::Lua,
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            preview_hooks: Vec::new(),
            filter_hooks: Vec::new(),
            lua: mlua::Lua::new(),
        }
    }

    pub fn load_plugins(&mut self) {
        let plugin_dir = dirs::config_dir()
            .unwrap_or_else(|| Path::new("/tmp").to_path_buf())
            .join("arx")
            .join("plugins");
        if !plugin_dir.exists() {
            let _ = std::fs::create_dir_all(&plugin_dir);
            return;
        }
        let Ok(entries) = std::fs::read_dir(&plugin_dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("lua") {
                continue;
            }
            let Ok(source) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Err(e) = self.lua.load(&source).exec() {
                eprintln!("arx: plugin {} error: {e}", path.display());
            }
        }
    }

    pub fn eval(&self, code: &str) -> Result<String, String> {
        self.lua
            .load(code)
            .eval::<mlua::LuaString>()
            .map(|s| s.to_string_lossy())
            .map_err(|e| format!("{e}"))
    }
}
