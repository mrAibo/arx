use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ArxConfig {
    #[serde(default)]
    pub ui: UiConfig,
}

#[derive(Debug, Default, Deserialize)]
pub struct UiConfig {
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default)]
    pub show_hidden: bool,
}

fn default_theme() -> String {
    "dark".into()
}

impl Default for ArxConfig {
    fn default() -> Self {
        Self {
            ui: UiConfig {
                theme: default_theme(),
                show_hidden: false,
            },
        }
    }
}

pub fn load() -> ArxConfig {
    let path = config_path();
    if path.exists() {
        match std::fs::read_to_string(&path) {
            Ok(content) => toml::from_str(&content).unwrap_or_default(),
            Err(e) => {
                tracing::warn!("cannot read config {}: {e}", path.display());
                ArxConfig::default()
            }
        }
    } else {
        ArxConfig::default()
    }
}

// ponytail: single well-known path; add XDG_CONFIG_HOME override when needed
fn config_path() -> std::path::PathBuf {
    directories::ProjectDirs::from("", "", "arx")
        .map(|d| d.config_dir().join("arx.toml"))
        .unwrap_or_else(|| std::path::PathBuf::from("arx.toml"))
}
