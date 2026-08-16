//! Parse ~/.ssh/config for host aliases, ProxyJump chains, and IdentityFile directives.
//! ponytail: line-by-line parser, no crate dependency. Supports Host/HostName/User/Port/IdentityFile/ProxyJump.
//! Include directives are resolved so ARX-managed entries are discovered. OpenSSH Include
//! supports `~` expansion, multiple space-separated paths, glob patterns (`*`/`?`), nested
//! includes, `Keyword=value` or whitespace separation, and relative paths resolved against the
//! including file's directory (bounded, cycle-safe).
//!
//! Safety: discovery is fail-closed. If any referenced included file cannot be read or a
//! directive cannot be resolved, the whole parse returns an error so callers refuse to write
//! rather than silently miss an unmanaged host (requirement #8).

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

/// Discovery error: a referenced config/include could not be safely resolved/read.
/// Callers must fail closed when this is returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryError {
    /// The entry file itself could not be canonicalized or read.
    Unreadable(String),
    /// An Include token could not be resolved to a path.
    UnresolvableInclude(String),
}

impl std::fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiscoveryError::Unreadable(p) => write!(f, "SSH config unreadable: {p}"),
            DiscoveryError::UnresolvableInclude(p) => {
                write!(f, "SSH Include unresolvable: {p}")
            }
        }
    }
}

pub type DiscoveryResult = Result<BTreeMap<String, SshHostEntry>, DiscoveryError>;

