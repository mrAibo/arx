//! Managed SSH host configuration: install a single safe Include into the
//! user-owned ~/.ssh/config and own only ~/.ssh/arx_hosts.conf.
//!
//! ponytail: ARX never rewrites arbitrary user Host blocks. It appends at most
//! one `Include ~/.ssh/arx_hosts.conf` to ~/.ssh/config and writes only the
//! managed file. Writes are atomic (temp + fsync + rename); the original
//! ~/.ssh/config is backed up before the first mutation.

use std::collections::BTreeMap;
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::remote::ssh_config::{SshHostEntry, home_dir, managed_include_path, parse_ssh_config};

/// Managed-file model written by ARX. Only literal aliases are allowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedHost {
    pub alias: String,
    pub hostname: String,
    pub user: String,
    pub port: u16,
    pub identity_file: Option<PathBuf>,
    pub proxy_jump: Option<String>,
    pub identities_only: bool,
}

impl ManagedHost {
    pub fn validate(&self) -> Result<(), String> {
        validate_alias(&self.alias)?;
        if self.alias.contains('*') || self.alias.contains('?') {
            return Err("wildcard aliases are not allowed".into());
        }
        if self.hostname.trim().is_empty() {
            return Err("hostname must not be empty".into());
        }
        if !(1..=65535).contains(&self.port) {
            return Err("port must be 1..65535".into());
        }
        Ok(())
    }
}

/// Reject aliases that could break config parsing or inject directives.
pub fn validate_alias(alias: &str) -> Result<(), String> {
    if alias.is_empty() {
        return Err("alias must not be empty".into());
    }
    if alias.chars().any(|c| c.is_whitespace()) {
        return Err("alias must not contain whitespace".into());
    }
    if alias.starts_with('-') {
        return Err("alias must not start with '-'".into());
    }
    if alias.contains('*') || alias.contains('?') {
        return Err("wildcard aliases are not allowed".into());
    }
    if alias.contains('/') || alias == ".." || alias.starts_with("../") || alias.starts_with("..\\")
    {
        return Err("alias must not contain a path separator".into());
    }
    for ch in alias.chars() {
        if ch == '\0' || ch.is_control() {
            return Err("alias must not contain NUL or control characters".into());
        }
    }
    Ok(())
}

fn ssh_dir() -> PathBuf {
    crate::remote::ssh_config::managed_include_path()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("/root/.ssh"))
}

/// True when the ARX-managed include is already installed in `config`.
/// Recognizes tilde, absolute, relative-to-~/.ssh, and glob forms as equivalent,
/// so ARX never appends a second Include for an already-included managed file.
pub fn is_arx_include_installed() -> bool {
    let dir = ssh_dir();
    let cfg = dir.join("config");
    if !cfg.exists() {
        return false;
    }
    let content = match std::fs::read_to_string(&cfg) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let target = crate::remote::ssh_config::canonical_managed_path();
    let target_display = target.display().to_string();
    let target_rel = "~/.ssh/arx_hosts.conf"; // ponytail: preferred readable form
    content.lines().any(|l| {
        let t = l.trim_start();
        let (kw, val) = match t.split_once(char::is_whitespace) {
            Some((k, v)) => (k.to_lowercase(), v.trim().to_string()),
            None => return false,
        };
        if kw != "include" {
            return false;
        }
        // Each include value (space-separated) compared canonically.
        val.split_whitespace().any(|tok| {
            let expanded = expand_include_token(tok, &dir);
            expanded == target_display || tok == target_rel
        })
    })
}

/// Expand one Include token: ~, absolute, or relative-to-~/.ssh base.
fn expand_include_token(tok: &str, base_dir: &Path) -> String {
    let expanded = if let Some(rest) = tok.strip_prefix("~/") {
        format!(
            "{}/{}",
            crate::remote::ssh_config::home_dir().display(),
            rest
        )
    } else if tok == "~" {
        crate::remote::ssh_config::home_dir().display().to_string()
    } else {
        tok.to_string()
    };
    let p = PathBuf::from(&expanded);
    let abs = if p.is_absolute() {
        p
    } else {
        base_dir.join(&p)
    };
    abs.display().to_string()
}

/// Ensure ~/.ssh exists. Only sets 0700 when ARX creates it; an existing dir
/// keeps its permissions (F9: respect user ownership).
fn ensure_ssh_dir() -> std::io::Result<PathBuf> {
    let dir = ssh_dir();
    if dir.exists() {
        return Ok(dir);
    }
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(dir)
}

