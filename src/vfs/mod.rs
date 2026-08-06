use std::fmt;
use std::path::PathBuf;

pub mod archive;
pub mod local;
pub mod s3;
pub mod sftp;
pub mod webdav;

// ── Provider Registry (new architecture — phased migration) ──
// ponytail: add ProviderId + VfsProvider + Registry alongside old Location enum.
// Old Location dispatch stays working during migration; call sites switch one by one.
// Once all call sites use registry, delete old Location enum.

use std::collections::HashMap;

// ── Error taxonomy ──
// ponytail: typed error replaces anyhow in VFS layer. anyhow stays in TUI via From impl.

#[derive(Debug)]
pub enum VfsError {
    NotFound(String),
    PermissionDenied(String),
    Timeout(String),
    AuthFailed(String),
    ProtocolError(String),
    Io(std::io::Error),
}

impl fmt::Display for VfsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(msg) => write!(f, "not found: {msg}"),
            Self::PermissionDenied(msg) => write!(f, "permission denied: {msg}"),
            Self::Timeout(msg) => write!(f, "timeout: {msg}"),
            Self::AuthFailed(msg) => write!(f, "auth failed: {msg}"),
            Self::ProtocolError(msg) => write!(f, "protocol error: {msg}"),
            Self::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl std::error::Error for VfsError {}

impl From<std::io::Error> for VfsError {
    fn from(e: std::io::Error) -> Self {
        match e.kind() {
            std::io::ErrorKind::NotFound => Self::NotFound(e.to_string()),
            std::io::ErrorKind::PermissionDenied => Self::PermissionDenied(e.to_string()),
            std::io::ErrorKind::TimedOut => Self::Timeout(e.to_string()),
            _ => Self::Io(e),
        }
    }
}

impl From<VfsError> for std::io::Error {
    fn from(e: VfsError) -> Self {
        match e {
            VfsError::Io(io) => io,
            e => std::io::Error::other(e.to_string()),
        }
    }
}

// anyhow gets From<VfsError> automatically via std::error::Error impl

// ── Provider Registry types ──

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderId {
    Local,
    Sftp,
    Archive,
    S3,
    WebDAV,
}

/// Unified location target — replaces Location enum after migration.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Target {
    pub provider: ProviderId,
    pub path: String,
}

impl Target {
    pub fn local(p: &std::path::Path) -> Self {
        Self {
            provider: ProviderId::Local,
            path: p.to_string_lossy().into_owned(),
        }
    }
    pub fn sftp(path: &str) -> Self {
        Self {
            provider: ProviderId::Sftp,
            path: path.to_string(),
        }
    }
    pub fn archive(inner: &str) -> Self {
        Self {
            provider: ProviderId::Archive,
            path: inner.to_string(),
        }
    }
    pub fn resolve(&self, name: &str) -> Self {
        let p = if self.path.ends_with('/') || self.path.is_empty() {
            format!("{}{}", self.path, name)
        } else {
            format!("{}/{}", self.path, name)
        };
        Self {
            path: p,
            ..self.clone()
        }
    }
    pub fn parent(&self) -> Option<Self> {
        let t = self.path.trim_end_matches('/');
        if t.is_empty() || !t.contains('/') {
            return None;
        }
        Some(Self {
            path: t.rsplit_once('/').unwrap().0.to_string(),
            ..self.clone()
        })
    }
}

/// Backend trait — each provider implements this. async deferred to F2.
/// ponytail: sync list() kept for backward compat; list_async() is the new path.
#[async_trait::async_trait]
pub trait VfsProvider: Send + Sync + std::fmt::Debug {
    fn list(&self, path: &str) -> std::io::Result<Vec<Entry>>;
    /// Async entry point — providers that can (SFTP, S3, WebDAV) skip block_on.
    /// Default impl delegates to sync list(); override for native async.
    async fn list_async(&self, path: &str) -> std::io::Result<Vec<Entry>> {
        self.list(path)
    }
    fn read_head(&self, path: &str, lines: usize) -> std::io::Result<Vec<String>>;
    fn copy_files(&self, src: &str, dst: &str, names: &[String]) -> std::io::Result<usize>;
    fn move_files(&self, src: &str, dst: &str, names: &[String]) -> std::io::Result<usize>;
    fn delete_files(&self, dir: &str, names: &[String]) -> std::io::Result<usize>;
}

#[derive(Debug)]
pub struct ProviderRegistry(HashMap<ProviderId, Box<dyn VfsProvider>>);

impl ProviderRegistry {
    pub fn new() -> Self {
        Self(HashMap::new())
    }
    pub fn insert(&mut self, id: ProviderId, provider: Box<dyn VfsProvider>) {
        self.0.insert(id, provider);
    }
    pub fn get(&self, id: &ProviderId) -> Option<&dyn VfsProvider> {
        self.0.get(id).map(|b| b.as_ref())
    }
    pub fn contains_key(&self, id: &ProviderId) -> bool {
        self.0.contains_key(id)
    }
}

/// Build default registry with local backend. SFTP/Archive registered per-connection.
pub fn default_registry() -> ProviderRegistry {
    let mut r = ProviderRegistry::new();
    r.insert(ProviderId::Local, Box::new(local::LocalProvider));
    r
}

// ponytail: thread-local bridge during migration. Delete after all call sites
// use ProviderRegistry directly (Phase 3).
std::thread_local! {
    static PROVIDER_REGISTRY: std::cell::RefCell<ProviderRegistry> =
        std::cell::RefCell::new(ProviderRegistry::new());
}

