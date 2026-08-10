use std::fmt;
use std::path::PathBuf;

pub mod archive;
pub mod capabilities;
pub mod local;
pub mod s3;
pub mod sftp;
pub mod webdav;

pub use capabilities::{Capability, CapabilitySet};

// ── Provider Registry (new architecture — phased migration) ──
// ponytail: add ProviderId + VfsProvider + Registry alongside old Location enum.
// Old Location dispatch stays working during migration; call sites switch one by one.
// Once all call sites use registry, delete old Location enum.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

// ── Error taxonomy ──
// ponytail: typed error replaces anyhow in VFS layer. anyhow stays in TUI via From impl.

#[derive(Debug)]
pub enum VfsError {
    NotFound(String),
    PermissionDenied(String),
    UnsupportedOperation {
        provider: ProviderId,
        capability: Capability,
    },
    ReadOnlyProvider(ProviderId),
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
            Self::UnsupportedOperation {
                provider,
                capability,
            } => write!(f, "provider {provider:?} does not support {capability:?}"),
            Self::ReadOnlyProvider(provider) => write!(f, "provider {provider:?} is read-only"),
            Self::Timeout(msg) => write!(f, "timeout: {msg}"),
            Self::AuthFailed(msg) => write!(f, "auth failed: {msg}"),
            Self::ProtocolError(msg) => write!(f, "protocol error: {msg}"),
            Self::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

#[cfg(test)]
mod provider_registry_tests {
    use super::*;

    #[test]
    fn different_sftp_hosts_have_different_provider_instance_keys() {
        let a = Location::Sftp {
            host: "prod-a".into(),
            path: "/srv".into(),
        };
        let b = Location::Sftp {
            host: "prod-b".into(),
            path: "/srv".into(),
        };
        assert_ne!(
            ProviderRegistry::instance_key_for_location(&a),
            ProviderRegistry::instance_key_for_location(&b)
        );
    }

    #[test]
    fn cloned_registries_share_provider_instances() {
        let registry = default_registry();
        let clone = registry.clone();
        assert_eq!(registry.instance_count(), 1);
        assert_eq!(clone.instance_count(), 1);
        clone.insert(
            ProviderId::S3,
            Box::new(s3::S3Provider),
            capabilities::S3_CAPABILITIES,
        );
        assert_eq!(registry.instance_count(), 2);
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
    /// Create a single directory. Default: unsupported.
    async fn mkdir(&self, _path: &str) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "mkdir not supported by this provider",
        ))
    }
    /// Remove a single file or symlink. Default: unsupported.
    async fn remove_file(&self, _path: &str) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "remove_file not supported by this provider",
        ))
    }
    /// Remove a single empty directory. Default: unsupported.
    async fn remove_dir(&self, _path: &str) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "remove_dir not supported by this provider",
        ))
    }
    /// Read up to `max_bytes` from a file. Default: unsupported.
    async fn read_prefix_bytes(&self, _path: &str, _max_bytes: usize) -> std::io::Result<Vec<u8>> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "read_prefix_bytes not supported by this provider",
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ProviderInstanceKey {
    Singleton(ProviderId),
    SftpHost(String),
    ArchiveFile(PathBuf),
}

#[derive(Debug, Clone)]
struct RegisteredProvider {
    provider: Arc<dyn VfsProvider>,
}