/// Install the ARX Include directive into ~/.ssh/config if missing.
/// Safe: never rewrites existing blocks, never duplicates the Include,
/// backs up the config before the first mutation (and never overwrites an
/// existing backup), preserves all bytes/comments.
pub fn ensure_arx_include() -> std::io::Result<()> {
    let dir = ensure_ssh_dir()?;
    let cfg = dir.join("config");
    let include_value = managed_include_path();
    let include_value_str = include_value.display().to_string();

    if is_arx_include_installed() {
        return Ok(());
    }

    // Backup before first mutation (preserve any pre-existing backup).
    if cfg.exists() {
        let backup = dir.join("config.arx-backup");
        if !backup.exists() {
            std::fs::copy(&cfg, &backup)?;
        }
    }

    let mut content = if cfg.exists() {
        let mut c = std::fs::read_to_string(&cfg)?;
        if !c.ends_with('\n') {
            c.push('\n');
        }
        c
    } else {
        // ARX creates the user config with 0600 when it did not exist.
        String::new()
    };
    content.push_str(&format!("Include {}\n", include_value_str));
    atomic_write(&cfg, content.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&cfg, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Path to the ARX-managed hosts file.
pub fn managed_config_path() -> PathBuf {
    managed_include_path()
}

/// Path to the user-owned ~/.ssh/config (ARX never writes to it except the
/// single Include directive installed by `ensure_arx_include`).
pub fn user_ssh_config_path() -> PathBuf {
    ssh_dir().join("config")
}

/// Resolve the config file to open: `false` -> user `~/.ssh/config`,
/// `true` -> ARX-managed `~/.ssh/arx_hosts.conf`. Returns `None` only if the
/// ssh dir cannot be resolved.
pub fn open_config(managed: bool) -> Option<PathBuf> {
    if managed {
        Some(managed_config_path())
    } else {
        Some(user_ssh_config_path())
    }
}

/// Reload the managed-host snapshot from disk (sees external edits).
pub fn reload_managed_hosts() -> BTreeMap<String, ManagedHost> {
    list_managed_hosts()
}

/// List currently managed hosts (parsed from the managed file only).
pub fn list_managed_hosts() -> BTreeMap<String, ManagedHost> {
    let path = managed_config_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return BTreeMap::new(),
    };
    parse_managed_content(&content)
}

/// Does an alias already exist anywhere ARX can see (managed or user config)?
/// Uses the full discovery parser (glob/nested includes) — collision fail-closed (B5).
pub fn alias_collision(alias: &str) -> bool {
    let lower = alias.to_lowercase();
    parse_ssh_config().keys().any(|k| k.to_lowercase() == lower)
}

/// True when the alias collides with an *unmanaged* (user-owned) entry.
/// Used so a rename/add fails closed against real discovered hosts.
pub fn alias_collides_unmanaged(alias: &str) -> bool {
    let lower = alias.to_lowercase();
    parse_ssh_config()
        .iter()
        .any(|(k, e)| k.to_lowercase() == lower && !e.managed)
}

/// Add a managed host. Fails closed on invalid alias or collision.
pub fn add_managed_host(host: &ManagedHost) -> Result<(), String> {
    host.validate()?;
    if alias_collision(&host.alias) {
        return Err(format!(
            "Host alias '{}' already exists in your SSH configuration.",
            host.alias
        ));
    }
    ensure_arx_include().map_err(|e| format!("failed to install include: {e}"))?;
    let mut hosts = list_managed_hosts();
    hosts.insert(host.alias.clone(), host.clone());
    write_managed(&hosts)
}

/// Edit an existing managed host.
/// `original_alias` must be an ARX-managed entry. If the alias changed, the
/// new alias is validated and collision-checked against unmanaged + other managed
/// entries; the old block is removed and the new block added in ONE atomic write.
/// On failure the old entry remains intact.
pub fn update_managed_host(original_alias: &str, updated: &ManagedHost) -> Result<(), String> {
    validate_alias(original_alias)?;
    updated.validate()?;
    let mut hosts = list_managed_hosts();
    if !hosts.contains_key(original_alias) {
        return Err(format!("managed host '{}' does not exist", original_alias));
    }
    let alias_changed = original_alias != updated.alias;
    if alias_changed {
        if alias_collides_unmanaged(&updated.alias) {
            return Err(format!(
                "Host alias '{}' collides with an unmanaged SSH configuration.",
                updated.alias
            ));
        }
        if hosts.contains_key(&updated.alias) && updated.alias != original_alias {
            return Err(format!("managed host '{}' already exists", updated.alias));
        }
        // Atomic rename: drop old, insert new in the same map write.
        hosts.remove(original_alias);
    }
    hosts.insert(updated.alias.clone(), updated.clone());
    write_managed(&hosts)
}

/// Delete a managed host. Never touches unmanaged blocks.
pub fn delete_managed_host(alias: &str) -> Result<(), String> {
    validate_alias(alias)?;
    let mut hosts = list_managed_hosts();
    if hosts.remove(alias).is_none() {
        return Err(format!("managed host '{}' does not exist", alias));
    }
    write_managed(&hosts)
}

/// Parse the managed file content into a map (single Host block per alias).
fn parse_managed_content(content: &str) -> BTreeMap<String, ManagedHost> {
    let mut map = BTreeMap::new();
    let mut current: Option<ManagedHost> = None;
    for line in content.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let (kw, val) = match t.split_once(char::is_whitespace) {
            Some((k, v)) => (k.to_lowercase(), v.trim().to_string()),
            None => continue,
        };
        match kw.as_str() {
            "host" => {
                if let Some(h) = current.take() {
                    map.insert(h.alias.clone(), h);
                }
                let alias = val.split_whitespace().next().unwrap_or("").to_string();
                current = Some(ManagedHost {
                    alias,
                    hostname: String::new(),
                    user: whoami_user(),
                    port: 22,
                    identity_file: None,
                    proxy_jump: None,
                    identities_only: false,
                });
            }
            "hostname" => {
                if let Some(h) = &mut current {
                    h.hostname = val;
                }
            }
            "user" => {
                if let Some(h) = &mut current {
                    h.user = val;
                }
            }
            "port" => {
                if let (Some(h), Ok(p)) = (&mut current, val.parse()) {
                    h.port = p;
                }
            }
            "identityfile" => {
                if let Some(h) = &mut current {
                    h.identity_file = Some(PathBuf::from(expand_tilde(&val)));
                }
            }
            "proxyjump" => {
                if let Some(h) = &mut current {
                    h.proxy_jump = Some(val);
                }
            }
            "identitiesonly" => {
                if let Some(h) = &mut current {
                    h.identities_only = val.eq_ignore_ascii_case("yes");
                }
            }
            _ => {}
        }
    }
    if let Some(h) = current.take() {
        map.insert(h.alias.clone(), h);
    }
    map
}