// ponytail: one-shot init from AppState::default
pub fn set_global_registry(r: ProviderRegistry) {
    PROVIDER_REGISTRY.with(|cell| *cell.borrow_mut() = r);
}

pub(crate) fn with_registry_mut<F, R>(f: F) -> R
where
    F: FnOnce(&mut ProviderRegistry) -> R,
{
    PROVIDER_REGISTRY.with(|cell| f(&mut cell.borrow_mut()))
}

#[allow(clippy::derivable_impls)]
impl Default for ProviderRegistry {
    fn default() -> Self {
        Self(HashMap::new())
    }
}

impl ProviderRegistry {
    /// Map old Location enum → (ProviderId, path) and dispatch through registry.
    /// ponytail: bridge; delete when Location enum is replaced by Target.
    fn map_location(&mut self, loc: &Location) -> (ProviderId, String) {
        let (pid, path) = match loc {
            Location::Local(p) => (ProviderId::Local, p.to_string_lossy().into_owned()),
            Location::Sftp { host, path } => {
                // ponytail: lazy-register SFTP provider per-host
                if !self.contains_key(&ProviderId::Sftp) {
                    let h = crate::remote::Host::from_alias(host);
                    self.insert(ProviderId::Sftp, Box::new(sftp::SftpProvider { host: h }));
                }
                (ProviderId::Sftp, path.clone())
            }
            Location::Archive {
                archive: _,
                inner_path,
            } => (ProviderId::Archive, inner_path.clone()),
        };
        (pid, path)
    }

    pub fn list_location(&mut self, loc: &Location) -> std::io::Result<Vec<Entry>> {
        let (pid, path) = self.map_location(loc);
        self.get(&pid)
            .ok_or_else(|| std::io::Error::other("provider not registered"))?
            .list(&path)
    }

    /// Async entry point — delegates to list_async() on the provider.
    /// ponytail: use this in async contexts; sync list_location() kept for backward compat.
    pub async fn list_location_async(&mut self, loc: &Location) -> std::io::Result<Vec<Entry>> {
        let (pid, path) = self.map_location(loc);
        self.get(&pid)
            .ok_or_else(|| std::io::Error::other("provider not registered"))?
            .list_async(&path)
            .await
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Location {
    Local(PathBuf),
    Sftp {
        host: String,
        path: String,
    },
    Archive {
        archive: PathBuf,
        inner_path: String,
    },
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local(path) => write!(f, "file://{}", path.display()),
            Self::Sftp { host, path } => write!(f, "sftp://{host}{path}"),
            Self::Archive {
                archive,
                inner_path,
            } => write!(f, "archive://{}!/{inner_path}", archive.display()),
        }
    }
}

impl Location {
    /// Human-readable short label for a pane title (e.g. the directory name or host).
    pub fn label(&self) -> String {
        match self {
            Self::Local(p) => p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.to_string_lossy().into_owned()),
            Self::Sftp { host, path } => {
                let last = path.rsplit('/').next().unwrap_or(path);
                format!("{host}:{last}")
            }
            Self::Archive { inner_path, .. } => {
                let last = inner_path.rsplit('/').next().unwrap_or(inner_path);
                last.to_string()
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub kind: EntryKind,
    pub size: Option<u64>,
}

/// Abstract VFS operations — backend-agnostic interface.
/// ponytail: trait object dispatch; full provider registry deferred to Wave 2.
pub trait VfsOps {
    fn list(&self) -> anyhow::Result<Vec<Entry>>;
    fn read_head(&self, path: &str, lines: usize) -> anyhow::Result<Vec<String>>;
    fn copy_files(&self, src_dir: &str, dst_dir: &str, names: &[String]) -> std::io::Result<usize>;
    fn move_files(&self, src_dir: &str, dst_dir: &str, names: &[String]) -> std::io::Result<usize>;
    fn delete_files(&self, dir: &str, names: &[String]) -> std::io::Result<usize>;
}

impl VfsOps for Location {
    fn list(&self) -> anyhow::Result<Vec<Entry>> {
        with_registry_mut(|r| r.list_location(self)).map_err(|e| anyhow::anyhow!("{e}"))
    }

    fn read_head(&self, path: &str, lines: usize) -> anyhow::Result<Vec<String>> {
        match self {
            Location::Local(_) => local::LocalFs::read_head(std::path::Path::new(path), lines)
                .map_err(|e| anyhow::anyhow!("{e}")),
            _ => Err(anyhow::anyhow!("read_head only supported for Local paths")),
        }
    }

    fn copy_files(&self, src_dir: &str, dst_dir: &str, names: &[String]) -> std::io::Result<usize> {
        match self {
            Location::Local(_) => local::LocalFs::copy_files(
                std::path::Path::new(src_dir),
                std::path::Path::new(dst_dir),
                names,
            ),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "copy only supported for Local",
            )),
        }
    }

    fn move_files(&self, src_dir: &str, dst_dir: &str, names: &[String]) -> std::io::Result<usize> {
        match self {
            Location::Local(_) => local::LocalFs::move_files(
                std::path::Path::new(src_dir),
                std::path::Path::new(dst_dir),
                names,
            ),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "move only supported for Local",
            )),
        }
    }

    fn delete_files(&self, dir: &str, names: &[String]) -> std::io::Result<usize> {
        match self {
            Location::Local(_) => local::LocalFs::delete_files(std::path::Path::new(dir), names),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "delete only supported for Local",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_sftp_location() {
        let location = Location::Sftp {
            host: "db-prod".into(),
            path: "/var/log".into(),
        };
        assert_eq!(location.to_string(), "sftp://db-prod/var/log");
    }
}
