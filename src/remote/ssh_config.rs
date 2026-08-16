//! Parse ~/.ssh/config for host aliases, ProxyJump chains, and IdentityFile directives.
//! ponytail: line-by-line parser, no crate dependency. Supports Host/HostName/User/Port/IdentityFile/ProxyJump.
//! Include directives are resolved so ARX-managed entries are discovered. OpenSSH Include
//! supports `~` expansion, multiple space-separated paths, glob patterns (`*`/`?`) in ANY path
//! component, nested includes, `Keyword=value` or whitespace separation, and relative paths
//! anchored to ~/.ssh for user configuration.
//!
//! Safety: discovery is fail-closed. If any referenced included file or directory cannot be
//! resolved/read (permission denied, broken symlink, unreadable glob directory), the whole parse
//! returns an error so callers refuse to write rather than silently miss an unmanaged host
//! (requirement #8). Only a genuinely MISSING file (NotFound) is treated as "nothing to include",
//! matching OpenSSH which ignores absent Include targets.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Component, Path, PathBuf};

/// Discovery error: a referenced config/include could not be safely resolved/read.
/// Callers must fail closed when this is returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryError {
    /// The entry file/directory itself could not be accessed (read/permission/canonicalize).
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

/// User SSH directory (~/.ssh). OpenSSH anchors every relative Include in user
/// configuration here, including nested includes.
pub(crate) fn ssh_dir() -> PathBuf {
    home_dir().join(".ssh")
}

/// Managed include path ARX installs into ~/.ssh/config.
pub fn managed_include_path() -> PathBuf {
    ssh_dir().join("arx_hosts.conf")
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
    let config_path = ssh_dir().join("config");
    let mut seen = BTreeSet::new();
    parse_config_file(&config_path, &mut seen)
}