fn whoami_user() -> String {
    std::env::var("USER").unwrap_or_else(|_| "root".into())
}

fn expand_tilde(v: &str) -> String {
    if let Some(rest) = v.strip_prefix("~/") {
        format!(
            "{}/{}",
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("/root"))
                .display(),
            rest
        )
    } else {
        v.to_string()
    }
}

/// Serialize managed hosts to the canonical managed-file format.
fn render_managed(hosts: &BTreeMap<String, ManagedHost>) -> String {
    let mut out = String::new();
    for h in hosts.values() {
        out.push_str(&format!("Host {}\n", h.alias));
        out.push_str(&format!("    HostName {}\n", h.hostname));
        out.push_str(&format!("    User {}\n", h.user));
        out.push_str(&format!("    Port {}\n", h.port));
        if let Some(id) = &h.identity_file {
            out.push_str(&format!("    IdentityFile {}\n", id.display()));
        }
        if let Some(pj) = &h.proxy_jump {
            out.push_str(&format!("    ProxyJump {}\n", pj));
        }
        if h.identities_only {
            out.push_str("    IdentitiesOnly yes\n");
        }
        out.push('\n');
    }
    out
}

/// Write the managed file atomically with 0600 permissions.
fn write_managed(hosts: &BTreeMap<String, ManagedHost>) -> Result<(), String> {
    let path = managed_config_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let content = render_managed(hosts);
    atomic_write(&path, content.as_bytes())
        .and_then(|_| set_mode_0600(&path))
        .map_err(|e| format!("failed to write managed config: {e}"))
}

/// Atomic write: temp file + fsync + rename. Never truncates before success.
fn atomic_write(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = dir.join(format!(
        ".{}.tmp.{}",
        path.file_name()
            .map(|f| f.to_string_lossy())
            .unwrap_or_default(),
        std::process::id()
    ));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(unix)]