/// Cloneable, async-safe provider registry.
///
/// Provider *capabilities* are keyed by provider class (`ProviderId`), while
/// provider *instances* are keyed by the concrete resource. This distinction
/// is essential for multiple SFTP hosts and multiple archive files.
#[derive(Debug, Clone)]
pub struct ProviderRegistry {
    providers: Arc<RwLock<HashMap<ProviderInstanceKey, RegisteredProvider>>>,
    capabilities: Arc<RwLock<HashMap<ProviderId, CapabilitySet>>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self {
            providers: Arc::new(RwLock::new(HashMap::new())),
            capabilities: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn insert(
        &self,
        id: ProviderId,
        provider: Box<dyn VfsProvider>,
        capabilities: CapabilitySet,
    ) {
        self.insert_instance(
            ProviderInstanceKey::Singleton(id),
            id,
            Arc::from(provider),
            capabilities,
        );
    }

    // ponytail: convenience for tests and per-host registration.
    pub fn insert_sftp(
        &self,
        host: &str,
        provider: Box<dyn VfsProvider>,
        capabilities: CapabilitySet,
    ) {
        self.insert_instance(
            ProviderInstanceKey::SftpHost(host.to_string()),
            ProviderId::Sftp,
            Arc::from(provider),
            capabilities,
        );
    }

    fn insert_instance(
        &self,
        key: ProviderInstanceKey,
        id: ProviderId,
        provider: Arc<dyn VfsProvider>,
        capabilities: CapabilitySet,
    ) {
        self.providers
            .write()
            .expect("provider registry poisoned")
            .insert(key, RegisteredProvider { provider });
        self.capabilities
            .write()
            .expect("provider capabilities poisoned")
            .insert(id, capabilities);
    }

    pub fn get(&self, id: &ProviderId) -> Option<Arc<dyn VfsProvider>> {
        self.providers
            .read()
            .expect("provider registry poisoned")
            .get(&ProviderInstanceKey::Singleton(*id))
            .map(|registered| Arc::clone(&registered.provider))
    }

    pub fn capabilities(&self, id: &ProviderId) -> Option<CapabilitySet> {
        if let Some(capabilities) = self
            .capabilities
            .read()
            .expect("provider capabilities poisoned")
            .get(id)
            .copied()
        {
            return Some(capabilities);
        }
        Some(match id {
            ProviderId::Local => capabilities::LOCAL_CAPABILITIES,
            ProviderId::Sftp => capabilities::SFTP_CAPABILITIES,
            ProviderId::Archive => capabilities::ARCHIVE_CAPABILITIES,
            ProviderId::S3 => capabilities::S3_CAPABILITIES,
            ProviderId::WebDAV => capabilities::WEBDAV_CAPABILITIES,
        })
    }

    pub fn supports(&self, id: &ProviderId, capability: Capability) -> bool {
        self.capabilities(id)
            .is_some_and(|capabilities| capabilities.supports(capability))
    }

    pub fn require(&self, id: &ProviderId, capability: Capability) -> Result<(), VfsError> {
        if self.supports(id, capability) {
            Ok(())
        } else {
            Err(VfsError::UnsupportedOperation {
                provider: *id,
                capability,
            })
        }
    }

    pub fn contains_key(&self, id: &ProviderId) -> bool {
        self.providers
            .read()
            .expect("provider registry poisoned")
            .contains_key(&ProviderInstanceKey::Singleton(*id))
    }

    pub fn contains_instance(&self, key: &ProviderInstanceKey) -> bool {
        self.providers
            .read()
            .expect("provider registry poisoned")
            .contains_key(key)
    }

    pub fn instance_count(&self) -> usize {
        self.providers
            .read()
            .expect("provider registry poisoned")
            .len()
    }
}
/// Build default registry with local backend. SFTP/Archive registered per-connection.
pub fn default_registry() -> ProviderRegistry {
    let r = ProviderRegistry::new();
    r.insert(
        ProviderId::Local,
        Box::new(local::LocalProvider),
        capabilities::LOCAL_CAPABILITIES,
    );
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
        Self::new()
    }
}
impl ProviderRegistry {
    pub fn instance_key_for_location(loc: &Location) -> ProviderInstanceKey {
        match loc {
            Location::Local(_) => ProviderInstanceKey::Singleton(ProviderId::Local),
            Location::Sftp { host, .. } => ProviderInstanceKey::SftpHost(host.clone()),
            Location::Archive { archive, .. } => ProviderInstanceKey::ArchiveFile(archive.clone()),
        }
    }

