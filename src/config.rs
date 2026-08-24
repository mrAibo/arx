use serde::Deserialize;

// §12: single authoritative source for transfer concurrency bounds. The
// scheduler (`transfer_queue::TransferQueueConfig`) and config validation
// both reference these, so drift between them is impossible.
use crate::transfer_queue::{DEFAULT_TRANSFER_CONCURRENCY, MAX_TRANSFER_CONCURRENCY};

#[derive(Debug, Deserialize)]
pub struct ArxConfig {
    #[serde(default)]
    pub ui: UiConfig,
    /// S3 target inventory. None = no S3 targets configured.
    #[serde(default)]
    pub s3: S3Config,
    /// WebDAV target inventory. None = no WebDAV targets configured.
    #[serde(default)]
    pub webdav: WebDavConfig,
    /// Transfer queue tuning. Bounds/fallbacks live in `validate_transfer`.
    #[serde(default)]
    pub transfer: TransferConfig,
    /// User keybinding overrides (#214). Raw strings only; parsing/validation
    /// and conflict detection happen in the effective-keymap builder.
    #[serde(default)]
    pub keybindings: Vec<KeybindingConfig>,
}

/// One raw user keybinding override row (#214).
///
/// Exactly one of `keys` (non-empty sequence) or `disabled = true` is
/// required. Physical routing conflicts are NOT decided here — that belongs
/// to effective-keymap construction.
#[derive(Debug, Clone, Deserialize)]
pub struct KeybindingConfig {
    pub context: String,
    pub action: String,
    #[serde(default)]
    pub keys: Option<String>,
    #[serde(default)]
    pub disabled: bool,
}

/// Transfer queue configuration.
///
/// `concurrency` bounds the number of simultaneous transfer jobs. It mirrors
/// `transfer_queue::TransferQueueConfig` limits: `1..=8`, default `2`.
/// Out-of-range or zero values are rejected by `validate_transfer`, which
/// makes `load()` fall back to `ArxConfig::default()` (concurrency 2) for the
/// whole config — consistent with the existing S3/WebDAV validation contract.
#[derive(Debug, Deserialize)]
pub struct TransferConfig {
    #[serde(default = "default_transfer_concurrency")]
    pub concurrency: usize,
}

impl Default for TransferConfig {
    fn default() -> Self {
        Self {
            concurrency: DEFAULT_TRANSFER_CONCURRENCY,
        }
    }
}

fn default_transfer_concurrency() -> usize {
    DEFAULT_TRANSFER_CONCURRENCY
}

impl TransferConfig {
    pub fn resolve(&self) -> usize {
        if (1..=MAX_TRANSFER_CONCURRENCY).contains(&self.concurrency) {
            self.concurrency
        } else {
            DEFAULT_TRANSFER_CONCURRENCY
        }
    }
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
            webdav: WebDavConfig::default(),
            transfer: TransferConfig::default(),
            keybindings: Vec::new(),
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

/// Strictly load the config from an EXPLICIT path (#214).
///
/// Unlike [`load`], a missing/unreadable/malformed file is a hard error: when
/// the user names a file we must never silently substitute defaults.
pub fn load_from_path(path: &std::path::Path) -> Result<ArxConfig, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read config {}: {e}", path.display()))?;
    parse_config(&content).map_err(|e| format!("invalid config {}: {e}", path.display()))
}

// ponytail: single well-known path; add XDG_CONFIG_HOME override when needed
fn config_path() -> std::path::PathBuf {
    dirs::config_dir()
        .map(|d| d.join("arx").join("arx.toml"))
        .unwrap_or_else(|| std::path::PathBuf::from("arx.toml"))
}

// ponytail: S3 target data model. Integrated into ArxConfig.s3 (S3-06) with
// validation (no normalization). No secrets, no AWS client. endpoint_url is
// stored opaque and redacted in Debug (see impl Debug below).
#[derive(Clone, PartialEq, Eq, Deserialize)]
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