/// Internal: parse one config file, following Include directives.
/// `seen` guards against include cycles. `managed` propagates the exact-owned flag.
///
/// Fail-closed: a referenced file that EXISTS but cannot be read (permission
/// denied, broken symlink) yields an error so callers refuse to write rather than
/// silently miss an unmanaged host. A MISSING file (including the root config) is
/// treated as "nothing to include" — distinguished from access errors via
/// `try_exists`, not the error-folding `Path::exists`.
fn parse_config_file(path: &Path, seen: &mut BTreeSet<PathBuf>) -> DiscoveryResult {
    if !is_present(path)? {
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
                // B3: safe Include resolution. Expand ~, follow relative paths
                // (anchored to ~/.ssh for user config, including nested includes),
                // glob-expand patterns in any component, and recurse with cycle
                // guard. A referenced-but-unreadable file fails closed.
                for inc in value.split_whitespace() {
                    let paths = resolve_include_paths(inc)?;
                    // An explicit path that exists and can't be read must fail closed.
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

/// `try_exists` distinguishes NotFound (→ Ok(false)) from access errors
/// (→ Err), unlike `Path::exists` which folds every error into `false`.
fn is_present(path: &Path) -> Result<bool, DiscoveryError> {
    match path.try_exists() {
        Ok(true) => Ok(true),
        Ok(false) => Ok(false),
        Err(e) if is_not_found(&e) => Ok(false),
        Err(e) => Err(DiscoveryError::Unreadable(format!(
            "{}: {e}",
            path.display()
        ))),
    }
}

#[cfg(unix)]
fn is_not_found(e: &std::io::Error) -> bool {
    e.raw_os_error() == Some(2) // ENOENT
}

#[cfg(not(unix))]
fn is_not_found(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::NotFound
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
///
/// OpenSSH semantics (user configuration):
/// - `~` expands to $HOME.
/// - Relative paths anchor to ~/.ssh, INCLUDING nested includes (not the
///   including file's directory).
/// - `*`/`?` wildcards are supported in ANY path component and expanded
///   recursively; a directory that cannot be read is an error (fail-closed),
///   not an empty result.
///
/// Returns an error when resolution or glob traversal fails, so a missing
/// unmanaged host inside an unreadable glob cannot be hidden.
fn resolve_include_paths(inc: &str) -> Result<Vec<PathBuf>, DiscoveryError> {
    let expanded = expand_home(inc);
    let p = PathBuf::from(expanded);
    let absolute = if p.is_absolute() {
        p
    } else {
        // Anchor relative includes to ~/.ssh (OpenSSH user-config rule).
        ssh_dir().join(&p)
    };

    let mut parts: Vec<GlobPart> = Vec::new();
    for comp in absolute.components() {
        match comp {
            Component::Normal(c) => {
                let s = c.to_string_lossy().into_owned();
                if s.contains('*') || s.contains('?') || s.contains('[') {
                    parts.push(GlobPart {
                        literal: None,
                        pattern: Some(s),
                    });
                } else {
                    parts.push(GlobPart {
                        literal: Some(s),
                        pattern: None,
                    });
                }
            }
            other => {
                parts.push(GlobPart {
                    literal: Some(other.as_os_str().to_string_lossy().into_owned()),
                    pattern: None,
                });
            }
        }
    }

    if !parts.iter().any(|p| p.pattern.is_some()) {
        return Ok(vec![absolute]);
    }
    // Build the literal base directory from the ORIGINAL absolute path's leading
    // literal components (preserving the real root/home prefix), then resolve the
    // remaining wildcard components against it.
    let prefix_len = parts.iter().take_while(|p| p.literal.is_some()).count();
    let mut base = PathBuf::new();
    for c in absolute.components().take(prefix_len) {
        base.push(c.as_os_str());
    }
    let mut out = Vec::new();
    resolve_components(&base, &parts[prefix_len..], &mut out)?;
    out.sort();
    Ok(out)
}

/// One path-component of an Include pattern.
struct GlobPart {
    /// Literal directory/file name (no wildcard).
    literal: Option<String>,
    /// Wildcard pattern (single wildcard segment), matched in full.
    pattern: Option<String>,
}

/// Recursively resolve `parts` starting from `base`, collecting concrete files.
/// A literal part must exist as a (sub)directory; an unreadable directory is an
/// error. A wildcard part is expanded against the current directory and descent
/// continues through each match (so `env-*/hosts.conf` works).
fn resolve_components(
    base: &Path,
    parts: &[GlobPart],
    out: &mut Vec<PathBuf>,
) -> Result<(), DiscoveryError> {
    let (head, tail) = match parts.split_first() {
        Some(x) => x,
        None => return Ok(()),
    };

    if let Some(lit) = &head.literal {
        let next = base.join(lit);
        if tail.is_empty() {
            if is_present(&next)? {
                out.push(next);
            }
            return Ok(());
        }
        let meta = std::fs::metadata(&next).map_err(|e| {
            if is_not_found(&e) {
                DiscoveryError::Unreadable(format!("include path {}: missing", next.display()))
            } else {
                DiscoveryError::Unreadable(format!("include dir {}: {e}", next.display()))
            }
        })?;
        if !meta.is_dir() {
            return Err(DiscoveryError::Unreadable(format!(
                "include path {} is not a directory",
                next.display()
            )));
        }
        resolve_components(&next, tail, out)
    } else if let Some(pat) = &head.pattern {
        let entries = std::fs::read_dir(base).map_err(|e| {
            DiscoveryError::Unreadable(format!("include dir {}: {e}", base.display()))
        })?;
        for entry in entries {
            let entry = entry.map_err(|e| {
                DiscoveryError::Unreadable(format!("include dir {}: {e}", base.display()))
            })?;
            let name = entry.file_name().to_string_lossy().into_owned();
            // Match the whole segment (e.g. `host?.conf`), so the literal suffix
            // after the wildcard is part of the match for the final component.
            if !wildcard_match(&name, pat) {
                continue;
            }
            let matched = entry.path();
            if tail.is_empty() {
                out.push(matched);
            } else {
                // Error here means we cannot safely determine/traverse the
                // matched path — fail closed (requirement #8), do NOT treat as
                // "no match". A non-directory match simply cannot be descended.
                let meta = std::fs::metadata(&matched).map_err(|e| {
                    DiscoveryError::Unreadable(format!("include path {}: {e}", matched.display()))
                })?;
                if meta.is_dir() {
                    resolve_components(&matched, tail, out)?;
                }
            }
        }
        Ok(())
    } else {
        Ok(())
    }
}

/// `*` (any run) / `?` (exactly one char) matcher, anchored at both ends.
/// Operates on `&str` via char vectors so UTF-8 is handled correctly.
pub(crate) fn wildcard_match(text: &str, pattern: &str) -> bool {
    let t: Vec<char> = text.chars().collect();
    let p: Vec<char> = pattern.chars().collect();
    wildcard_match_chars(&t, &p)
}

/// Match `text` against `pattern` where `*` matches any run, `?` any single char
/// and `[...]` a bracket class (char list or `a-z` range). Mirrors glob(3)
/// pathname semantics as used by OpenSSH for Include pathnames:
/// - the literal segment before the first `*` is anchored at the start,
/// - the segment after the last `*` at the end,
/// - internal segments are matched in order with no overlap (suffix start
///   must be >= the position consumed by prior segments),
/// - a leading `.` in `text` is only matched by an explicit `.` at the start of
///   the pattern (no implicit `*`/`.` matching), per POSIX pathname glob.
fn wildcard_match_chars(text: &[char], pattern: &[char]) -> bool {
    // Leading-dot rule (POSIX pathname glob): a leading `.` is NOT matched by an
    // implicit wildcard — only by an explicit `.` at pattern position 0.
    if text.first() == Some(&'.') && pattern.first() != Some(&'.') {
        return false;
    }
    let stars: Vec<usize> = pattern
        .iter()
        .enumerate()
        .filter(|(_, c)| **c == '*')
        .map(|(i, _)| i)
        .collect();
    if stars.is_empty() {
        // No '*': the whole text must equal the pattern (with ?/[...] inside).
        return match_glob(text, pattern);
    }
    // Split into literal segments (each may contain ?/[...]) between the '*'s.
    let mut segs: Vec<&[char]> = Vec::new();
    let mut prev = 0usize;
    for s in &stars {
        segs.push(&pattern[prev..*s]);
        prev = *s + 1;
    }
    segs.push(&pattern[prev..]);

    let mut ti = 0usize;
    // First segment anchored at start.
    if !segs[0].is_empty() {
        if text.len() < segs[0].len() {
            return false;
        }
        if !match_glob(&text[..segs[0].len()], segs[0]) {
            return false;
        }
        ti = segs[0].len();
    }
    // Internal segments in order, non-overlapping.
    for seg in &segs[1..segs.len() - 1] {
        if seg.is_empty() {
            continue;
        }
        match find_segment(text, ti, seg) {
            Some(idx) => ti = idx + seg.len(),
            None => return false,
        }
    }
    // Last segment anchored at end, and must not reuse consumed characters.
    let last = segs[segs.len() - 1];
    if !last.is_empty() {
        if text.len() < last.len() || text.len() - last.len() < ti {
            return false;
        }
        return match_glob(&text[text.len() - last.len()..], last);
    }
    true
}

/// Match `pattern` against the ENTIRE `text` slice: literal chars match exactly,
/// `?` matches any char, `[...]` a bracket class. Used for whole segments (which
/// are matched against an exactly-sized window of the filename).
fn match_glob(text: &[char], pattern: &[char]) -> bool {
    let mut ti = 0;
    let mut si = 0;
    while si < pattern.len() {
        let pc = pattern[si];
        if pc == '?' {
            if ti >= text.len() {
                return false;
            }
        } else if pc == '[' {
            // Bracket expression up to the matching ']'.
            let end = match pattern[si..].iter().position(|c| *c == ']') {
                Some(e) => si + e,
                None => return false,
            };
            let body = &pattern[si + 1..end];
            if ti >= text.len() || !match_bracket(text[ti], body) {
                return false;
            }
            si = end + 1;
            ti += 1;
            continue;
        } else if ti >= text.len() || text[ti] != pc {
            return false;
        }
        ti += 1;
        si += 1;
    }
    ti == text.len()
}

/// Does `c` match the bracket body `pat` (WITHOUT the outer brackets)? Supports a
/// char list and `x-y` ranges; `!`/`^` as first char negates.
fn match_bracket(c: char, pat: &[char]) -> bool {
    let (negate, body) = match pat.first() {
        Some(&'!' | &'^') => (true, &pat[1..]),
        _ => (false, pat),
    };
    let mut i = 0;
    let mut matched = false;
    while i < body.len() {
        if i + 2 < body.len() && body[i + 1] == '-' && body[i + 2] != ']' {
            let lo = body[i];
            let hi = body[i + 2];
            if lo <= c && c <= hi {
                matched = true;
            }
            i += 3;
        } else {
            if body[i] == c {
                matched = true;
            }
            i += 1;
        }
    }
    matched != negate
}

/// Find `seg` in `text[from..]`, returning the start index. `seg` may contain
/// `?` (any char) and `[...]` bracket expressions.
fn find_segment(text: &[char], from: usize, seg: &[char]) -> Option<usize> {
    if seg.is_empty() {
        return Some(from);
    }
    if text.len() < from + seg.len() {
        return None;
    }
    for start in from..=(text.len() - seg.len()) {
        if match_glob(&text[start..start + seg.len()], seg) {
            return Some(start);
        }
    }
    None
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
