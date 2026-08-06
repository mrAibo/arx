//! Parse ~/.ssh/config for host aliases, ProxyJump chains, and IdentityFile directives.
//! ponytail: line-by-line parser, no crate dependency. Supports Host/HostName/User/Port/IdentityFile/ProxyJump.

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct SshHostEntry {
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<PathBuf>,
    pub proxy_jump: Option<String>,
}

/// Parse ~/.ssh/config and return alias → resolved host details.
pub fn parse_ssh_config() -> BTreeMap<String, SshHostEntry> {
    let config_path = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/root"))
        .join(".ssh")
        .join("config");
    let content = match std::fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(_) => return BTreeMap::new(),
    };

    let mut hosts = BTreeMap::new();
    let mut current_aliases: Vec<String> = Vec::new();
    let mut current_entry = SshHostEntry::default();

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
                current_entry = SshHostEntry::default();
            }
            "hostname" => current_entry.hostname = Some(value),
            "user" => current_entry.user = Some(value),
            "port" => {
                if let Ok(p) = value.parse::<u16>() {
                    current_entry.port = Some(p);
                }
            }
            "identityfile" => {
                let expanded = value.replacen('~', &std::env::var("HOME").unwrap_or_default(), 1);
                current_entry.identity_file = Some(PathBuf::from(expanded));
            }
            "proxyjump" => current_entry.proxy_jump = Some(value),
            _ => {}
        }
    }
    flush_entry(&mut hosts, &current_aliases, &current_entry);
    hosts
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