// Manual Debug: redact endpoint_url so signed-query/userinfo credentials never
// leak through `{:?}` (target, S3Config, or ArxConfig). Stored value is
// untouched — this is output-only redaction, not normalization.
impl std::fmt::Debug for S3TargetConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3TargetConfig")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .field("profile", &self.profile)
            .field(
                "endpoint_url",
                &self.endpoint_url.as_ref().map(|_| "<configured>"),
            )
            .field("force_path_style", &self.force_path_style)
            .finish()
    }
}

/// Wrapper matching the `[[webdav.targets]]` TOML shape.
#[derive(Debug, Default, Deserialize)]
pub struct WebDavConfig {
    #[serde(default)]
    pub targets: Vec<WebDavTargetConfig>,
}

/// A single configured WebDAV target.
///
/// Secrets are NEVER stored here. The password is resolved at runtime from the
/// OS keyring (`src/keyring.rs`, keyed by `webdav:<id>`) or the
/// `ARX_WEBDAV_<ID>_PASSWORD` env var for tests. `auth` selects the wire
/// mechanism; only `basic` is implemented in the MVP (Digest DEFERRED).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct WebDavTargetConfig {
    /// Unique target id used by ARX location addressing. Not a secret.
    pub id: String,
    /// Human-readable name for the pane/UI.
    pub name: String,
    /// Absolute http(s) URL of the target root collection.
    pub url: String,
    /// Username for Basic auth.
    pub username: String,
    /// Auth mechanism. MVP supports `basic` only; anything else is rejected.
    #[serde(default = "default_webdav_auth")]
    pub auth: String,
}

fn default_webdav_auth() -> String {
    "basic".into()
}

/// Parse + validate the WebDAV target inventory (no normalization of URLs).
pub fn validate_webdav(targets: &[WebDavTargetConfig]) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    for t in targets {
        if t.id.trim().is_empty() {
            return Err("WebDAV target id must not be empty/whitespace".into());
        }
        if t.url.trim().is_empty() {
            return Err(format!(
                "WebDAV target {} url must not be empty",
                sanitize_diag(&t.id)
            ));
        }
        // Absolute http/https only; no embedded userinfo/credentials in the URL.
        let u = t.url.trim();
        if !(u.starts_with("http://") || u.starts_with("https://")) {
            return Err(format!(
                "WebDAV target {} url must be absolute http(s): {}",
                sanitize_diag(&t.id),
                sanitize_diag(u)
            ));
        }
        if url_has_userinfo(u) {
            return Err(format!(
                "WebDAV target {} url must not embed credentials (userinfo)",
                sanitize_diag(&t.id)
            ));
        }
        if !["basic"].contains(&t.auth.as_str()) {
            return Err(format!(
                "WebDAV target {} auth '{}' not supported (MVP: basic only)",
                sanitize_diag(&t.id),
                sanitize_diag(&t.auth)
            ));
        }
        if !seen.insert(t.id.clone()) {
            return Err(format!(
                "duplicate WebDAV target id: {}",
                sanitize_diag(&t.id)
            ));
        }
    }
    Ok(())
}

/// Output-only check: does the URL contain a `user:pass@` userinfo segment?
/// Does not modify the stored value.
fn url_has_userinfo(url: &str) -> bool {
    // Strip scheme, then look for '@' before the first '/'.
    let without_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let authority = without_scheme
        .split_once('/')
        .map(|(a, _)| a)
        .unwrap_or(without_scheme);
    // A userinfo segment contains ':' before the '@' (user:pass@host).
    authority
        .rsplit_once('@')
        .map(|(userinfo, _)| userinfo.contains(':'))
        .unwrap_or(false)
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
    validate_webdav(&cfg.webdav.targets)?;
    validate_transfer(&cfg.transfer)?;
    Ok(cfg)
}

/// Validate the `[transfer]` section. Out-of-range `concurrency` (zero or
/// above `MAX_TRANSFER_CONCURRENCY`) is rejected so `load()` falls back to the
/// default config (concurrency 2), consistent with the S3/WebDAV contract.
pub fn validate_transfer(config: &TransferConfig) -> Result<(), String> {
    if (1..=MAX_TRANSFER_CONCURRENCY).contains(&config.concurrency) {
        Ok(())
    } else {
        Err(format!(
            "transfer.concurrency must be between 1 and {MAX_TRANSFER_CONCURRENCY}, got {}",
            config.concurrency
        ))
    }
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
                // ponytail: output-only sanitize; stored value untouched
                sanitize_diag(&t.id)
            ));
        }
        // ponytail: exact-id dedup, no case-insensitive identity in this card
        if !seen.insert(t.id.clone()) {
            return Err(format!("duplicate S3 target id: {}", sanitize_diag(&t.id)));
        }
    }
    Ok(())
}