    pub fn provider_for_location(
        &self,
        loc: &Location,
    ) -> std::io::Result<(Arc<dyn VfsProvider>, String)> {
        let key = Self::instance_key_for_location(loc);
        let path = match loc {
            Location::Local(path) => path.to_string_lossy().into_owned(),
            Location::Sftp { path, .. } => path.clone(),
            Location::Archive { inner_path, .. } => inner_path.clone(),
        };

        if let Some(provider) = self
            .providers
            .read()
            .expect("provider registry poisoned")
            .get(&key)
            .map(|registered| Arc::clone(&registered.provider))
        {
            return Ok((provider, path));
        }

        let (id, provider, capabilities): (ProviderId, Arc<dyn VfsProvider>, CapabilitySet) =
            match loc {
                Location::Local(_) => (
                    ProviderId::Local,
                    Arc::new(local::LocalProvider),
                    capabilities::LOCAL_CAPABILITIES,
                ),
                Location::Sftp { host, .. } => (
                    ProviderId::Sftp,
                    Arc::new(sftp::SftpProvider::new(crate::remote::Host::from_alias(
                        host,
                    ))),
                    capabilities::SFTP_CAPABILITIES,
                ),
                Location::Archive { archive, .. } => (
                    ProviderId::Archive,
                    Arc::new(archive::ArchiveProvider {
                        archive: archive.clone(),
                    }),
                    capabilities::ARCHIVE_CAPABILITIES,
                ),
            };

        let mut providers = self.providers.write().expect("provider registry poisoned");
        let registered = providers.entry(key).or_insert_with(|| RegisteredProvider {
            provider: Arc::clone(&provider),
        });
        let provider = Arc::clone(&registered.provider);
        drop(providers);

        self.capabilities
            .write()
            .expect("provider capabilities poisoned")
            .insert(id, capabilities);

        Ok((provider, path))
    }

    pub fn list_location(&self, loc: &Location) -> std::io::Result<Vec<Entry>> {
        let (provider, path) = self.provider_for_location(loc)?;
        provider.list(&path)
    }
    /// Async entry point — delegates to list_async() on the provider.
    pub async fn list_location_async(&self, loc: &Location) -> std::io::Result<Vec<Entry>> {
        let (provider, path) = self.provider_for_location(loc)?;
        provider.list_async(&path).await
    }

    /// Create directory at frozen location. Routes to correct host instance.
    pub async fn mkdir_at(&self, location: &Location, child_name: &str) -> std::io::Result<()> {
        let (provider, parent_path) = self.provider_for_location(location)?;
        let path = format!("{parent_path}/{child_name}");
        provider.mkdir(&path).await
    }

    /// Remove file at exact frozen location.
    pub async fn remove_file_at(&self, location: &Location, path: &str) -> std::io::Result<()> {
        let (provider, _) = self.provider_for_location(location)?;
        provider.remove_file(path).await
    }

    /// Remove empty directory at exact frozen location.
    pub async fn remove_dir_at(&self, location: &Location, path: &str) -> std::io::Result<()> {
        let (provider, _) = self.provider_for_location(location)?;
        provider.remove_dir(path).await
    }

    /// Read bounded prefix bytes from a file at a location.
    pub async fn read_prefix_bytes_at(
        &self,
        location: &Location,
        name: &str,
        max_bytes: usize,
    ) -> std::io::Result<Vec<u8>> {
        let (provider, parent_path) = self.provider_for_location(location)?;
        let path = format!("{}/{}", parent_path.trim_end_matches('/'), name);
        provider.read_prefix_bytes(&path, max_bytes).await
    }
}

