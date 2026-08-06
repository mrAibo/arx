//! Plugin system — Lua scripts loaded from ~/.config/arx/plugins/.
//! ponytail: mlua runtime, one Lua VM shared across all plugins.
use std::path::Path;

pub type PreviewFn = dyn Fn(&Path) -> Option<Vec<String>> + Send;
pub type FilterFn = dyn Fn(&str, &str) -> bool + Send;

/// Plugin hook points.
pub enum Hook {
    OnPreview(Box<PreviewFn>),
    OnFilter(Box<FilterFn>),
}

/// Plugin registry with Lua runtime.
pub struct PluginRegistry {
    pub preview_hooks: Vec<Box<PreviewFn>>,
    pub filter_hooks: Vec<Box<FilterFn>>,
    lua: Option<mlua::Lua>,
}

impl Default for PluginRegistry {
    fn default() -> Self {
        // Safety: mlua requires Lua 5.4, send feature for cross-thread
        let lua = mlua::Lua::new().ok();
        Self {
            preview_hooks: Vec::new(),
            filter_hooks: Vec::new(),
            lua,
        }
    }
}

impl PluginRegistry {
    /// Load all .lua plugins from the plugin directory.
    pub fn load_plugins(&mut self) {
        let Some(ref lua) = self.lua else { return };
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
            if let Err(e) = lua.load(&source).exec() {
                eprintln!("arx: plugin {} error: {e}", path.display());
            }
        }
    }

    /// Execute an arbitrary Lua string in the plugin context.
    pub fn eval(&self, code: &str) -> Result<String, String> {
        let Some(ref lua) = self.lua else {
            return Err("Lua not available".into());
        };
        lua.load(code)
            .eval::<mlua::String>()
            .map(|s| s.to_str().unwrap_or("").to_string())
            .map_err(|e| format!("{e}"))
    }

    /// Register an API function callable from Lua.
    #[allow(dead_code)]
    pub fn register_api(&mut self, name: &str, f: impl Fn(Vec<String>) -> String + Send + 'static) {
        if let Some(ref lua) = self.lua {
            let _ = lua.globals().set(
                name,
                mlua::Function::create(lua, move |_, args: mlua::Variadic<String>| Ok(f(args))),
            );
        }
    }
}