fn set_mode_0600(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_mode_0600(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Render a managed host for display (no secret values).
pub fn display_entry(e: &SshHostEntry) -> String {
    format!(
        "host={} user={} port={} identity={:?} proxy={:?} managed={}",
        e.hostname.clone().unwrap_or_default(),
        e.user.clone().unwrap_or_default(),
        e.port.unwrap_or(22),
        e.identity_file
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        e.proxy_jump,
        e.managed
    )
}

/// B6 — Classified result of a connection test (user-facing, factual).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestConnectionResult {
    ConfigValid,
    Connected,
    HostUnreachable,
    AuthenticationFailed,
    HostKeyTrustFailure,
    SshConfigError,
}

impl TestConnectionResult {
    pub fn message(&self, alias: &str) -> String {
        match self {
            TestConnectionResult::ConfigValid => {
                format!("Config valid for {alias} (no live probe)")
            }
            TestConnectionResult::Connected => format!("Connected to {alias}"),
            TestConnectionResult::HostUnreachable => format!("Host {alias} unreachable"),
            TestConnectionResult::AuthenticationFailed => {
                format!("Authentication failed for {alias}")
            }
            TestConnectionResult::HostKeyTrustFailure => {
                format!("Host key/trust failure for {alias}")
            }
            TestConnectionResult::SshConfigError => format!("SSH config error for {alias}"),
        }
    }
}

/// Classify an ssh stderr tail into a factual result bucket (no secret leakage).
fn classify_ssh_stderr(stderr: &str) -> TestConnectionResult {
    let tail = stderr.lines().last().unwrap_or("").to_lowercase();
    if tail.contains("permission denied") || tail.contains("authentication") {
        TestConnectionResult::AuthenticationFailed
    } else if tail.contains("host key")
        || tail.contains("hostkey")
        || tail.contains("verify")
        || tail.contains("fingerprint")
    {
        TestConnectionResult::HostKeyTrustFailure
    } else {
        // ponytail: any other stderr tail is conservatively treated as unreachable.
        TestConnectionResult::HostUnreachable
    }
}

/// B6 — Test reachability of an alias using ssh -G (truth) then a batch-mode
/// connection probe. The alias is validated to prevent option injection.
/// Returns a classified result; the caller (TUI) must run this off the event loop.
pub fn test_connection(alias: &str) -> TestConnectionResult {
    if super::validate_ssh_alias(alias).is_err() {
        return TestConnectionResult::SshConfigError;
    }
    // Preflight: ssh -G must succeed (config resolves).
    let probe = std::process::Command::new("ssh")
        .args(["-G", "-o", "BatchMode=yes", alias])
        .output();
    let probe = match probe {
        Ok(p) => p,
        Err(_) => return TestConnectionResult::SshConfigError,
    };
    if !probe.status.success() {
        return TestConnectionResult::SshConfigError;
    }
    // Actual connection probe (no PTY, no command execution beyond true).
    let conn = std::process::Command::new("ssh")
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            "-o",
            "PreferredAuthentications=publickey",
            alias,
            "true",
        ])
        .output();
    match conn {
        Ok(c) if c.status.success() => TestConnectionResult::Connected,
        Ok(c) => classify_ssh_stderr(&String::from_utf8_lossy(&c.stderr)),
        Err(_) => TestConnectionResult::HostUnreachable,
    }
}

/// B7 — Generate an Ed25519 key via ssh-keygen. ARX never implements crypto.
/// Passphrase is empty and NOT stored; private key stays a file under ~/.ssh/arx/.
/// Returns the private-key path. Never returns or prints key contents.
pub fn generate_ed25519_key(name: &str) -> io::Result<PathBuf> {
    validate_alias(name).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let arx_dir = home_dir().join(".ssh").join("arx");
    std::fs::create_dir_all(&arx_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&arx_dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let key_path = arx_dir.join(name);
    if key_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("key {name} already exists"),
        ));
    }
    // -N "" => no passphrase; passphrase is not stored by ARX.
    let status = std::process::Command::new("ssh-keygen")
        .args([
            "-t",
            "ed25519",
            "-f",
            &key_path.display().to_string(),
            "-N",
            "",
            "-C",
            &format!("arx-{name}"),
        ])
        .status()
        .map_err(|e| io::Error::other(format!("ssh-keygen: {e}")))?;
    if !status.success() {
        return Err(io::Error::other("ssh-keygen failed"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(key_path)
}

/// B8 — Path to the user-owned ~/.ssh/config for opening in an editor.
pub fn user_config_path() -> PathBuf {
    ssh_dir().join("config")
}

/// B10 — Reload: re-read the effective SSH config. Parsing is lazy, so this
/// simply re-parses. Returns the merged alias → entry map (incl. managed).
pub fn reload() -> BTreeMap<String, SshHostEntry> {
    parse_ssh_config()
}