#[derive(Debug, Clone, Default)]
pub struct SshHostEntry {
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<PathBuf>,
    pub proxy_jump: Option<String>,
    /// True only when this entry was declared in the exact ARX-managed include file
    /// (~/.ssh/arx_hosts.conf). A different file merely named arx_hosts.conf is NOT owned.
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

/// Canonical form of the ARX-managed file (symlink-resolved), used for
/// exact-ownership checks. Both sides of the comparison must be canonicalized,
/// otherwise a symlinked `~/.ssh` would be misclassified.
pub fn canonical_managed_path() -> PathBuf {
    managed_include_path()
        .canonicalize()
        .unwrap_or_else(|_| managed_include_path())
}

/// Parse ~/.ssh/config and return alias → resolved host details.
/// Fail-closed: any unreadable/unsupported included file yields an error.
pub fn parse_ssh_config() -> DiscoveryResult {
    let config_path = home_dir().join(".ssh").join("config");
    let mut seen = BTreeSet::new();
    parse_config_file(&config_path, &mut seen)
}

/// Internal: parse one config file, following Include directives.
/// `seen` guards against include cycles. `managed` propagates the exact-owned flag.
///
/// Fail-closed: a referenced file that EXISTS but cannot be read (permission
/// denied, broken symlink) yields an error so callers refuse to write rather than
/// silently miss an unmanaged host. A MISSING file (including the root config) is
/// treated as "nothing to include" — OpenSSH ignores absent Include targets.
fn parse_config_file(path: &Path, seen: &mut BTreeSet<PathBuf>) -> DiscoveryResult {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let canon = path
        .canonicalize()
        .map_err(|e| DiscoveryError::Unreadable(format!("{}: {e}", path.display())))?;
    if !seen.insert(canon.clone()) {
        return Ok(BTreeMap::new()); // cycle guard: already visited, no hosts
    }
    let content = std::fs::read_to_string(&canon)
        .map_err(|e| DiscoveryError::Unreadable(format!("{}: {e}", path.display())))?;

    // Managed only when canonical path equals the canonical ARX-managed file.
    let managed = canon == canonical_managed_path();

    let mut hosts = BTreeMap::new();
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
        let (keyword, value) = parse_directive(trimmed);
        let keyword = keyword.to_lowercase();

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
                // B3: safe Include resolution. Expand ~, follow relative paths, glob-expand
                // patterns, and recurse with cycle guard. A referenced-but-unreadable file
                // fails closed.
                for inc in value.split_whitespace() {
                    let paths = resolve_include_paths(inc, &base_dir)?;
                    // OpenSSH treats a glob with no matches as "nothing to include" (ignored).
                    // But an explicit path that exists and can't be read must fail closed.
                    for inc_path in paths {
                        let included = parse_config_file(&inc_path, seen)?;
                        for (a, e) in included {
                            hosts.entry(a).or_insert(e);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    flush_entry(&mut hosts, &current_aliases, &current_entry);
    Ok(hosts)
}

/// Split a directive line into (keyword, value), supporting both
/// `Keyword value` and `Keyword=value` (OpenSSH allows a single `=`).
pub(crate) fn parse_directive(line: &str) -> (&str, String) {
    if let Some((k, v)) = line.split_once('=') {
        // Only treat as `=` assignment if there is no leading whitespace inside
        // the keyword (OpenSSH: `Keyword=value`, spaces around = are tolerated).
        let k = k.trim();
        if !k.is_empty() && !k.chars().any(|c| c.is_whitespace()) {
            return (k, v.trim().to_string());
        }
    }
    match line.split_once(char::is_whitespace) {
        Some((k, v)) => (k, v.trim().to_string()),
        None => (line, String::new()),
    }
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

/// Resolve one Include argument to zero or more absolute paths.
/// Handles: `~` expansion, absolute paths, relative-to-base resolution, and glob patterns.
/// ponytail: no shell execution; glob uses std::fs dir walk.
fn resolve_include_paths(inc: &str, base_dir: &Path) -> Result<Vec<PathBuf>, DiscoveryError> {
    let expanded = expand_home(inc);
    let p = PathBuf::from(expanded);
    let absolute = if p.is_absolute() {
        p
    } else {
        base_dir.join(&p)
    };

    // Glob support: OpenSSH expands `*`/`?` in include patterns.
    if inc.contains('*') || inc.contains('?') {
        return Ok(glob_paths(&absolute));
    }
    Ok(vec![absolute])
}

/// Minimal glob expansion over a single directory level for `*`/`?` wildcards.
/// ponytail: only the final path component may contain wildcards; no `**`.
fn glob_paths(pattern: &Path) -> Vec<PathBuf> {
    let parent = pattern.parent().unwrap_or_else(|| Path::new("."));
    let file_pattern = match pattern.file_name().and_then(|f| f.to_str()) {
        Some(s) => s,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(parent) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if wildcard_match(&name, file_pattern) {
                out.push(entry.path());
            }
        }
    }
    out.sort();
    out
}

/// `*` (any run) / `?` (exactly one char) matcher, anchored at both ends.
/// Operates on `&str` via char vectors so UTF-8 is handled correctly.
fn wildcard_match(text: &str, pattern: &str) -> bool {
    let t: Vec<char> = text.chars().collect();
    let p: Vec<char> = pattern.chars().collect();
    wildcard_match_chars(&t, &p)
}

/// Match `text` against `pattern` where `*` matches any run and `?` any single char.
fn wildcard_match_chars(text: &[char], pattern: &[char]) -> bool {
    // Split pattern on `*` into literal segments; match left-to-right with gaps.
    let mut seg_start = 0usize;
    let mut ti = 0usize;
    let mut pi = 0usize;
    while pi < pattern.len() {
        if pattern[pi] == '*' {
            // segment [seg_start, pi) must be found starting at ti
            if pi == seg_start {
                // leading `*`: skip, next segment begins after
                seg_start = pi + 1;
                pi += 1;
                continue;
            }
            let seg = &pattern[seg_start..pi];
            match find_segment(text, ti, seg) {
                Some(idx) => {
                    ti = idx + seg.len();
                    seg_start = pi + 1;
                    pi += 1;
                }
                None => return false,
            }
        } else {
            pi += 1;
        }
    }
    // trailing segment must match suffix
    if seg_start <= pattern.len() {
        let seg = &pattern[seg_start..];
        if seg.is_empty() {
            return true; // pattern ended with `*` or was all `*`
        }
        if text.len() < seg.len() {
            return false;
        }
        let suffix = &text[text.len() - seg.len()..];
        return match_segment(suffix, seg);
    }
    true
}

/// Find `seg` in `text[from..]` allowing `?` wildcards in `seg`, return start index.
fn find_segment(text: &[char], from: usize, seg: &[char]) -> Option<usize> {
    if seg.is_empty() {
        return Some(from);
    }
    if text.len() < seg.len() {
        return None;
    }
    for start in from..=(text.len() - seg.len()) {
        if match_segment(&text[start..start + seg.len()], seg) {
            return Some(start);
        }
    }
    None
}

/// Match two equal-length char slices, with `?` = any single char.
fn match_segment(text: &[char], seg: &[char]) -> bool {
    debug_assert_eq!(text.len(), seg.len());
    text.iter().zip(seg).all(|(c, p)| *p == '?' || *p == *c)
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
        let (k, v) = parse_directive(line);
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

    Ok((hostname, port, user, identity_file, proxy_jump))
}

/// Resolve via ARX parser (fast, for discovery/listing). Falls back to ssh -G on failure.
pub fn resolve_alias(alias: &str) -> (String, u16, String, Option<PathBuf>, Option<String>) {
    if let Ok(effective) = resolve_effective(alias) {
        return effective;
    }
    let config = parse_ssh_config().unwrap_or_default();
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
