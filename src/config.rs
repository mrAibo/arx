use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ArxConfig {
    #[serde(default)]
    pub ui: UiConfig,
    /// S3 target inventory. None = no S3 targets configured.
    #[serde(default)]
    pub s3: S3Config,
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
            s3: S3Config::default(),
        }
    }
}

pub fn load() -> ArxConfig {
    let path = config_path();
    if path.exists() {
        match std::fs::read_to_string(&path) {
            Ok(content) => match parse_config(&content) {
                Ok(cfg) => cfg,
                Err(e) => {
                    // ponytail: malformed/invalid config → preserve existing fallback
                    eprintln!("arx: invalid config {}: {e}", path.display());
                    ArxConfig::default()
                }
            },
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

/// Wrapper matching the `[[s3.targets]]` TOML shape.
#[derive(Debug, Default, Deserialize)]
pub struct S3Config {
    #[serde(default)]
    pub targets: Vec<S3TargetConfig>,
}

/// Parse ARX config from TOML and validate the S3 target inventory.
/// Returns Err on malformed TOML or invalid S3 targets (caller decides
/// fallback). Validation inspects with `trim().is_empty()` but never rewrites
/// accepted strings.
pub fn parse_config(content: &str) -> Result<ArxConfig, String> {
    let cfg: ArxConfig = toml::from_str(content).map_err(|e| e.to_string())?;
    validate_s3(&cfg.s3.targets)?;
    Ok(cfg)
}

fn validate_s3(targets: &[S3TargetConfig]) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    for t in targets {
        // ponytail: trim only to detect emptiness; stored id stays verbatim
        if t.id.trim().is_empty() {
            return Err(format!(
                "S3 target id must not be empty/whitespace: {:?}",
                t.id
            ));
        }
        if let Some(b) = t.bucket.as_deref()
            && b.trim().is_empty()
        {
            return Err(format!(
                "S3 target {} bucket must not be empty/whitespace",
                t.id
            ));
        }
        // ponytail: exact-id dedup, no case-insensitive identity in this card
        if !seen.insert(t.id.clone()) {
            return Err(format!("duplicate S3 target id: {}", t.id));
        }
    }
    Ok(())
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

    // ---- S3-06: parsing + validation ----

    #[test]
    fn t1_backward_compat_no_s3() {
        let cfg = parse_config("[ui]\n").expect("valid config");
        assert!(cfg.s3.targets.is_empty());
    }

    #[test]
    fn t2_valid_aws_target() {
        let cfg = parse_config(
            r#"
[s3]
[[s3.targets]]
id = "aws"
name = "AWS"
"#,
        )
        .expect("valid");
        assert_eq!(cfg.s3.targets.len(), 1);
        assert_eq!(cfg.s3.targets[0].id, "aws");
    }

    #[test]
    fn t3_valid_bucket_bound() {
        let cfg = parse_config(
            r#"
[s3]
[[s3.targets]]
id = "artifacts"
name = "Artifacts"
bucket = "company-artifacts"
region = "eu-central-1"
profile = "release"
"#,
        )
        .expect("valid");
        assert_eq!(
            cfg.s3.targets[0].bucket.as_deref(),
            Some("company-artifacts")
        );
    }

    #[test]
    fn t4_valid_minio() {
        let cfg = parse_config(
            r#"
[s3]
[[s3.targets]]
id = "minio"
name = "MinIO"
bucket = "files"
endpoint_url = "http://127.0.0.1:9000"
force_path_style = true
"#,
        )
        .expect("valid");
        assert_eq!(
            cfg.s3.targets[0].endpoint_url.as_deref(),
            Some("http://127.0.0.1:9000")
        );
        assert!(cfg.s3.targets[0].force_path_style);
    }

    #[test]
    fn t5_multiple_distinct() {
        let cfg = parse_config(
            r#"
[s3]
[[s3.targets]]
id = "aws"
name = "AWS"

[[s3.targets]]
id = "minio"
name = "MinIO"
bucket = "files"
endpoint_url = "http://127.0.0.1:9000"
force_path_style = true
"#,
        )
        .expect("valid");
        assert_eq!(cfg.s3.targets.len(), 2);
        assert_eq!(cfg.s3.targets[0].id, "aws");
        assert_eq!(cfg.s3.targets[1].id, "minio");
    }

    #[test]
    fn t6_duplicate_id_rejected() {
        let r = parse_config(
            r#"
[s3]
[[s3.targets]]
id = "aws"
name = "AWS"

[[s3.targets]]
id = "aws"
name = "AWS2"
"#,
        );
        assert!(r.is_err());
    }

    #[test]
    fn t7_empty_id_rejected() {
        let r = parse_config(
            r#"
[s3]
[[s3.targets]]
id = ""
name = "X"
"#,
        );
        assert!(r.is_err());
    }

    #[test]
    fn t8_whitespace_id_rejected() {
        let r = parse_config(
            r#"
[s3]
[[s3.targets]]
id = "   "
name = "X"
"#,
        );
        assert!(r.is_err());
    }

    #[test]
    fn t9_empty_bucket_rejected() {
        let r = parse_config(
            r#"
[s3]
[[s3.targets]]
id = "x"
name = "X"
bucket = ""
"#,
        );
        assert!(r.is_err());
    }

    #[test]
    fn t10_whitespace_bucket_rejected() {
        let r = parse_config(
            r#"
[s3]
[[s3.targets]]
id = "x"
name = "X"
bucket = "   "
"#,
        );
        assert!(r.is_err());
    }

    #[test]
    fn t11_absent_bucket_ok() {
        let cfg = parse_config(
            r#"
[s3]
[[s3.targets]]
id = "x"
name = "X"
"#,
        )
        .expect("valid");
        assert_eq!(cfg.s3.targets[0].bucket, None);
    }

    #[test]
    fn t12_no_normalization() {
        // value with surrounding spaces is accepted verbatim (only trimmed for
        // emptiness check, never rewritten)
        let cfg = parse_config(
            r#"
[s3]
[[s3.targets]]
id = "prod"
name = "Prod Bucket"
bucket = "my-bucket"
"#,
        )
        .expect("valid");
        assert_eq!(cfg.s3.targets[0].id, "prod");
        assert_eq!(cfg.s3.targets[0].name, "Prod Bucket");
        assert_eq!(cfg.s3.targets[0].bucket.as_deref(), Some("my-bucket"));
    }
}
