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
    /// Editor command (overrides $EDITOR/$VISUAL). Example: "hx" or "nano".
    #[serde(default)]
    pub editor: Option<String>,
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
                editor: None,
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
                eprintln!("arx: cannot read config {}: {e}", path.display());
                ArxConfig::default()
            }
        }
    } else {
        ArxConfig::default()
    }
}

// ponytail: single well-known path; add XDG_CONFIG_HOME override when needed
fn config_path() -> std::path::PathBuf {
    dirs::config_dir()
        .map(|d| d.join("arx").join("arx.toml"))
        .unwrap_or_else(|| std::path::PathBuf::from("arx.toml"))
}

// ponytail: S3 data model only — no ArxConfig field, no validation (S3-06),
// no secrets, no client. bucket/endpoint are not normalized here.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct S3TargetConfig {
    /// Unique target id used by ARX location addressing. Not a secret.
    pub id: String,
    /// Human-readable name for the pane/UI.
    pub name: String,
    /// Bucket to bind the target to. None = whole-account/target root.
    /// Never use `Some("")` as a root sentinel.
    #[serde(default)]
    pub bucket: Option<String>,
    /// S3 region. None = SDK/provider default resolution.
    #[serde(default)]
    pub region: Option<String>,
    /// AWS profile name (shared credentials file). None = default chain.
    #[serde(default)]
    pub profile: Option<String>,
    /// Custom endpoint (MinIO / other S3-compatible). None = AWS default.
    #[serde(default)]
    pub endpoint_url: Option<String>,
    /// Path-style addressing for non-AWS S3-compatible stores.
    #[serde(default)]
    pub force_path_style: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    // helper: TOML root is always a table, so a bare Vec needs a wrapper
    #[derive(Deserialize)]
    struct Targets {
        targets: Vec<S3TargetConfig>,
    }

    #[test]
    fn t1_minimal_target() {
        let t: S3TargetConfig = toml::from_str(
            r#"
id = "aws"
name = "AWS"
"#,
        )
        .expect("valid toml");
        assert_eq!(t.id, "aws");
        assert_eq!(t.name, "AWS");
        assert_eq!(t.bucket, None);
        assert_eq!(t.region, None);
        assert_eq!(t.profile, None);
        assert_eq!(t.endpoint_url, None);
        assert!(!t.force_path_style);
    }

    #[test]
    fn t2_bucket_bound_target() {
        let t: S3TargetConfig = toml::from_str(
            r#"
id = "prod"
name = "Prod"
bucket = "company-artifacts"
"#,
        )
        .expect("valid toml");
        assert_eq!(t.id, "prod");
        assert_eq!(t.name, "Prod");
        assert_eq!(t.bucket.as_deref(), Some("company-artifacts"));
        assert_eq!(t.region, None);
        assert!(!t.force_path_style);
    }

    #[test]
    fn t3_minio_style_target() {
        let t: S3TargetConfig = toml::from_str(
            r#"
id = "minio"
name = "MinIO"
endpoint_url = "http://127.0.0.1:9000"
force_path_style = true
"#,
        )
        .expect("valid toml");
        assert_eq!(t.id, "minio");
        assert_eq!(t.name, "MinIO");
        assert_eq!(t.endpoint_url.as_deref(), Some("http://127.0.0.1:9000"));
        assert!(t.force_path_style);
        assert_eq!(t.bucket, None);
    }

    #[test]
    fn t4_multiple_targets() {
        let wrapped: Targets = toml::from_str(
            r#"
[[targets]]
id = "aws"
name = "AWS"

[[targets]]
id = "minio"
name = "MinIO"
endpoint_url = "http://127.0.0.1:9000"
force_path_style = true
"#,
        )
        .expect("valid toml");
        let targets = wrapped.targets;
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].id, "aws");
        assert_eq!(targets[1].id, "minio");
        assert!(targets[1].force_path_style);
        assert_ne!(targets[0].id, targets[1].id);
    }
}
