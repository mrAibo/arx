//! Parse ~/.ssh/config for host aliases, ProxyJump chains, and IdentityFile directives.
//! ponytail: line-by-line parser, no crate dependency. Supports Host/HostName/User/Port/IdentityFile/ProxyJump.
//! Include directives are resolved so ARX-managed entries (e.g. ~/.ssh/arx_hosts.conf) are discovered.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct SshHostEntry {
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<PathBuf>,
    pub proxy_jump: Option<String>,
    /// True when this entry was declared in an ARX-managed include file.
    pub managed: bool,
}

/// Resolve `~` to the user's home directory.
pub(crate) fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/root"))
}

/// Managed include path ARX installs into ~/.ssh/config.
pub fn managed_include_path() -> PathBuf {
    home_dir().join(".ssh").join("arx_hosts.conf")
}

/// Parse ~/.ssh/config and return alias → resolved host details.
/// Include directives are followed (bounded recursion, no shell expansion).
pub fn parse_ssh_config() -> BTreeMap<String, SshHostEntry> {
    let config_path = home_dir().join(".ssh").join("config");
    parse_config_file(&config_path, &mut BTreeSet::new())
}

/// Internal: parse one config file, following Include directives.
/// `seen` guards against include cycles.
fn parse_config_file(path: &Path, seen: &mut BTreeSet<PathBuf>) -> BTreeMap<String, SshHostEntry> {
    let mut hosts = BTreeMap::new();
    let canon = match path.canonicalize() {
        Ok(p) => p,
        Err(_) => return hosts,
    };
    if !seen.insert(canon.clone()) {
        return hosts; // cycle guard
    }
    let content = match std::fs::read_to_string(&canon) {
        Ok(c) => c,
        Err(_) => return hosts,
    };

    let managed = canon.ends_with("arx_hosts.conf");

    let mut current_aliases: Vec<String> = Vec::new();
    let mut current_entry = SshHostEntry {
        managed,
        ..Default::default()
    };
    let base_dir = canon
        .parent()
        .unwrap_or_else(|| Path::new("/"))
        .to_path_buf();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let (keyword, value) = match trimmed.split_once(char::is_whitespace) {
            Some((k, v)) => (k.to_lowercase(), v.trim().to_string()),
            None => continue,
        };

        match keyword.as_str() {
            "host" => {
                flush_entry(&mut hosts, &current_aliases, &current_entry);
                current_aliases = value
                    .split_whitespace()
                    .filter(|a| !a.contains('*') && !a.contains('?'))
                    .map(|a| a.to_lowercase())
                    .collect();
                current_entry = SshHostEntry {
                    managed,
                    ..Default::default()
                };
            }
            "hostname" => current_entry.hostname = Some(value),
            "user" => current_entry.user = Some(value),
            "port" => {
                if let Ok(p) = value.parse::<u16>() {
                    current_entry.port = Some(p);
                }
            }
            "identityfile" => {
                let expanded = expand_home(&value);
                current_entry.identity_file = Some(PathBuf::from(expanded));
            }
            "proxyjump" => current_entry.proxy_jump = Some(value),
            "include" => {
                // B3: safe Include resolution. Expand ~, follow relative paths
                // against the current file's directory, bounded by `seen`.
                for inc in value.split_whitespace() {
                    let inc_path = resolve_include_path(inc, &base_dir);
                    let included = parse_config_file(&inc_path, seen);
                    for (a, e) in included {
                        hosts.entry(a).or_insert(e);
                    }
                }
            }
            _ => {}
        }
    }
    flush_entry(&mut hosts, &current_aliases, &current_entry);
    hosts
}

/// Expand a leading `~` to $HOME. No glob/shell expansion (read-only parse).
fn expand_home(value: &str) -> String {
    if let Some(rest) = value.strip_prefix("~/") {
        format!("{}/{}", home_dir().display(), rest)
    } else if value == "~" {
        home_dir().display().to_string()
    } else {
        value.to_string()
    }
}

/// Resolve an Include path: ~ expansion, relative-to-base resolution.
fn resolve_include_path(inc: &str, base_dir: &Path) -> PathBuf {
    let expanded = expand_home(inc);
    let p = PathBuf::from(expanded);
    if p.is_absolute() { p } else { base_dir.join(p) }
}

fn flush_entry(
    hosts: &mut BTreeMap<String, SshHostEntry>,
    aliases: &[String],
    entry: &SshHostEntry,
) {
    if aliases.is_empty() || entry.hostname.is_none() {
        return;
    }
    for alias in aliases {
        hosts.entry(alias.clone()).or_insert_with(|| entry.clone());
    }
}

/// Resolve effective SSH config via `ssh -G` (handles all OpenSSH features).
/// ponytail: preferred over ARX parser for actual connections.
/// Returns (hostname, port, user, identity_file_path, proxy_jump).
#[allow(clippy::type_complexity)]
pub fn resolve_effective(
    alias: &str,
) -> io::Result<(String, u16, String, Option<PathBuf>, Option<String>)> {
    super::validate_ssh_alias(alias)?;
    let output = std::process::Command::new("ssh")
        .args(["-G", alias])
        .output()
        .map_err(|e| io::Error::other(format!("ssh -G {alias}: {e}")))?;

    if !output.status.success() {
        return Err(io::Error::other(format!(
            "ssh -G {alias} exited {}",
            output.status
        )));
    }

    let out = String::from_utf8_lossy(&output.stdout);
    let mut hostname = alias.to_string();
    let mut port = 22u16;
    let mut user = std::env::var("USER").unwrap_or_else(|_| "root".into());
    let mut identity_file: Option<PathBuf> = None;
    let mut proxy_jump: Option<String> = None;

    for line in out.lines() {
        let line = line.trim();
        if let Some((k, v)) = line.split_once(char::is_whitespace) {
            match k.to_lowercase().as_str() {
                "hostname" => hostname = v.to_string(),
                "port" => {
                    if let Ok(p) = v.parse() {
                        port = p;
                    }
                }
                "user" => user = v.to_string(),
                "identityfile" => identity_file = Some(PathBuf::from(v)),
                "proxyjump" => proxy_jump = Some(v.to_string()),
                _ => {}
            }
        }
    }

    Ok((hostname, port, user, identity_file, proxy_jump))
}

/// Resolve via ARX parser (fast, for discovery/listing). Falls back to ssh -G on failure.
pub fn resolve_alias(alias: &str) -> (String, u16, String, Option<PathBuf>, Option<String>) {
    // Try ssh -G first for accurate config
    if let Ok(effective) = resolve_effective(alias) {
        return effective;
    }
    // Fall back to ARX parser
    let config = parse_ssh_config();
    let alias_lower = alias.to_lowercase();
    let user = std::env::var("USER").unwrap_or_else(|_| "root".into());

    if let Some(entry) = config.get(&alias_lower) {
        (
            entry.hostname.clone().unwrap_or_else(|| alias.to_string()),
            entry.port.unwrap_or(22),
            entry.user.clone().unwrap_or(user),
            entry.identity_file.clone(),
            entry.proxy_jump.clone(),
        )
    } else {
        (alias.to_string(), 22, user, None, None)
    }
}