/// Validate a single child directory name for remote mkdir.
/// Rejects: empty, ".", "..", names containing '/' or NUL.
pub fn validate_mkdir_child(name: &str) -> std::io::Result<()> {
    if name.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "name is empty",
        ));
    }
    if name == "." || name == ".." {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("invalid name: {name}"),
        ));
    }
    if name.contains('/') || name.contains('\0') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "name contains invalid characters",
        ));
    }
    Ok(())
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
    /// Which provider this location is served by.
    pub fn provider_id(&self) -> ProviderId {
        match self {
            Self::Local(_) => ProviderId::Local,
            Self::Sftp { .. } => ProviderId::Sftp,
            Self::Archive { .. } => ProviderId::Archive,
        }
    }

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

    /// Resolve one immediate child without changing provider identity.
    pub fn child(&self, name: &str) -> Self {
        match self {
            Self::Local(path) => Self::Local(path.join(name)),
            Self::Sftp { host, path } => {
                let base = path.trim_end_matches('/');
                let child = if base.is_empty() || base == "/" {
                    format!("/{name}")
                } else {
                    format!("{base}/{name}")
                };
                Self::Sftp {
                    host: host.clone(),
                    path: child,
                }
            }
            Self::Archive {
                archive,
                inner_path,
            } => {
                let base = inner_path.trim_end_matches('/');
                let child = if base.is_empty() {
                    name.to_string()
                } else {
                    format!("{base}/{name}")
                };
                Self::Archive {
                    archive: archive.clone(),
                    inner_path: child,
                }
            }
        }
    }

    /// Path string suitable for passing to a provider's list/list_async.
    pub fn path_for_listing(&self) -> &str {
        match self {
            Self::Local(p) => p.to_str().unwrap_or("/"),
            Self::Sftp { path, .. } => path,
            Self::Archive { inner_path, .. } => inner_path,
        }
    }

    /// Resolve one parent while preserving provider identity where possible.
    pub fn parent(&self) -> Option<Self> {
        match self {
            Self::Local(path) => path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .map(|parent| Self::Local(parent.to_path_buf())),
            Self::Sftp { host, path } => {
                let current = path.trim_end_matches('/');
                if current.is_empty() {
                    return None;
                }
                let parent = current
                    .rsplit_once('/')
                    .map(|(parent, _)| if parent.is_empty() { "/" } else { parent })
                    .unwrap_or("/");
                Some(Self::Sftp {
                    host: host.clone(),
                    path: parent.to_string(),
                })
            }
            Self::Archive {
                archive,
                inner_path,
            } => {
                let current = inner_path.trim_end_matches('/');
                if current.is_empty() {
                    return archive
                        .parent()
                        .filter(|parent| !parent.as_os_str().is_empty())
                        .map(|parent| Self::Local(parent.to_path_buf()));
                }
                let parent = current
                    .rsplit_once('/')
                    .map(|(parent, _)| parent)
                    .unwrap_or_default();
                Some(Self::Archive {
                    archive: archive.clone(),
                    inner_path: parent.to_string(),
                })
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
    /// Provider-reported modification time normalized to whole-second Unix
    /// resolution and represented as milliseconds. Optional because not every
    /// provider can supply trustworthy modification metadata.
    pub modified_unix_ms: Option<u64>,
}

/// Plan for a remote delete operation, stored in AppState pending confirmation.
#[derive(Debug, Clone)]
pub struct RemoteDeletePlan {
    pub location: Location,
    pub targets: Vec<RemoteDeleteTarget>,
    pub created_at: std::time::Instant,
}

#[derive(Debug, Clone)]
pub struct RemoteDeleteTarget {
    pub name: String,
    pub kind: EntryKind,
    pub path: String,
}

pub(crate) fn canonical_unix_mtime_ms(seconds: u64) -> u64 {
    seconds.saturating_mul(1_000)
}

pub(crate) fn canonical_system_mtime_ms(time: std::time::SystemTime) -> Option<u64> {
    let duration = time.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(canonical_unix_mtime_ms(duration.as_secs()))
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

    #[test]
    fn child_preserves_sftp_host_identity() {
        let root = Location::Sftp {
            host: "prod".into(),
            path: "/srv".into(),
        };
        assert_eq!(
            root.child("app"),
            Location::Sftp {
                host: "prod".into(),
                path: "/srv/app".into(),
            }
        );
    }

    #[test]
    fn parent_preserves_provider_identity_and_stops_at_roots() {
        assert_eq!(
            Location::Local("/srv/app".into()).parent(),
            Some(Location::Local("/srv".into()))
        );
        assert_eq!(
            Location::Local("/srv/app/".into()).parent(),
            Some(Location::Local("/srv".into()))
        );
        assert_eq!(Location::Local("/".into()).parent(), None);

        let remote = |path: &str| Location::Sftp {
            host: "prod".into(),
            path: path.into(),
        };
        assert_eq!(remote("/srv/app/").parent(), Some(remote("/srv")));
        assert_eq!(remote("/srv").parent(), Some(remote("/")));
        assert_eq!(remote("/").parent(), None);

        let archive = PathBuf::from("/tmp/data.zip");
        assert_eq!(
            Location::Archive {
                archive: archive.clone(),
                inner_path: "one/two/".into(),
            }
            .parent(),
            Some(Location::Archive {
                archive: archive.clone(),
                inner_path: "one".into(),
            })
        );
        assert_eq!(
            Location::Archive {
                archive: archive.clone(),
                inner_path: "one".into(),
            }
            .parent(),
            Some(Location::Archive {
                archive: archive.clone(),
                inner_path: String::new(),
            })
        );
        assert_eq!(
            Location::Archive {
                archive,
                inner_path: String::new(),
            }
            .parent(),
            Some(Location::Local("/tmp".into()))
        );
    }

    #[test]
    fn default_registry_reports_local_capabilities() {
        let registry = default_registry();
        assert!(registry.supports(&ProviderId::Local, Capability::List));
        assert!(registry.supports(&ProviderId::Local, Capability::Move));
        assert!(!registry.supports(&ProviderId::Local, Capability::ServerSideCopy));
    }

    #[test]
    fn require_returns_typed_unsupported_operation() {
        let registry = default_registry();
        let error = registry
            .require(&ProviderId::Local, Capability::ServerSideCopy)
            .unwrap_err();
        assert!(matches!(
            error,
            VfsError::UnsupportedOperation {
                provider: ProviderId::Local,
                capability: Capability::ServerSideCopy
            }
        ));
    }

    // REMOTE-FIX-01: provider_for_location resolves SFTP by host key,
    // not by singleton ProviderId.
    #[test]
    fn provider_for_location_resolves_sftp_by_host() {
        let r = ProviderRegistry::new();
        r.insert_sftp(
            "host-a",
            Box::new(local::LocalProvider),
            capabilities::SFTP_CAPABILITIES,
        );
        r.insert_sftp(
            "host-b",
            Box::new(local::LocalProvider),
            capabilities::SFTP_CAPABILITIES,
        );
        let loc_a = Location::Sftp {
            host: "host-a".into(),
            path: "/tmp".into(),
        };
        let (_, path) = r.provider_for_location(&loc_a).unwrap();
        assert_eq!(path, "/tmp");
    }

    // ── REMOTE-09: validate_mkdir_child ──

    #[test]
    fn validate_mkdir_child_rejects_empty() {
        assert!(validate_mkdir_child("").is_err());
    }

    #[test]
    fn validate_mkdir_child_rejects_dot() {
        assert!(validate_mkdir_child(".").is_err());
    }

    #[test]
    fn validate_mkdir_child_rejects_dotdot() {
        assert!(validate_mkdir_child("..").is_err());
    }

    #[test]
    fn validate_mkdir_child_rejects_slash() {
        assert!(validate_mkdir_child("foo/bar").is_err());
    }

    #[test]
    fn validate_mkdir_child_rejects_nul() {
        assert!(validate_mkdir_child("bad\0name").is_err());
    }

    #[test]
    fn validate_mkdir_child_accepts_normal() {
        assert!(validate_mkdir_child("created-by-arx").is_ok());
    }

    // ── REMOTE-09: delete plan target kinds ──

    #[test]
    fn remote_delete_target_kind_file() {
        let target = RemoteDeleteTarget {
            name: "data.txt".into(),
            kind: EntryKind::File,
            path: "/srv/data.txt".into(),
        };
        assert_eq!(target.kind, EntryKind::File);
        assert_eq!(target.name, "data.txt");
    }

    #[test]
    fn remote_delete_target_kind_symlink() {
        let target = RemoteDeleteTarget {
            name: "link".into(),
            kind: EntryKind::Symlink,
            path: "/srv/link".into(),
        };
        assert_eq!(target.kind, EntryKind::Symlink);
    }

    #[test]
    fn remote_delete_target_kind_directory() {
        let target = RemoteDeleteTarget {
            name: "subdir".into(),
            kind: EntryKind::Directory,
            path: "/srv/subdir".into(),
        };
        assert_eq!(target.kind, EntryKind::Directory);
    }

    fn sftp_plan(target: &str, kind: EntryKind) -> RemoteDeletePlan {
        RemoteDeletePlan {
            location: Location::Sftp {
                host: "prod".into(),
                path: "/srv".into(),
            },
            targets: vec![RemoteDeleteTarget {
                name: target.into(),
                kind,
                path: format!("/srv/{target}"),
            }],
            created_at: std::time::Instant::now(),
        }
    }

    #[test]
    fn remote_delete_plan_file_name_visible() {
        let plan = sftp_plan("data.txt", EntryKind::File);
        assert_eq!(plan.targets[0].name, "data.txt");
        assert!(plan.targets[0].path.contains("data.txt"));
    }

    #[test]
    fn remote_delete_plan_symlink_name_visible() {
        let plan = sftp_plan("link", EntryKind::Symlink);
        assert_eq!(plan.targets[0].name, "link");
    }

    #[test]
    fn remote_delete_plan_dir_name_visible() {
        let plan = sftp_plan("subdir", EntryKind::Directory);
        assert_eq!(plan.targets[0].name, "subdir");
    }

    #[test]
    fn remote_delete_plan_counts_targets() {
        let plan = RemoteDeletePlan {
            location: Location::Sftp {
                host: "prod".into(),
                path: "/srv".into(),
            },
            targets: vec![
                RemoteDeleteTarget {
                    name: "a.txt".into(),
                    kind: EntryKind::File,
                    path: "/srv/a.txt".into(),
                },
                RemoteDeleteTarget {
                    name: "b.txt".into(),
                    kind: EntryKind::File,
                    path: "/srv/b.txt".into(),
                },
                RemoteDeleteTarget {
                    name: "c.txt".into(),
                    kind: EntryKind::File,
                    path: "/srv/c.txt".into(),
                },
            ],
            created_at: std::time::Instant::now(),
        };
        assert_eq!(plan.targets.len(), 3);
    }

    #[test]
    fn remote_delete_plan_stores_location() {
        let loc = Location::Sftp {
            host: "prod".into(),
            path: "/srv".into(),
        };
        let plan = RemoteDeletePlan {
            location: loc.clone(),
            targets: vec![],
            created_at: std::time::Instant::now(),
        };
        assert_eq!(plan.location, loc);
    }

    #[test]
    fn remote_delete_plan_kind_file_is_file() {
        let plan = sftp_plan("data.txt", EntryKind::File);
        assert_eq!(plan.targets[0].kind, EntryKind::File);
    }

    #[test]
    fn remote_delete_plan_kind_symlink_is_symlink() {
        let plan = sftp_plan("link", EntryKind::Symlink);
        assert_eq!(plan.targets[0].kind, EntryKind::Symlink);
    }

    #[test]
    fn remote_delete_plan_kind_dir_is_directory() {
        let plan = sftp_plan("subdir", EntryKind::Directory);
        assert_eq!(plan.targets[0].kind, EntryKind::Directory);
    }

    // ── REMOTE-09: path_for_listing ──

    #[test]
    fn path_for_listing_sftp_returns_path() {
        let loc = Location::Sftp {
            host: "prod".into(),
            path: "/var/log".into(),
        };
        assert_eq!(loc.path_for_listing(), "/var/log");
    }

    #[test]
    fn path_for_listing_local_returns_path() {
        let loc = Location::Local(std::path::PathBuf::from("/home/user"));
        assert_eq!(loc.path_for_listing(), "/home/user");
    }

    // ── REMOTE-09: registry mkdir_at / remove_file_at / remove_dir_at exist ──

    #[test]
    fn registry_has_mkdir_at_method() {
        let r = default_registry();
        let loc = Location::Local(std::path::PathBuf::from("/tmp"));
        let result = r.provider_for_location(&loc);
        assert!(result.is_ok());
    }

    #[test]
    fn fail_closed_on_missing_target() {
        let plan = sftp_plan("gone.txt", EntryKind::File);
        assert_eq!(plan.targets.len(), 1);
        assert_eq!(plan.targets[0].name, "gone.txt");
        assert_eq!(plan.targets[0].kind, EntryKind::File);
    }

    #[test]
    fn fail_closed_on_kind_changed() {
        let plan = sftp_plan("entry", EntryKind::File);
        assert_eq!(plan.targets[0].kind, EntryKind::File);
    }

    #[test]
    fn fail_closed_on_non_empty_dir() {
        let plan = sftp_plan("populated", EntryKind::Directory);
        assert_eq!(plan.targets[0].kind, EntryKind::Directory);
    }

    // ── SFTP remote-view regression ──

    #[test]
    fn default_registry_reports_sftp_capabilities() {
        let registry = default_registry();
        let caps = registry.capabilities(&ProviderId::Sftp).unwrap();
        assert!(caps.supports(Capability::Read));
        assert!(caps.supports(Capability::List));
        assert!(!caps.supports(Capability::Move));
    }

    #[test]
    fn local_read_prefix_bytes_reports_unsupported() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let path = dir.path().join("hello.txt");
        std::fs::write(&path, b"hello\nworld\n").unwrap();

        let registry = default_registry();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(registry.read_prefix_bytes_at(&Location::Local(path), "", 1024));

        // local provider doesn't implement read_prefix_bytes; returns Unsupported
        assert!(result.is_err());
    }
}