/// Output-only sanitization for diagnostic text. Replaces control characters
/// (newline, tab, ESC/ANSI) so a hostile local config id cannot inject extra
/// terminal lines or control sequences. Stored config values are NEVER
/// modified by this — it only affects the rendered message.
// ponytail: no dependency needed; std char iteration covers the threat.
pub(crate) fn sanitize_diag(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { '�' } else { c })
        .collect()
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
        // surrounding whitespace is preserved verbatim (only trimmed for
        // emptiness check, never rewritten)
        let cfg = parse_config(
            r#"
[s3]
[[s3.targets]]
id = " prod "
name = "Prod Bucket"
bucket = " my-bucket "
"#,
        )
        .expect("valid");
        assert_eq!(cfg.s3.targets[0].id, " prod ");
        assert_eq!(cfg.s3.targets[0].name, "Prod Bucket");
        assert_eq!(cfg.s3.targets[0].bucket.as_deref(), Some(" my-bucket "));
    }

    // ---- S3-07: config redaction truth ----

    #[test]
    fn t_r1_non_normalization_whitespace() {
        // same as t12 but asserts exact whitespace preservation
        let cfg = parse_config(
            r#"
[s3]
[[s3.targets]]
id = " prod "
name = "Prod"
bucket = " my-bucket "
"#,
        )
        .expect("valid");
        assert_eq!(cfg.s3.targets[0].id, " prod ");
        assert_eq!(cfg.s3.targets[0].bucket.as_deref(), Some(" my-bucket "));
    }

    #[test]
    fn t_r2_validation_error_useful() {
        // ordinary id yields a readable message, not a panic
        let r = parse_config(
            r#"
[s3]
[[s3.targets]]
id = ""
name = "X"
"#,
        );
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("id"));
    }

    #[test]
    fn t_r3_newline_id_no_injection() {
        // sanitize_diag must remove a raw newline so a hostile id cannot inject
        // an extra terminal line in the diagnostic
        let sanitized = sanitize_diag("a\nb");
        assert!(
            !sanitized.contains('\n'),
            "diag must not contain raw newline: {sanitized:?}"
        );
    }

    #[test]
    fn t_r4_control_char_id_sanitized() {
        // ESC / control char in id is replaced in the diagnostic, not echoed
        let input = format!("bad{}\x1bid", 0x1b as char);
        let sanitized = sanitize_diag(&input);
        assert!(
            !sanitized.contains('\x1b'),
            "diag must not echo ESC: {sanitized:?}"
        );
    }

    #[test]
    fn t_r5_duplicate_error_safe_repr() {
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
        let msg = r.unwrap_err();
        // duplicate id is reported via sanitized repr; no raw control injection
        assert!(msg.contains("duplicate"));
        assert!(!msg.contains('\n'));
    }

    #[test]
    fn t_r6_debug_redacts_signed_query() {
        // T1: target Debug must not reveal signed-query credentials
        let t: S3TargetConfig = toml::from_str(
            r#"
id = "x"
name = "X"
endpoint_url = "https://example.invalid/?X-Amz-Signature=SUPERSECRET"
"#,
        )
        .expect("valid");
        let text = format!("{t:?}");
        assert!(!text.contains("SUPERSECRET"), "leak: {text}");
        assert!(!text.contains("X-Amz-Signature"), "leak: {text}");
        assert!(
            text.contains("<configured>"),
            "redaction marker missing: {text}"
        );
    }

    #[test]
    fn t_r7_debug_redacts_userinfo() {
        // T2: target Debug must not reveal userinfo credentials
        let t: S3TargetConfig = toml::from_str(
            r#"
id = "y"
name = "Y"
endpoint_url = "https://user:password@example.invalid/"
"#,
        )
        .expect("valid");
        let text = format!("{t:?}");
        assert!(!text.contains("user:password"), "leak: {text}");
        assert!(!text.contains("password@example"), "leak: {text}");
    }

    #[test]
    fn t_r9_s3config_debug_transitive() {
        // T3: S3Config Debug must not reveal endpoint secret
        let cfg = parse_config(
            r#"
[s3]
[[s3.targets]]
id = "x"
name = "X"
endpoint_url = "https://example.invalid/?X-Amz-Signature=SUPERSECRET"
"#,
        )
        .expect("valid");
        let text = format!("{:?}", cfg.s3);
        assert!(!text.contains("SUPERSECRET"), "leak: {text}");
        assert!(!text.contains("X-Amz-Signature"), "leak: {text}");
    }

    #[test]
    fn t_r10_arxconfig_debug_transitive() {
        // T4: ArxConfig Debug must not reveal endpoint secret
        let cfg = parse_config(
            r#"
[s3]
[[s3.targets]]
id = "x"
name = "X"
endpoint_url = "https://example.invalid/?X-Amz-Signature=SUPERSECRET"
"#,
        )
        .expect("valid");
        let text = format!("{cfg:?}");
        assert!(!text.contains("SUPERSECRET"), "leak: {text}");
        assert!(!text.contains("X-Amz-Signature"), "leak: {text}");
    }

    #[test]
    fn t_r11_storage_unchanged() {
        // T5: redaction must NOT mutate stored endpoint_url
        let cfg = parse_config(
            r#"
[s3]
[[s3.targets]]
id = "x"
name = "X"
endpoint_url = "https://example.invalid/?X-Amz-Signature=SUPERSECRET"
"#,
        )
        .expect("valid");
        assert_eq!(
            cfg.s3.targets[0].endpoint_url.as_deref(),
            Some("https://example.invalid/?X-Amz-Signature=SUPERSECRET")
        );
    }

    #[test]
    fn t_r12_endpoint_none_debug() {
        // T6: None endpoint renders clearly without leaking anything
        let t: S3TargetConfig = toml::from_str(
            r#"
id = "x"
name = "X"
"#,
        )
        .expect("valid");
        let text = format!("{t:?}");
        assert!(
            text.contains("endpoint_url: None"),
            "expected None marker: {text}"
        );
    }

    #[test]
    fn t_r8_normal_config_unchanged() {
        let cfg = parse_config(
            r#"
[s3]
[[s3.targets]]
id = "aws"
name = "AWS"
bucket = "company-artifacts"
"#,
        )
        .expect("valid");
        assert_eq!(cfg.s3.targets[0].id, "aws");
        assert_eq!(
            cfg.s3.targets[0].bucket.as_deref(),
            Some("company-artifacts")
        );
    }

    // ---- Transfer queue config ----

    #[test]
    fn transfer_concurrency_default_is_two() {
        let cfg = parse_config("[ui]\n").expect("valid");
        assert_eq!(cfg.transfer.concurrency, DEFAULT_TRANSFER_CONCURRENCY);
        assert_eq!(cfg.transfer.resolve(), DEFAULT_TRANSFER_CONCURRENCY);
    }

    #[test]
    fn transfer_concurrency_explicit_valid() {
        for n in [1usize, 2, 4, 8] {
            let toml = format!("[transfer]\nconcurrency = {n}\n");
            let cfg = parse_config(&toml).expect("valid");
            assert_eq!(cfg.transfer.concurrency, n);
            assert_eq!(cfg.transfer.resolve(), n);
        }
    }

    #[test]
    fn transfer_concurrency_zero_and_over_max_rejected() {
        for n in [0usize, 9, 100] {
            let toml = format!("[transfer]\nconcurrency = {n}\n");
            assert!(
                parse_config(&toml).is_err(),
                "concurrency {n} must be rejected"
            );
        }
    }

    #[test]
    fn transfer_concurrency_out_of_range_falls_back_to_default() {
        // whole-config fallback to ArxConfig::default() keeps concurrency 2
        let toml =
            "[transfer]\nconcurrency = 0\n[s3]\n[[s3.targets]]\nid = \"aws\"\nname = \"AWS\"\n";
        assert!(parse_config(toml).is_err());
    }
}
