use std::fmt;
use std::path::PathBuf;

pub mod archive;
pub mod capabilities;
pub mod local;
pub mod s3;
pub mod sftp;
pub mod webdav;
#[cfg(all(test, feature = "physical-webdav"))]
mod webdav_acceptance;
#[cfg(all(test, feature = "physical-webdav"))]
mod webdav_interop_acceptance;
#[cfg(all(test, feature = "physical-webdav"))]
mod webdav_remote_copy_acceptance;
#[cfg(test)]
mod webdav_tests;

pub use capabilities::{Capability, CapabilitySet};
pub use s3::{S3BucketRef, S3ObjectRef, S3PrefixRef};
pub use webdav::{WebDavCollectionRef, WebDavObjectRef, encode_segment};

/// Shared safety bounds for all recursive WebDAV operations.
pub const MAX_WEBDAV_TREE_DESCENDANTS: usize = 50_000;
pub const MAX_WEBDAV_TREE_DEPTH: usize = 128;

// ── VFS authority model (final; PACK Q closeout) ──
//
// Location          = typed identity / address / navigation information
// ProviderRegistry  = explicit provider execution authority
// CapabilitySet     = exact-location / concrete-instance capability truth
// VfsProvider       = backend provider interface
//
// Copy/Move execution lives in TransferPlanner / TransferQueue / the transfer
// executor; mutations live in MutationService and the typed Registry/provider
// mutation seams. There is no hidden global registry and no second execution
// path. Two deliberate resolver seams exist:
//   * provider_for_location      — string-path backend operations
//     (evaluates Location::legacy_listing_path first, so S3 fails closed
//     before any provider construction)
//   * provider_for_page_location — typed page/native-identity operations
//     (S3 resolves its exact configured S3Target, WebDAV its WebDavTarget;
//     Local/SFTP/Archive delegate to the string-path resolver)
// Both seams are kept intentionally distinct; neither flattens typed identity.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
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
            Box::new(local::LocalProvider),
            capabilities::S3_CAPABILITIES,
        );
        assert_eq!(registry.instance_count(), 2);
    }

    #[test]
    fn s3_transfer_provider_is_same_instance_as_listing_provider() {
        let registry = ProviderRegistry::new();
        registry.register_s3_targets(&[crate::config::S3TargetConfig {
            id: "aws-prod".to_string(),
            name: "aws-prod".to_string(),
            bucket: None,
            region: Some("eu-central-1".to_string()),
            profile: Some("prod".to_string()),
            endpoint_url: None,
            force_path_style: false,
        }]);

        let listing = registry
            .provider_for_page_location(&Location::S3 {
                target: "aws-prod".to_string(),
                bucket: Some("some-bucket".into()),
                prefix: "".to_string(),
            })
            .unwrap();

        let transfer = registry.s3_provider_for_transfer("aws-prod").unwrap();

        // Same underlying S3Provider allocation (same data pointer).
        let listing_ptr = Arc::as_ptr(&listing) as *const u8;
        let transfer_ptr = Arc::as_ptr(&transfer) as *const u8;
        assert_eq!(
            listing_ptr, transfer_ptr,
            "listing and transfer must share one S3Provider instance"
        );

        // Unknown target fails.
        assert!(registry.s3_provider_for_transfer("nope").is_err());
    }
}

impl std::error::Error for VfsError {}

/// Registry resolution errors (distinct from `VfsError`, which is the
/// provider-operation error taxonomy).
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("unknown S3 target: {0}")]
    NotFound(String),
}

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

/// String-path location target used by string-path backend operations.
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

/// Truncation-aware bounded read result from a remote provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedRead {
    pub bytes: Vec<u8>,
    pub truncated: bool,
    /// Unix mode and ownership captured in the same stable-read window as `bytes`.
    pub unix_mode: Option<u32>,
    pub unix_uid: Option<u32>,
    pub unix_gid: Option<u32>,
}

impl BoundedRead {
    pub fn into_revision(self) -> std::io::Result<RemoteEditRevision> {
        if self.truncated {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "remote snapshot is truncated",
            ));
        }
        let missing = |field| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("remote snapshot has no Unix {field}"),
            )
        };
        Ok(RemoteEditRevision {
            bytes: self.bytes,
            unix_mode: self.unix_mode.ok_or_else(|| missing("mode"))?,
            unix_uid: self.unix_uid.ok_or_else(|| missing("uid"))?,
            unix_gid: self.unix_gid.ok_or_else(|| missing("gid"))?,
        })
    }
}

/// Exact content, mode, and ownership captured by one stable remote read.
#[derive(Clone, PartialEq, Eq)]
pub struct RemoteEditRevision {
    bytes: Vec<u8>,
    unix_mode: u32,
    unix_uid: u32,
    unix_gid: u32,
}

impl RemoteEditRevision {
    /// Construct from a captured stable read. Used by tests/acceptance harness
    /// to drive `write_file_bytes_if_unchanged_at` without a prior read roundtrip.
    pub fn new(bytes: Vec<u8>, unix_mode: u32, unix_uid: u32, unix_gid: u32) -> Self {
        Self {
            bytes,
            unix_mode,
            unix_uid,
            unix_gid,
        }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn unix_mode(&self) -> u32 {
        self.unix_mode
    }

    pub fn unix_uid(&self) -> u32 {
        self.unix_uid
    }

    pub fn unix_gid(&self) -> u32 {
        self.unix_gid
    }
}

impl std::fmt::Debug for RemoteEditRevision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteEditRevision")
            .field("bytes_len", &self.bytes.len())
            .field("unix_mode", &format_args!("{:#o}", self.unix_mode))
            .field("unix_uid", &self.unix_uid)
            .field("unix_gid", &self.unix_gid)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteWriteFailureKind {
    RecoveryRequired,
    CommittedWithWarning,
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
struct RemoteWriteFailure {
    kind: RemoteWriteFailureKind,
    message: String,
}

pub fn remote_write_error(
    kind: RemoteWriteFailureKind,
    message: impl Into<String>,
) -> std::io::Error {
    std::io::Error::other(RemoteWriteFailure {
        kind,
        message: message.into(),
    })
}

pub fn remote_write_failure_kind(error: &std::io::Error) -> Option<RemoteWriteFailureKind> {
    error
        .get_ref()
        .and_then(|source| source.downcast_ref::<RemoteWriteFailure>())
        .map(|failure| failure.kind)
}

/// Cooperative cancellation checked only at data-safe I/O boundaries.
#[derive(Clone, Default)]
pub struct CancellationFlag {
    cancelled: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
}

impl CancellationFlag {
    pub fn cancel(&self) {
        if !self.cancelled.swap(true, Ordering::AcqRel) {
            self.notify.notify_waiters();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub async fn cancelled(&self) {
        let notified = self.notify.notified();
        tokio::pin!(notified);
        let _ = notified.as_mut().enable();
        if !self.is_cancelled() {
            notified.await;
        }
    }
}

/// Maximum bytes ARX will download for remote editing.
/// Files larger than this are refused before editor launch.
pub const MAX_REMOTE_EDIT_BYTES: usize = 16 * 1024 * 1024; // 16 MiB

/// ponytail: narrow synchronous Send+Sync progress callback from a remote-write
/// transaction to JobManager. The provider never knows about JobManager; it just
/// emits the phase at the real boundary (Verifying before verify,
/// RollbackOrRecovery at the real recovery transition). The TUI supplies a
/// closure that synchronously forwards to JobManager, so progress is published
/// in program order before the terminal event — no detached relay, no scheduler
/// races, deterministic Verifying -> Completed ordering.
pub type RemoteEditProgressFn = std::sync::Arc<dyn Fn(crate::jobs::RemoteEditPhase) + Send + Sync>;

/// Backend provider interface — each concrete provider implements this.
/// Sync `list()` remains for simple providers; `list_async()` is preferred.
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
    /// Read bounded prefix from a file. Default: unsupported.
    async fn read_prefix_bytes(
        &self,
        _path: &str,
        _max_bytes: usize,
    ) -> std::io::Result<BoundedRead> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "read_prefix_bytes not supported by this provider",
        ))
    }

    /// Identity-aware bounded prefix read for a listed entry.
    ///
    /// Default: `EntryIdentity::Other` delegates to `read_prefix_bytes` via the
    /// legacy `parent + name` path (exact same validation as
    /// `read_prefix_bytes_at`). Structured provider-native identities
    /// (S3Object/S3Prefix/S3Bucket) fail closed by default so the generic layer
    /// never reconstructs an S3 key from `entry.name` — the concrete provider
    /// overrides this for its native identity.
    // ponytail: default seam; S3 overrides in S3-27, no name->key rewrite here
    async fn read_listed_prefix_bytes(
        &self,
        location: &Location,
        listed: &ListedEntry,
        max_bytes: usize,
    ) -> std::io::Result<BoundedRead> {
        match &listed.identity {
            EntryIdentity::Other => {
                let parent_path = Location::legacy_listing_path(location)?;
                let path = validated_child_path(&parent_path, &listed.entry.name)?;
                self.read_prefix_bytes(&path, max_bytes).await
            }
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "read_listed_prefix_bytes requires a provider-native identity override",
            )),
        }
    }

    /// Write bytes atomically only if the current target still matches the
    /// exact frozen content, Unix mode, and ownership captured before editing.
    /// Default: unsupported.
    async fn write_file_bytes_if_unchanged(
        &self,
        _path: &str,
        _data: &[u8],
        _revision: &RemoteEditRevision,
        _cancellation: &CancellationFlag,
        _progress: Option<crate::vfs::RemoteEditProgressFn>,
    ) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "write_file_bytes not supported by this provider",
        ))
    }

    /// Return filesystem metadata for a file. Default: unsupported.
    async fn metadata(&self, _path: &str) -> std::io::Result<FileMetadata> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "metadata not supported by this provider",
        ))
    }

    /// Read entire file up to a safety cap. Returns the bytes and whether
    /// the file is complete (false = file was larger than max_bytes and
    /// the returned Vec is truncated).
    async fn read_all_capped(
        &self,
        _path: &str,
        _max_bytes: usize,
    ) -> std::io::Result<BoundedRead> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "read_all_capped not supported by this provider",
        ))
    }

    /// Cancellable bounded read. Providers with cleanup-bearing reads should
    /// override this so cancellation runs their cleanup before returning.
    async fn read_all_capped_cancellable(
        &self,
        path: &str,
        max_bytes: usize,
        cancellation: &CancellationFlag,
    ) -> std::io::Result<BoundedRead> {
        tokio::select! {
            _ = cancellation.cancelled() => Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                format!("read cancelled: {path}"),
            )),
            result = self.read_all_capped(path, max_bytes) => result,
        }
    }

    /// Provider-side paginated listing contract.
    ///
    /// `location` is the typed `Location` (NOT a flattened `&str` path), so
    /// typed/native-identity providers distinguish target-root, bucket, and
    /// prefix without encoding them into a pseudo-filesystem path. The default
    /// implementation serves unpaged/string-path-compatible providers: for
    /// `continuation == None` it wraps `list_async` and converts each `Entry`
    /// into `ListedEntry { entry, identity: EntryIdentity::Other }` with
    /// `continuation: None`; for `continuation == Some(..)` on an unpaged
    /// provider it fails closed with `Unsupported` rather than silently
    /// re-running page 1. Typed providers (S3, WebDAV) override this contract.
    async fn list_page(
        &self,
        location: &Location,
        continuation: Option<&ProviderContinuation>,
    ) -> std::io::Result<ProviderListingPage> {
        if continuation.is_some() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "provider does not support listing continuation",
            ));
        }
        let path = Location::legacy_listing_path(location)?;
        let entries = self.list_async(path.as_ref()).await?;
        let listed: Vec<ListedEntry> = entries
            .into_iter()
            .map(|entry| ListedEntry {
                entry,
                identity: EntryIdentity::Other,
            })
            .collect();
        Ok(ProviderListingPage {
            entries: listed,
            continuation: None,
        })
    }
}

/// File metadata returned by a remote provider.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileMetadata {
    pub len: u64,
    pub is_regular: bool,
    /// Unix permission and ownership fields. None if unavailable.
    pub unix_mode: Option<u32>,
    pub unix_uid: Option<u32>,
    pub unix_gid: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ProviderInstanceKey {
    Singleton(ProviderId),
    SftpHost(String),
    ArchiveFile(PathBuf),
    /// One concrete configured S3 target instance, keyed by its config `id`.
    /// Distinct from `Singleton(ProviderId::S3)` (provider class vs instance).
    // ponytail: id stored verbatim; no normalization — S3-06 already validated
    S3Target(String),
    /// One concrete configured WebDAV target instance, keyed by its config `id`.
    // ponytail: id stored verbatim; no normalization — validate_webdav validated
    WebDavTarget(String),
}

#[derive(Debug, Clone)]
pub struct RegisteredProvider {
    pub provider: Arc<dyn VfsProvider>,
    /// Effective capability declaration of THIS concrete instance. Replaces the
    /// former ProviderId-keyed registry map (PACK Q1): two SFTP hosts may carry
    /// different sets without overwriting each other.
    pub capabilities: CapabilitySet,
    /// Concrete S3 provider when this instance is an `S3Target(id)`; `None`
    /// otherwise. For S3 registrations this aliases the exact `provider` Arc
    /// (same underlying `S3Provider`), so the transfer path reuses the listing
    /// client — no second client cache.
    // ponytail: same-instance alias; resolve_s3_provider populates both
    pub s3: Option<Arc<s3::S3Provider>>,
    /// Concrete WebDAV provider when this instance is a `WebDavTarget(id)`;
    /// `None` otherwise. For WebDAV registrations this aliases the exact
    /// `provider` Arc (same underlying `WebDavProvider`), so the transfer path
    /// reuses the listing client — no second client cache.
    // ponytail: same-instance alias; resolve_webdav_provider populates both
    pub webdav: Option<Arc<webdav::WebDavProvider>>,
}

/// Cloneable, async-safe provider registry.
///
/// Each concrete provider instance carries its own effective capability
/// declaration; per-location queries answer from that declaration (PACK Q1).
/// Unregistered locations fall back to the built-in per-kind declarations,
/// except S3/WebDAV which fail closed unless the target id is configured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum S3TargetBinding {
    /// `config.bucket == None` — whole-account / target-root listing.
    AccountRoot,
    /// `config.bucket == Some(bucket)` — bound to exactly this bucket.
    BucketBound(String),
}

#[derive(Debug, Clone)]
pub struct ProviderRegistry {
    providers: Arc<RwLock<HashMap<ProviderInstanceKey, RegisteredProvider>>>,
    // ponytail: configured S3 target inventory (id -> validated config).
    // Populated at startup via register_s3_targets; never builds clients.
    s3_targets: Arc<RwLock<HashMap<String, crate::config::S3TargetConfig>>>,
    // ponytail: configured WebDAV target inventory (id -> validated config).
    // Populated at startup via register_webdav_targets; provider built lazily.
    webdav_targets: Arc<RwLock<HashMap<String, crate::config::WebDavTargetConfig>>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self {
            providers: Arc::new(RwLock::new(HashMap::new())),
            s3_targets: Arc::new(RwLock::new(HashMap::new())),
            webdav_targets: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Install the configured S3 target inventory.
    ///
    /// Synchronous, offline: copies validated target definitions only. Does
    /// NOT construct `S3Provider` instances or AWS clients (DESIGN_S3 §10
    /// lazy per-target model — first client appears inside a provider on use).
    // ponytail: id stored verbatim (no trim/lowercase); config parsing owns
    // validation (S3-06 / validate_s3 already ran).
    pub fn register_s3_targets(&self, targets: &[crate::config::S3TargetConfig]) {
        let mut inventory = self
            .s3_targets
            .write()
            .expect("s3 target inventory poisoned");
        for target in targets {
            inventory.insert(target.id.clone(), target.clone());
        }
    }

    /// Install the configured WebDAV target inventory.
    pub fn register_webdav_targets(&self, targets: &[crate::config::WebDavTargetConfig]) {
        let mut inventory = self
            .webdav_targets
            .write()
            .expect("webdav target inventory poisoned");
        for target in targets {
            inventory.insert(target.id.clone(), target.clone());
        }
    }

    /// Resolve a concrete `WebDavTarget(id)` provider instance, building it on
    /// first resolution and caching under `ProviderInstanceKey::WebDavTarget`.
    /// Unknown target id fails factually/closed.
    pub fn resolve_webdav_provider(
        &self,
        target_id: &str,
    ) -> std::io::Result<Arc<dyn VfsProvider>> {
        let key = ProviderInstanceKey::WebDavTarget(target_id.to_string());
        if let Some(registered) = self
            .providers
            .read()
            .expect("provider registry poisoned")
            .get(&key)
        {
            return Ok(Arc::clone(&registered.provider));
        }
        let target = self
            .webdav_targets
            .read()
            .expect("webdav target inventory poisoned")
            .get(target_id)
            .cloned()
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!(
                        "unknown WebDAV target: {}",
                        crate::config::sanitize_diag(target_id)
                    ),
                )
            })?;
        // Ponytail/Blocker A: resolve the password through the real target-id
        // secret path (keyring, ARX_WEBDAV_<ID>_PASSWORD fallback). `auth` is
        // only the scheme; never treat it as a password. Fail closed if no
        // secret is configured.
        let password = crate::keyring::webdav_secret(&target.id).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "no WebDAV secret configured for target {}",
                    crate::config::sanitize_diag(&target.id)
                ),
            )
        })?;
        // ponytail: BOTH arcs alias the same WebDavProvider allocation — no second
        // client cache; transfer + listing share one provider instance.
        let webdav_provider: Arc<webdav::WebDavProvider> = Arc::new(webdav::WebDavProvider::new(
            webdav::WebDavTarget {
                id: target.id.clone(),
                name: target.name.clone(),
                url: target.url.clone(),
                username: target.username.clone(),
                // ponytail: only the scheme is stored here; the password is
                // resolved per-request from the keyring keyed by id.
                auth: target.auth.clone(),
            },
            password,
        )?);
        let provider: Arc<dyn VfsProvider> = webdav_provider.clone();
        let mut providers = self.providers.write().expect("provider registry poisoned");
        let registered = providers.entry(key).or_insert_with(|| RegisteredProvider {
            provider: Arc::clone(&provider),
            capabilities: capabilities::WEBDAV_CAPABILITIES,
            s3: None,
            webdav: Some(webdav_provider.clone()),
        });
        Ok(Arc::clone(&registered.provider))
    }

    /// Narrow read-only view of a target's binding. `None` for unknown ids.
    pub fn s3_target_binding(&self, target_id: &str) -> Option<S3TargetBinding> {
        let inventory = self
            .s3_targets
            .read()
            .expect("s3 target inventory poisoned");
        let config = inventory.get(target_id)?;
        Some(match &config.bucket {
            None => S3TargetBinding::AccountRoot,
            Some(bucket) => S3TargetBinding::BucketBound(bucket.clone()),
        })
    }

    pub fn insert(
        &self,
        id: ProviderId,
        provider: Box<dyn VfsProvider>,
        capabilities: CapabilitySet,
    ) {
        self.insert_instance(
            ProviderInstanceKey::Singleton(id),
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
            Arc::from(provider),
            capabilities,
        );
    }

    fn insert_instance(
        &self,
        key: ProviderInstanceKey,
        provider: Arc<dyn VfsProvider>,
        capabilities: CapabilitySet,
    ) {
        self.providers
            .write()
            .expect("provider registry poisoned")
            .insert(
                key,
                RegisteredProvider {
                    provider,
                    capabilities,
                    s3: None,
                    webdav: None,
                },
            );
    }

    pub fn get(&self, id: &ProviderId) -> Option<Arc<dyn VfsProvider>> {
        self.providers
            .read()
            .expect("provider registry poisoned")
            .get(&ProviderInstanceKey::Singleton(*id))
            .map(|registered| Arc::clone(&registered.provider))
    }

    /// Capability truth for an EXACT location (PACK Q1).
    ///
    /// Answers offline from the registered instance's own declaration or the
    /// built-in per-provider-kind declaration. Never constructs an S3/WebDAV
    /// provider, never creates clients, never touches keyring/secrets.
    /// S3/WebDAV return `Some` only when the location's target id is present in
    /// this registry's configured inventory; unknown target fails closed (`None`).
    pub fn capabilities_for_location(&self, location: &Location) -> Option<CapabilitySet> {
        let key = Self::instance_key_for_location(location);
        if let Some(registered) = self
            .providers
            .read()
            .expect("provider registry poisoned")
            .get(&key)
        {
            return Some(registered.capabilities);
        }
        match location {
            Location::Local(_) => Some(capabilities::LOCAL_CAPABILITIES),
            Location::Sftp { .. } => Some(capabilities::SFTP_CAPABILITIES),
            Location::Archive { .. } => Some(capabilities::ARCHIVE_CAPABILITIES),
            // ponytail: inventory membership only — no provider construction
            Location::S3 { target, .. } => {
                if self
                    .s3_targets
                    .read()
                    .expect("s3 target inventory poisoned")
                    .contains_key(target)
                {
                    Some(capabilities::S3_CAPABILITIES)
                } else {
                    None
                }
            }
            Location::WebDav { target, .. } => {
                if self
                    .webdav_targets
                    .read()
                    .expect("webdav target inventory poisoned")
                    .contains_key(target)
                {
                    Some(capabilities::WEBDAV_CAPABILITIES)
                } else {
                    None
                }
            }
        }
    }

    pub fn supports_at(&self, location: &Location, capability: Capability) -> bool {
        self.capabilities_for_location(location)
            .is_some_and(|capabilities| capabilities.supports(capability))
    }

    pub fn require_at(&self, location: &Location, capability: Capability) -> Result<(), VfsError> {
        match self.capabilities_for_location(location) {
            Some(caps) if caps.supports(capability) => Ok(()),
            _ => Err(VfsError::UnsupportedOperation {
                provider: location.provider_id(),
                capability,
            }),
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
            // ponytail: concrete-instance identity; not S3 client registration
            Location::S3 { target, .. } => ProviderInstanceKey::S3Target(target.clone()),
            Location::WebDav { target, .. } => ProviderInstanceKey::WebDavTarget(target.clone()),
        }
    }

    pub fn provider_for_location(
        &self,
        loc: &Location,
    ) -> std::io::Result<(Arc<dyn VfsProvider>, String)> {
        let key = Self::instance_key_for_location(loc);

        let path = Location::legacy_listing_path(loc)?;

        if let Some(provider) = self
            .providers
            .read()
            .expect("provider registry poisoned")
            .get(&key)
            .map(|registered| Arc::clone(&registered.provider))
        {
            return Ok((provider, path));
        }

        let (provider, capabilities): (Arc<dyn VfsProvider>, CapabilitySet) = match loc {
            Location::Local(_) => (
                Arc::new(local::LocalProvider),
                capabilities::LOCAL_CAPABILITIES,
            ),
            Location::Sftp { host, .. } => (
                Arc::new(sftp::SftpProvider::new(crate::remote::Host::from_alias(
                    host,
                ))),
                capabilities::SFTP_CAPABILITIES,
            ),
            Location::Archive { archive, .. } => (
                Arc::new(archive::ArchiveProvider {
                    archive: archive.clone(),
                }),
                capabilities::ARCHIVE_CAPABILITIES,
            ),
            Location::WebDav { target, .. } => {
                let provider = self.resolve_webdav_provider(target)?;
                (provider, capabilities::WEBDAV_CAPABILITIES)
            }
            // Deliberate fail-closed boundary: S3 has no string-path form.
            // Typed S3 goes through provider_for_page_location only.
            Location::S3 { .. } => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "S3 provider routing not implemented yet",
                ));
            }
        };

        let mut providers = self.providers.write().expect("provider registry poisoned");
        let registered = providers.entry(key).or_insert_with(|| RegisteredProvider {
            provider: Arc::clone(&provider),
            capabilities,
            s3: None,
            webdav: None,
        });
        let provider = Arc::clone(&registered.provider);
        drop(providers);

        Ok((provider, path))
    }

    /// Typed provider resolver for the page contract.
    ///
    /// Local/SFTP/Archive keep existing behavior. `Location::S3` resolves to a
    /// concrete `S3Target(id)` provider instance via the configured inventory —
    /// never to `Singleton(ProviderId::S3)`, never flattening bucket/prefix.
    // Deliberate second seam (PACK Q3): typed page/native-identity operations
    // resolve S3/WebDAV per configured target without a string path; the
    // string-path seam stays fail-closed for S3 via Location::legacy_listing_path.
    pub fn provider_for_page_location(
        &self,
        loc: &Location,
    ) -> std::io::Result<Arc<dyn VfsProvider>> {
        match loc {
            Location::S3 { target, .. } => self.resolve_s3_provider(target),
            Location::WebDav { target, .. } => self.resolve_webdav_provider(target),
            other => Ok(self.provider_for_location(other)?.0),
        }
    }

    /// Resolve a concrete `S3Target(id)` provider instance.
    ///
    /// Returns the already-registered instance on repeat resolution (stable
    /// `Arc`). Unknown target id fails factually/closed — no Singleton fallback,
    /// no default/first-target substitution. Constructs no AWS client here.
    fn resolve_s3_provider(&self, target_id: &str) -> std::io::Result<Arc<dyn VfsProvider>> {
        let key = ProviderInstanceKey::S3Target(target_id.to_string());

        if let Some(registered) = self
            .providers
            .read()
            .expect("provider registry poisoned")
            .get(&key)
        {
            return Ok(Arc::clone(&registered.provider));
        }

        let target = self
            .s3_targets
            .read()
            .expect("s3 target inventory poisoned")
            .get(target_id)
            .cloned()
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!(
                        "unknown S3 target: {}",
                        crate::config::sanitize_diag(target_id)
                    ),
                )
            })?;

        // ponytail: BOTH arcs alias the same S3Provider allocation — no second
        // client cache; transfer + listing share one provider instance.
        let s3_provider: Arc<s3::S3Provider> = Arc::new(s3::S3Provider::new(target));
        let provider: Arc<dyn VfsProvider> = s3_provider.clone();

        let mut providers = self.providers.write().expect("provider registry poisoned");
        let registered = providers.entry(key).or_insert_with(|| RegisteredProvider {
            provider: Arc::clone(&provider),
            capabilities: capabilities::S3_CAPABILITIES,
            s3: Some(s3_provider.clone()),
            webdav: None,
        });
        Ok(Arc::clone(&registered.provider))
    }

    /// Typed sibling of `resolve_s3_provider`: returns the concrete
    /// `Arc<S3Provider>` (the same allocation stored in `registered.s3`).
    /// Keeps `resolve_s3_provider` unchanged — this reuses its registration so
    /// the AWS client is shared with the listing/transfer paths.
    fn resolve_s3_provider_typed(&self, target_id: &str) -> std::io::Result<Arc<s3::S3Provider>> {
        // Ensure the instance is registered (populates `registered.s3`).
        self.resolve_s3_provider(target_id)?;
        let key = ProviderInstanceKey::S3Target(target_id.to_string());
        let registered = self
            .providers
            .read()
            .expect("provider registry poisoned")
            .get(&key)
            .cloned();
        registered.and_then(|r| r.s3).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "unknown S3 target: {}",
                    crate::config::sanitize_diag(target_id)
                ),
            )
        })
    }

    /// S3-native create-prefix typed seam (S3-54R).
    ///
    /// Creates an empty marker object `<nav_prefix>/<child>/` directly through
    /// the target's AWS client — NO generic filesystem `mkdir_at` /
    /// `validated_child_path` routing, NO bucket creation, NO overwrite of an
    /// existing object. Fails closed on any preflight error that is not an
    /// explicit missing-object result.
    // ponytail: seam takes target from the Location and resolves THAT target's
    // provider, so a Location cannot reach another target's provider.
    pub async fn create_s3_prefix_marker_at(
        &self,
        location: &Location,
        child_name: &str,
    ) -> std::io::Result<S3PrefixRef> {
        let Location::S3 {
            target,
            bucket,
            prefix,
        } = location
        else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "create_s3_prefix_marker_at requires Location::S3",
            ));
        };
        // bucket == None => target root. Creating a bucket is out of scope;
        // this guard proves no bucket-creation path exists.
        let bucket = bucket.clone().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "bucket creation is not supported",
            )
        })?;
        validate_child_name(child_name)?;
        let provider = self.resolve_s3_provider_typed(target)?;
        provider
            .create_prefix_marker(&bucket, prefix, child_name)
            .await
    }

    /// S3-native exact object delete seam (S3-58).
    ///
    /// Deletes ONE exact `S3ObjectRef` via the target's typed provider.
    /// Validates target identity, bucket binding, and non-empty bucket/key.
    /// No prefix recursion, no bucket delete, no `DeleteObjects` batch.
    pub async fn delete_s3_object_exact(&self, object: &s3::S3ObjectRef) -> std::io::Result<()> {
        let provider = self.resolve_s3_provider_typed(&object.target)?;
        provider.delete_object_exact(object).await
    }

    /// S3-native exact delete seam (S3-58 / S3-55 Phase 8).
    ///
    /// Deletes ONE exact object identified by `key` under the `Location::S3`.
    /// The key is taken verbatim from the frozen selection — no normalization,
    /// no prefix recursion, no bucket delete. Bucket must be present (target
    /// root => Unsupported; bucket creation is out of scope).
    // ponytail: seam derives target/bucket from the Location, so a Location
    // cannot reach another target's provider.
    pub async fn delete_s3_at(&self, location: &Location, key: &str) -> std::io::Result<()> {
        let Location::S3 { target, bucket, .. } = location else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "delete_s3_at requires Location::S3",
            ));
        };
        let bucket = bucket.clone().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "bucket creation is not supported",
            )
        })?;
        let object = s3::S3ObjectRef {
            target: target.to_string(),
            bucket,
            key: key.to_string(),
        };
        self.delete_s3_object_exact(&object).await
    }

    /// S3-native empty-prefix-marker proof seam (S3-58P / S3-55 Phase 8).
    ///
    /// Returns `Ok(true)` ONLY when `prefix` names an empty marker (exactly one
    /// zero-byte object equal to the prefix). All other cases => `Ok(false)`
    /// (fail closed). Never paginates; never normalizes the prefix.
    pub async fn prove_empty_s3_prefix_at(
        &self,
        location: &Location,
        prefix: &str,
    ) -> std::io::Result<bool> {
        let Location::S3 { target, bucket, .. } = location else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "prove_empty_s3_prefix_at requires Location::S3",
            ));
        };
        let bucket = bucket.clone().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "bucket creation is not supported",
            )
        })?;
        let provider = self.resolve_s3_provider_typed(target)?;
        let prefix_ref = s3::S3PrefixRef {
            target: target.to_string(),
            bucket,
            prefix: prefix.to_string(),
        };
        provider.prove_empty_prefix_marker(&prefix_ref).await
    }

    /// Resolve the concrete `S3Target(id)` provider for a transfer operation.
    ///
    /// Returns the SAME `Arc<S3Provider>` already registered under
    /// `ProviderInstanceKey::S3Target(target_id)` (the listing path uses the same
    /// resolver, so the AWS client is shared — no second client cache). Unknown
    /// target id fails factually with `RegistryError::NotFound`.
    // ponytail: reuses resolve_s3_provider (list_page's resolver); no second inventory
    pub fn s3_provider_for_transfer(
        &self,
        target_id: &str,
    ) -> Result<Arc<s3::S3Provider>, RegistryError> {
        self.resolve_s3_provider(target_id)
            .map_err(|_| RegistryError::NotFound(target_id.to_string()))?;
        let key = ProviderInstanceKey::S3Target(target_id.to_string());
        let registered = self
            .providers
            .read()
            .expect("provider registry poisoned")
            .get(&key)
            .cloned();
        match registered.and_then(|r| r.s3) {
            Some(s3) => Ok(s3),
            None => Err(RegistryError::NotFound(target_id.to_string())),
        }
    }

    /// Resolve the concrete `WebDavTarget(id)` provider for a transfer operation.
    ///
    /// Returns the SAME `Arc<WebDavProvider>` already registered under
    /// `ProviderInstanceKey::WebDavTarget(target_id)` (the listing path uses the same
    /// resolver, so the HTTP client is shared — no second client cache). Unknown
    /// target id fails factually with `RegistryError::NotFound`.
    // ponytail: reuses resolve_webdav_provider (list_page's resolver); no second inventory
    pub fn webdav_provider_for_transfer(
        &self,
        target_id: &str,
    ) -> Result<Arc<webdav::WebDavProvider>, RegistryError> {
        self.resolve_webdav_provider(target_id)
            .map_err(|_| RegistryError::NotFound(target_id.to_string()))?;
        let key = ProviderInstanceKey::WebDavTarget(target_id.to_string());
        let registered = self
            .providers
            .read()
            .expect("provider registry poisoned")
            .get(&key)
            .cloned();
        match registered.and_then(|r| r.webdav) {
            Some(webdav) => Ok(webdav),
            None => Err(RegistryError::NotFound(target_id.to_string())),
        }
    }

    /// Same concrete provider/client instance as listing and transfer.
    pub fn webdav_provider_for_mutation(
        &self,
        target_id: &str,
    ) -> Result<Arc<webdav::WebDavProvider>, RegistryError> {
        self.webdav_provider_for_transfer(target_id)
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

    /// Provider-side paginated listing entry point.
    ///
    /// Selects the concrete provider via the typed page/native-identity
    /// resolver and invokes the provider `list_page` contract: configured S3
    /// and WebDAV locations resolve their exact configured target provider;
    /// unknown target IDs fail closed. No typed identity/path flattening
    /// occurs. (The separate string-path seam `provider_for_location` is where
    /// S3 fails closed before any provider construction.)
    pub async fn list_page(
        &self,
        loc: &Location,
        continuation: Option<&ProviderContinuation>,
    ) -> std::io::Result<ProviderListingPage> {
        let provider = self.provider_for_page_location(loc)?;
        provider.list_page(loc, continuation).await
    }

    /// Create directory at frozen location. Routes to correct host instance.
    pub async fn mkdir_at(&self, location: &Location, child_name: &str) -> std::io::Result<()> {
        let (provider, parent_path) = self.provider_for_location(location)?;
        let path = validated_child_path(&parent_path, child_name)?;
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
    ) -> std::io::Result<BoundedRead> {
        let (provider, parent_path) = self.provider_for_location(location)?;
        let path = validated_child_path(&parent_path, name)?;
        provider.read_prefix_bytes(&path, max_bytes).await
    }

    /// Identity-aware bounded prefix read at a location.
    ///
    /// Resolves the concrete provider instance via the same resolver as
    /// `list_page` (so `Location::S3` maps to its `S3Target` instance, never the
    /// legacy fail-closed `S3` path) and dispatches the provider's
    /// `read_listed_prefix_bytes` identity seam. No name flattening: the exact
    /// `ListedEntry` identity is forwarded untouched.
    // ponytail: mirrors list_page routing; identity wins, no name->key rewrite
    pub async fn read_listed_prefix_bytes_at(
        &self,
        location: &Location,
        listed: &ListedEntry,
        max_bytes: usize,
    ) -> std::io::Result<BoundedRead> {
        let provider = self.provider_for_page_location(location)?;
        provider
            .read_listed_prefix_bytes(location, listed, max_bytes)
            .await
    }

    pub async fn write_file_bytes_if_unchanged_at(
        &self,
        location: &Location,
        name: &str,
        data: &[u8],
        revision: &RemoteEditRevision,
        cancellation: &CancellationFlag,
        progress: Option<crate::vfs::RemoteEditProgressFn>,
    ) -> std::io::Result<()> {
        self.require_at(location, Capability::Write)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::Unsupported, error))?;
        let (provider, parent_path) = self.provider_for_location(location)?;
        let path = validated_child_path(&parent_path, name)?;
        provider
            .write_file_bytes_if_unchanged(&path, data, revision, cancellation, progress)
            .await
    }

    pub async fn metadata_at(
        &self,
        location: &Location,
        name: &str,
    ) -> std::io::Result<FileMetadata> {
        let (provider, parent_path) = self.provider_for_location(location)?;
        let path = validated_child_path(&parent_path, name)?;
        provider.metadata(&path).await
    }

    pub async fn read_all_capped_at(
        &self,
        location: &Location,
        name: &str,
        max_bytes: usize,
    ) -> std::io::Result<BoundedRead> {
        let (provider, parent_path) = self.provider_for_location(location)?;
        let path = validated_child_path(&parent_path, name)?;
        provider.read_all_capped(&path, max_bytes).await
    }

    pub async fn read_all_capped_cancellable_at(
        &self,
        location: &Location,
        name: &str,
        max_bytes: usize,
        cancellation: &CancellationFlag,
    ) -> std::io::Result<BoundedRead> {
        let (provider, parent_path) = self.provider_for_location(location)?;
        let path = validated_child_path(&parent_path, name)?;
        provider
            .read_all_capped_cancellable(&path, max_bytes, cancellation)
            .await
    }
}

fn validated_child_path(parent: &str, name: &str) -> std::io::Result<String> {
    validate_child_name(name)?;
    Ok(format!("{}/{}", parent.trim_end_matches('/'), name))
}

/// Validate one child name before joining it to a provider path.
/// Rejects: empty, ".", "..", names containing '/' or NUL.
pub fn validate_child_name(name: &str) -> std::io::Result<()> {
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
    /// Typed S3 location: exact target id, optional bucket, navigation prefix.
    /// bucket=None => target root; Some+"" => bucket root; Some+prefix => listing
    /// prefix. Fields stored verbatim (no fs normalization).
    // Typed identity: S3 routing happens through the typed page resolver.
    S3 {
        target: String,
        bucket: Option<String>,
        prefix: String,
    },
    /// Typed WebDAV location: exact target id + logical path (verbatim).
    // ponytail: WebDAV identity = target id + path; no bucket/prefix semantics
    WebDav {
        target: String,
        path: String,
    },
}

/// Escape terminal-control characters for safe presentation only.
/// Printable Unicode is preserved unchanged; stored identity is never mutated.
// ponytail: display-only escaping, no normalization/lookup
fn s3_display_safe_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\x1b' => out.push_str("\\x1b"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
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
            // ponytail: S3-10 control-safe display; presentation only, identity untouched
            Self::S3 {
                target,
                bucket,
                prefix,
            } => match bucket {
                None => write!(f, "[S3 {}]", s3_display_safe_component(target)),
                Some(bucket) => {
                    let safe_bucket = s3_display_safe_component(bucket);
                    if prefix.is_empty() {
                        write!(f, "s3://{safe_bucket}/")
                    } else {
                        let trailing = if prefix.ends_with('/') { "" } else { "/" };
                        write!(
                            f,
                            "s3://{}/{}{}",
                            safe_bucket,
                            s3_display_safe_component(prefix),
                            trailing
                        )
                    }
                }
            },
            Self::WebDav { target, path } => {
                write!(f, "[WebDAV {}] {}", s3_display_safe_component(target), path)
            }
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
            Self::S3 { .. } => ProviderId::S3,
            Self::WebDav { .. } => ProviderId::WebDAV,
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
            // Intentionally generic pane label; exact S3 target identity lives
            // in the Location / ProviderInstanceKey / typed refs, never derived
            // from presentation text.
            Self::S3 { .. } => "S3".to_string(),
            Self::WebDav { target, path } => {
                let last = path.rsplit('/').next().unwrap_or(path);
                format!("{target}:{last}")
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
            // Deliberately fail-closed identity: generic child(name) must not
            // invent S3 object identity; dedicated S3 paths own navigation.
            Self::S3 { .. } => self.clone(),
            Self::WebDav { target, path } => {
                let base = path.trim_end_matches('/');
                let child = if base.is_empty() || base == "/" {
                    format!("/{name}")
                } else {
                    format!("{base}/{name}")
                };
                Self::WebDav {
                    target: target.clone(),
                    path: child,
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
            // ponytail: navigation prefix component only; not object key / provider addr
            Self::S3 { prefix, .. } => prefix.as_str(),
            Self::WebDav { path, .. } => path,
        }
    }

    /// String-path adapter conversion for unpaged-provider listing.
    ///
    /// Maps Local -> filesystem path, SFTP -> remote path, Archive -> inner
    /// path, WebDAV -> logical path; S3 is Unsupported (never flattened).
    ///
    /// Used by both `provider_for_location` and the default `list_page` adapter so
    /// the two listing paths cannot drift. For `Location::Local` an unrepresentable
    /// (non-UTF8) path is lossily converted — it is NOT silently retargeted to
    /// filesystem root "/". S3 stays typed/fail-closed and is never flattened.
    // ponytail: one conversion rule; the page resolver never uses this for S3.
    pub(crate) fn legacy_listing_path(location: &Location) -> std::io::Result<String> {
        match location {
            Location::Local(path) => Ok(path.to_string_lossy().into_owned()),
            Location::Sftp { path, .. } => Ok(path.clone()),
            Location::Archive { inner_path, .. } => Ok(inner_path.clone()),
            // Deliberate fail-closed boundary: no bucket+prefix flatten, ever.
            Location::S3 { .. } => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "S3 provider routing not implemented yet",
            )),
            Location::WebDav { path, .. } => Ok(path.clone()),
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
            // Deliberately fail-closed: generic parent must not invent S3
            // virtual-parent semantics; dedicated S3 paths own that.
            Self::S3 { .. } => None,
            Self::WebDav { target, path } => {
                let current = path.trim_end_matches('/');
                if current.is_empty() {
                    return None;
                }
                let parent = current
                    .rsplit_once('/')
                    .map(|(parent, _)| if parent.is_empty() { "/" } else { parent })
                    .unwrap_or("/");
                Some(Self::WebDav {
                    target: target.clone(),
                    path: parent.to_string(),
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

/// Presentation/listing metadata plus authoritative provider-native identity.
///
/// `entry` is presentation/listing metadata (name shown in the pane, kind,
/// size, mtime). For providers with an authoritative operational identity that
/// is NOT reconstructable from `parent + entry.name` (notably S3, where a key
/// like `foo//bar` or `foo/../bar` is opaque), `identity` carries that exact
/// ref. For everything still using the existing identity model, `Other` is the
/// safe compatibility identity.
// ponytail: presentation metadata never becomes operational identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedEntry {
    pub entry: Entry,
    pub identity: EntryIdentity,
}

/// Authoritative operational identity for a listed entry.
///
/// For S3, `entry.name` is presentation only; the `*Ref` variants hold the
/// exact provider-native key/prefix/bucket. No helper may reconstruct a key
/// from parent Location + `entry.name` — the ref wins.
// ponytail: S3 identity derived from exact *Ref; Other keeps non-S3 compat
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryIdentity {
    S3Object(s3::S3ObjectRef),
    S3Prefix(s3::S3PrefixRef),
    S3Bucket(s3::S3BucketRef),
    WebDavObject(webdav::WebDavObjectRef),
    WebDavCollection(webdav::WebDavCollectionRef),
    Other,
}

/// Opaque, provider-native pagination continuation token.
///
/// The token is treated as opaque bytes: not trimmed, not normalized, not
/// parsed, and no provider kind / pane id / generation / location / S3
/// target / bucket / prefix / page number is encoded into it. An exact value
/// such as `"  opaque+/=token 日本語  "` must survive storage verbatim.
// ponytail: provider-side only; pane correlation lives in S3-14/S3-15
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderContinuation {
    pub token: String,
}

/// One provider-side page of listed entries with authoritative identity.
///
/// `entries` carries `ListedEntry` (presentation + exact identity). `continuation`
/// is `None` when the page is the last page (or the provider is unpaged). This is
/// the provider contract only — no pane-layer correlation data is attached here.
// ponytail: provider page boundary; S3-14/S3-15 own pane correlation/staleness
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderListingPage {
    pub entries: Vec<ListedEntry>,
    pub continuation: Option<ProviderContinuation>,
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

/// Immutable snapshot of a remote file before editing.
/// Used to detect remote changes before write-back.
#[derive(Clone)]
pub struct RemoteEditSession {
    pub name: String,
    pub location: Location,
    pub editor: String,
    /// Exact content+mode revision captured by one stable provider read.
    pub revision: RemoteEditRevision,
    /// Secure unique temp directory (auto-cleaned on drop).
    /// Contains `working` (editable copy) and `original` (immutable snapshot).
    pub temp_dir: std::sync::Arc<tempfile::TempDir>,
    pub state: RemoteEditState,
    /// JobManager job id tracking this edit across all phases (download→editor→writeback).
    pub job_id: Option<String>,
}

impl std::fmt::Debug for RemoteEditSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteEditSession")
            .field("name", &self.name)
            .field("location", &self.location)
            .field("editor", &self.editor)
            .field("revision", &self.revision)
            .field("temp_dir", &self.temp_dir.path())
            .field("state", &self.state)
            .finish()
    }
}

impl PartialEq for RemoteEditSession {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.location == other.location
            && self.editor == other.editor
            && self.revision == other.revision
            && self.state == other.state
    }
}

impl Eq for RemoteEditSession {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteEditState {
    ReadyToEdit,
    Editing,
    NoChange,
    WritingBack,
    Conflict,
    Failed,
}

pub(crate) fn canonical_unix_mtime_ms(seconds: u64) -> u64 {
    seconds.saturating_mul(1_000)
}

pub(crate) fn canonical_system_mtime_ms(time: std::time::SystemTime) -> Option<u64> {
    let duration = time.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(canonical_unix_mtime_ms(duration.as_secs()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // S3-08: S3Target is concrete-instance identity, distinct from class/other kinds.
    #[test]
    fn s3_target_distinct_ids() {
        let a = ProviderInstanceKey::S3Target("prod".into());
        let b = ProviderInstanceKey::S3Target("backups".into());
        assert_ne!(a, b);
    }

    #[test]
    fn s3_target_distinct_from_class() {
        let class_key = ProviderInstanceKey::Singleton(ProviderId::S3);
        let instance_key = ProviderInstanceKey::S3Target("prod".into());
        assert_ne!(class_key, instance_key);
    }

    #[test]
    fn s3_target_distinct_from_sftp_same_string() {
        let s3 = ProviderInstanceKey::S3Target("prod".into());
        let sftp = ProviderInstanceKey::SftpHost("prod".into());
        assert_ne!(s3, sftp);
    }

    #[test]
    fn s3_target_map_identity() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(ProviderInstanceKey::S3Target("prod".into()));
        set.insert(ProviderInstanceKey::S3Target("backups".into()));
        set.insert(ProviderInstanceKey::SftpHost("prod".into()));
        set.insert(ProviderInstanceKey::Singleton(ProviderId::S3));
        assert_eq!(set.len(), 4);
    }

    #[test]
    fn s3_target_stable_identity() {
        let a = ProviderInstanceKey::S3Target("prod".into());
        let b = ProviderInstanceKey::S3Target("prod".into());
        assert_eq!(a, b);
        use std::collections::HashMap;
        let mut map = HashMap::new();
        map.insert(a, 1u32);
        assert_eq!(map.get(&b), Some(&1));
    }

    #[test]
    fn s3_target_no_normalization() {
        let exact = ProviderInstanceKey::S3Target("prod".into());
        let spaced = ProviderInstanceKey::S3Target(" prod ".into());
        assert_ne!(exact, spaced);
    }

    // S3-09: typed S3 Location + fail-closed navigation/routing.
    #[test]
    fn s3_location_target_root_fields() {
        let loc = Location::S3 {
            target: "aws".into(),
            bucket: None,
            prefix: "".into(),
        };
        assert_eq!(loc.provider_id(), ProviderId::S3);
        if let Location::S3 {
            target,
            bucket,
            prefix,
        } = &loc
        {
            assert_eq!(target, "aws");
            assert_eq!(*bucket, None);
            assert_eq!(prefix, "");
        } else {
            panic!("not S3");
        }
    }

    #[test]
    fn s3_location_bucket_root() {
        let loc = Location::S3 {
            target: "aws".into(),
            bucket: Some("company-artifacts".into()),
            prefix: "".into(),
        };
        assert_eq!(loc.provider_id(), ProviderId::S3);
        if let Location::S3 { bucket, prefix, .. } = &loc {
            assert_eq!(bucket.as_deref(), Some("company-artifacts"));
            assert_eq!(prefix, "");
        } else {
            panic!("not S3");
        }
    }

    #[test]
    fn s3_location_nested_prefix() {
        let loc = Location::S3 {
            target: "aws".into(),
            bucket: Some("company-artifacts".into()),
            prefix: "releases/2026".into(),
        };
        assert_eq!(loc.path_for_listing(), "releases/2026");
    }

    #[test]
    fn s3_location_unicode_prefix_preserved() {
        let p = "δοκιμή/данные/日本語";
        let loc = Location::S3 {
            target: "aws".into(),
            bucket: Some("b".into()),
            prefix: p.into(),
        };
        assert_eq!(loc.path_for_listing(), p);
    }

    #[test]
    fn s3_location_double_slash_preserved() {
        let p = "foo//bar";
        let loc = Location::S3 {
            target: "aws".into(),
            bucket: Some("b".into()),
            prefix: p.into(),
        };
        assert_eq!(loc.path_for_listing(), p);
    }

    #[test]
    fn s3_location_dot_segments_preserved() {
        for p in ["foo/../bar", "foo/./bar"] {
            let loc = Location::S3 {
                target: "aws".into(),
                bucket: Some("b".into()),
                prefix: p.into(),
            };
            assert_eq!(loc.path_for_listing(), p, "prefix must not be normalized");
        }
    }

    #[test]
    fn s3_instance_key_is_target() {
        let loc = Location::S3 {
            target: "aws".into(),
            bucket: None,
            prefix: "".into(),
        };
        assert_eq!(
            ProviderRegistry::instance_key_for_location(&loc),
            ProviderInstanceKey::S3Target("aws".into())
        );
    }

    #[test]
    fn s3_target_id_preserved_verbatim() {
        let loc = Location::S3 {
            target: " aws ".into(),
            bucket: None,
            prefix: "".into(),
        };
        // Generic control-safe label by design; exact target preserved in
        // instance key / typed identity, not in the label.
        assert_eq!(loc.label(), "S3");
        assert_eq!(
            ProviderRegistry::instance_key_for_location(&loc),
            ProviderInstanceKey::S3Target(" aws ".into())
        );
    }

    #[test]
    fn s3_display_label_control_safe() {
        // T1 — target root
        let loc = Location::S3 {
            target: "artifacts".into(),
            bucket: None,
            prefix: "".into(),
        };
        assert_eq!(format!("{loc}"), "[S3 artifacts]");

        // T2 — target root control safety
        let loc = Location::S3 {
            target: "prod\x1b[31m\nEVIL".into(),
            bucket: None,
            prefix: "".into(),
        };
        let displayed = format!("{loc}");
        assert!(!displayed.contains('\x1b')); // no raw ESC
        assert!(!displayed.contains('\n')); // no raw newline (only escaped \n)
        if let Location::S3 { target, .. } = &loc {
            assert_eq!(target, "prod\x1b[31m\nEVIL");
        } else {
            panic!("not S3");
        }

        // T3 — bucket root
        let loc = Location::S3 {
            target: "aws".into(),
            bucket: Some("company-artifacts".into()),
            prefix: "".into(),
        };
        assert_eq!(format!("{loc}"), "s3://company-artifacts/");

        // T4 — normal nested prefix
        let loc = Location::S3 {
            target: "aws".into(),
            bucket: Some("company-artifacts".into()),
            prefix: "releases/2026".into(),
        };
        assert_eq!(format!("{loc}"), "s3://company-artifacts/releases/2026/");

        // T5 — already trailing slash (exactly one display slash)
        let loc = Location::S3 {
            target: "aws".into(),
            bucket: Some("company-artifacts".into()),
            prefix: "releases/2026/".into(),
        };
        assert_eq!(format!("{loc}"), "s3://company-artifacts/releases/2026/");
        assert_eq!(loc.path_for_listing(), "releases/2026/");

        // T6 — double slash preserved
        let loc = Location::S3 {
            target: "aws".into(),
            bucket: Some("company-artifacts".into()),
            prefix: "foo//bar".into(),
        };
        assert_eq!(format!("{loc}"), "s3://company-artifacts/foo//bar/");

        // T7 — dot-dot segment preserved
        let loc = Location::S3 {
            target: "aws".into(),
            bucket: Some("company-artifacts".into()),
            prefix: "foo/../bar".into(),
        };
        assert_eq!(format!("{loc}"), "s3://company-artifacts/foo/../bar/");

        // T8 — dot segment preserved
        let loc = Location::S3 {
            target: "aws".into(),
            bucket: Some("company-artifacts".into()),
            prefix: "foo/./bar".into(),
        };
        assert_eq!(format!("{loc}"), "s3://company-artifacts/foo/./bar/");

        // T9 — Unicode preserved
        let loc = Location::S3 {
            target: "aws".into(),
            bucket: Some("company-artifacts".into()),
            prefix: "данные/日本語".into(),
        };
        assert_eq!(format!("{loc}"), "s3://company-artifacts/данные/日本語/");

        // T10 — prefix control character escaped
        let loc = Location::S3 {
            target: "aws".into(),
            bucket: Some("company-artifacts".into()),
            prefix: "foo\nEVIL".into(),
        };
        let displayed = format!("{loc}");
        assert!(!displayed.contains('\n')); // only escaped \n present
        assert_eq!(loc.path_for_listing(), "foo\nEVIL");

        // T11 — bucket control character escaped
        let loc = Location::S3 {
            target: "aws".into(),
            bucket: Some("bucket\x1b[31m".into()),
            prefix: "".into(),
        };
        let displayed = format!("{loc}");
        assert!(!displayed.contains('\x1b'));
        if let Location::S3 { bucket, .. } = &loc {
            assert_eq!(bucket.as_deref(), Some("bucket\x1b[31m"));
        } else {
            panic!("not S3");
        }

        // T12 — leading slash preservation (no normalization)
        let loc = Location::S3 {
            target: "aws".into(),
            bucket: Some("b".into()),
            prefix: "/foo".into(),
        };
        assert_eq!(format!("{loc}"), "s3://b//foo/");

        // T13 — Display does not alter label
        let loc = Location::S3 {
            target: "artifacts".into(),
            bucket: Some("company-artifacts".into()),
            prefix: "releases/2026".into(),
        };
        assert_eq!(loc.label(), "S3");
    }

    #[test]
    fn s3_child_is_fail_closed() {
        let loc = Location::S3 {
            target: "aws".into(),
            bucket: Some("b".into()),
            prefix: "p".into(),
        };
        assert_eq!(loc.child("presentation-name"), loc);
    }

    #[test]
    fn s3_parent_is_fail_closed() {
        let loc = Location::S3 {
            target: "aws".into(),
            bucket: Some("b".into()),
            prefix: "p".into(),
        };
        assert_eq!(loc.parent(), None);
    }

    #[test]
    fn s3_string_path_routing_stays_unsupported() {
        let loc = Location::S3 {
            target: "aws".into(),
            bucket: Some("b".into()),
            prefix: "p".into(),
        };
        let reg = ProviderRegistry::new();
        let res = reg.provider_for_location(&loc);
        assert!(
            res.is_err(),
            "string-path resolver must keep S3 Unsupported/fail-closed"
        );
    }
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
        let local = Location::Local("/".into());
        assert!(registry.supports_at(&local, Capability::List));
        assert!(registry.supports_at(&local, Capability::Move));
        assert!(!registry.supports_at(&local, Capability::ServerSideCopy));
    }

    #[test]
    fn require_returns_typed_unsupported_operation() {
        let registry = default_registry();
        let error = registry
            .require_at(&Location::Local("/".into()), Capability::ServerSideCopy)
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

    // ── REMOTE-09: validate_child_name ──

    #[test]
    fn validate_child_name_rejects_empty() {
        assert!(validate_child_name("").is_err());
    }

    #[test]
    fn validate_child_name_rejects_dot() {
        assert!(validate_child_name(".").is_err());
    }

    #[test]
    fn validate_child_name_rejects_dotdot() {
        assert!(validate_child_name("..").is_err());
    }

    #[test]
    fn validate_child_name_rejects_slash() {
        assert!(validate_child_name("foo/bar").is_err());
    }

    #[test]
    fn validate_child_name_rejects_nul() {
        assert!(validate_child_name("bad\0name").is_err());
    }

    #[test]
    fn validate_child_name_accepts_normal() {
        assert!(validate_child_name("created-by-arx").is_ok());
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
        let caps = registry
            .capabilities_for_location(&Location::Sftp {
                host: "any-host".into(),
                path: "/".into(),
            })
            .unwrap();
        assert!(caps.supports(Capability::Read));
        assert!(caps.supports(Capability::List));
        assert!(!caps.supports(Capability::Move));
    }

    // ── PACK Q1: concrete-location capability authority ──

    #[test]
    fn q1_two_sftp_instances_carry_independent_capabilities() {
        let registry = ProviderRegistry::new();
        let read_only = CapabilitySet::NONE
            .with(Capability::List)
            .with(Capability::Read);
        assert_ne!(read_only, capabilities::SFTP_CAPABILITIES);
        registry.insert_sftp("host-a", Box::new(local::LocalProvider), read_only);
        registry.insert_sftp(
            "host-b",
            Box::new(local::LocalProvider),
            capabilities::SFTP_CAPABILITIES,
        );
        let loc_a = Location::Sftp {
            host: "host-a".into(),
            path: "/data".into(),
        };
        let loc_b = Location::Sftp {
            host: "host-b".into(),
            path: "/data".into(),
        };
        assert_eq!(
            registry.capabilities_for_location(&loc_a),
            Some(read_only),
            "host-a must keep exactly its supplied set"
        );
        assert_eq!(
            registry.capabilities_for_location(&loc_b),
            Some(capabilities::SFTP_CAPABILITIES),
            "host-b must carry exactly its own set"
        );
        // Registering host-b did NOT overwrite host-a (regression for removing
        // the ProviderId-global capability map).
        assert_eq!(registry.capabilities_for_location(&loc_a), Some(read_only));
        assert!(registry.supports_at(&loc_b, Capability::Write));
        assert!(!registry.supports_at(&loc_a, Capability::Write));
    }

    #[test]
    fn q1_unregistered_sftp_gets_builtin_declaration_without_construction() {
        let registry = ProviderRegistry::new();
        let loc = Location::Sftp {
            host: "never-registered".into(),
            path: "/x".into(),
        };
        assert_eq!(
            registry.capabilities_for_location(&loc),
            Some(capabilities::SFTP_CAPABILITIES)
        );
        assert_eq!(
            registry.instance_count(),
            0,
            "capability query must not construct a provider"
        );
    }

    #[test]
    fn q1_local_location_keeps_builtin_declaration() {
        let registry = ProviderRegistry::new();
        let loc = Location::Local("/tmp".into());
        assert_eq!(
            registry.capabilities_for_location(&loc),
            Some(capabilities::LOCAL_CAPABILITIES)
        );
        assert!(registry.supports_at(&loc, Capability::Move));
    }

    #[test]
    fn q1_s3_known_target_serves_builtin_unknown_fails_closed() {
        let registry = ProviderRegistry::new();
        registry.register_s3_targets(&[crate::config::S3TargetConfig {
            id: "known".into(),
            name: "Known Target".into(),
            bucket: Some("bucket".into()),
            region: Some("us-east-1".into()),
            profile: None,
            endpoint_url: Some("http://127.0.0.1:9".into()),
            force_path_style: true,
        }]);
        let known = Location::S3 {
            target: "known".into(),
            bucket: Some("bucket".into()),
            prefix: String::new(),
        };
        let unknown = Location::S3 {
            target: "missing".into(),
            bucket: None,
            prefix: String::new(),
        };
        assert_eq!(
            registry.capabilities_for_location(&known),
            Some(capabilities::S3_CAPABILITIES)
        );
        assert_eq!(
            registry.capabilities_for_location(&unknown),
            None,
            "unknown target fails closed"
        );
        assert_eq!(
            registry.instance_count(),
            0,
            "capability query must not resolve an S3 client/provider"
        );
    }

    #[test]
    fn q1_webdav_known_target_serves_builtin_unknown_fails_closed_no_secret() {
        // No secret is installed: the capability query must never attempt
        // keyring/provider resolution.
        let registry = ProviderRegistry::new();
        registry.register_webdav_targets(&[crate::config::WebDavTargetConfig {
            id: "web".into(),
            name: "Web Target".into(),
            url: "http://127.0.0.1:9/dav".into(),
            username: "user".into(),
            auth: "basic".into(),
        }]);
        let known = Location::WebDav {
            target: "web".into(),
            path: "/".into(),
        };
        let unknown = Location::WebDav {
            target: "missing".into(),
            path: "/".into(),
        };
        assert_eq!(
            registry.capabilities_for_location(&known),
            Some(capabilities::WEBDAV_CAPABILITIES)
        );
        assert_eq!(
            registry.capabilities_for_location(&unknown),
            None,
            "unknown target fails closed"
        );
        assert_eq!(
            registry.instance_count(),
            0,
            "capability query must not construct a WebDAV provider"
        );
    }

    #[test]
    fn webdav_mutation_resolver_reuses_transfer_instance() {
        let registry = ProviderRegistry::new();
        registry.register_webdav_targets(&[crate::config::WebDavTargetConfig {
            id: "mut".into(),
            name: "Mutation Target".into(),
            url: "http://127.0.0.1:9/dav".into(),
            username: "user".into(),
            // ponytail: test only checks Arc identity, not real auth
            auth: "basic".into(),
        }]);
        // A real secret is required to build the provider; stub one via env.
        // set_var is unsafe (data race); test only, single-threaded.
        unsafe {
            std::env::set_var("ARX_WEBDAV_MUT_PASSWORD", "test");
        }
        // Transfer seam and mutation seam must hand out the SAME Arc.
        let transfer = registry.webdav_provider_for_transfer("mut").unwrap();
        let mutation = registry.webdav_provider_for_mutation("mut").unwrap();
        assert!(Arc::ptr_eq(&transfer, &mutation));
        // Repeated mutation resolution reuses the registered instance.
        let again = registry.webdav_provider_for_mutation("mut").unwrap();
        assert!(Arc::ptr_eq(&mutation, &again));
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

    // ── VIEW-09B: routing ──

    use std::sync::Mutex;

    struct RoutingMockProvider {
        host_label: String,
        read_result: Mutex<Option<std::io::Result<BoundedRead>>>,
        // #51/MAJOR#1: capture the progress phases the provider emits so the
        // ordering contract (Verifying before terminal; RollbackOrRecovery at
        // the real recovery transition; no Verifying on pre-verify failure) is
        // asserted without a real SFTP host. Captured via the closure the test
        // passes in `write_file_bytes_if_unchanged` (see below) — no host.
        write_mode: Mutex<WriteMode>,
    }

    #[derive(Clone, Copy)]
    enum WriteMode {
        Success,
        Warning,
        PreVerifyFailure,
        RecoveryRequired,
    }

    impl std::fmt::Debug for RoutingMockProvider {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("RoutingMockProvider")
                .field("host_label", &self.host_label)
                .finish()
        }
    }

    #[async_trait::async_trait]
    impl VfsProvider for RoutingMockProvider {
        fn list(&self, _path: &str) -> std::io::Result<Vec<Entry>> {
            panic!("mock: list not called")
        }
        fn read_head(&self, _path: &str, _lines: usize) -> std::io::Result<Vec<String>> {
            panic!("mock: read_head not called")
        }
        fn copy_files(&self, _src: &str, _dst: &str, _names: &[String]) -> std::io::Result<usize> {
            panic!("mock: copy_files not called")
        }
        fn move_files(&self, _src: &str, _dst: &str, _names: &[String]) -> std::io::Result<usize> {
            panic!("mock: move_files not called")
        }
        fn delete_files(&self, _dir: &str, _names: &[String]) -> std::io::Result<usize> {
            panic!("mock: delete_files not called")
        }
        async fn read_prefix_bytes(
            &self,
            _path: &str,
            _max_bytes: usize,
        ) -> std::io::Result<BoundedRead> {
            self.read_result
                .lock()
                .unwrap()
                .take()
                .expect("mock: read_result already consumed")
        }

        async fn write_file_bytes_if_unchanged(
            &self,
            _path: &str,
            _data: &[u8],
            _revision: &RemoteEditRevision,
            _cancellation: &CancellationFlag,
            progress: Option<RemoteEditProgressFn>,
        ) -> std::io::Result<()> {
            // #51/MAJOR#1: drive the real writeback progress contract through the
            // mock so ordering is asserted without a live SFTP host.
            let mode = *self.write_mode.lock().unwrap();
            match mode {
                WriteMode::Success => {
                    // Verifying is emitted at the real verification boundary,
                    // BEFORE the terminal result — not post-hoc by the TUI.
                    if let Some(p) = &progress {
                        p(crate::jobs::RemoteEditPhase::Verifying);
                    }
                    Ok(())
                }
                WriteMode::Warning => {
                    // CommittedWithWarning shares the same verify merge point, so it
                    // also passes through real Verifying before the terminal result.
                    if let Some(p) = &progress {
                        p(crate::jobs::RemoteEditPhase::Verifying);
                    }
                    Ok(())
                }
                WriteMode::PreVerifyFailure => {
                    // Failure before verification: must NOT fabricate Verifying.
                    Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "pre-verify failure",
                    ))
                }
                WriteMode::RecoveryRequired => {
                    // Real recovery transition: RollbackOrRecovery emitted at the
                    // genuine boundary before the terminal recovery error.
                    if let Some(p) = &progress {
                        p(crate::jobs::RemoteEditPhase::RollbackOrRecovery);
                    }
                    Err(std::io::Error::other("recovery required"))
                }
            }
        }
    }

    fn make_mock(
        host_label: &str,
    ) -> (
        RoutingMockProvider,
        std::sync::Arc<std::sync::Mutex<Vec<crate::jobs::RemoteEditPhase>>>,
    ) {
        let capture = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        (
            RoutingMockProvider {
                host_label: host_label.to_string(),
                read_result: Mutex::new(None),
                write_mode: Mutex::new(WriteMode::Success),
            },
            capture,
        )
    }

    // ponytail: drive writeback progress through a channel and drain buffered
    // sends into the shared capture vec, so ordering is asserted without a host.
    async fn mock_write(
        provider: &RoutingMockProvider,
        cap: &std::sync::Arc<std::sync::Mutex<Vec<crate::jobs::RemoteEditPhase>>>,
        rev: &RemoteEditRevision,
    ) -> std::io::Result<()> {
        // ponytail: synchronous progress capture — same closure shape the TUI
        // supplies. No channel, no spawn; ordering is program-order deterministic.
        let cap2 = cap.clone();
        let progress: crate::vfs::RemoteEditProgressFn = std::sync::Arc::new(move |phase| {
            cap2.lock().unwrap().push(phase);
        });
        provider
            .write_file_bytes_if_unchanged(
                "file.txt",
                b"data",
                rev,
                &CancellationFlag::default(),
                Some(progress),
            )
            .await
    }

    #[tokio::test]
    async fn remote_write_progress_verifying_before_terminal_and_never_on_pre_verify_failure() {
        use crate::jobs::RemoteEditPhase;
        // Success path: Verifying emitted, then terminal Ok.
        let (provider, cap) = make_mock("mock-host");
        let rev = RemoteEditRevision::new(b"abc123".to_vec(), 0o600, 1000, 1000);
        let result = mock_write(&provider, &cap, &rev).await;
        assert!(result.is_ok(), "success write should succeed");
        let seen = cap.lock().unwrap().clone();
        assert_eq!(
            seen,
            vec![RemoteEditPhase::Verifying],
            "Verifying before terminal on success"
        );

        // Pre-verify failure: NO Verifying emitted.
        *provider.write_mode.lock().unwrap() = WriteMode::PreVerifyFailure;
        cap.lock().unwrap().clear();
        let result = mock_write(&provider, &cap, &rev).await;
        assert!(result.is_err(), "pre-verify failure should fail");
        assert!(
            cap.lock().unwrap().is_empty(),
            "Verifying must NOT be fabricated on pre-verify failure"
        );
    }

    #[tokio::test]
    async fn remote_write_progress_ordering_is_deterministic_and_never_late() {
        use crate::jobs::RemoteEditPhase;
        let (provider, cap) = make_mock("mock-host");
        let rev = RemoteEditRevision::new(b"abc123".to_vec(), 0o600, 1000, 1000);

        // Success: exactly one Verifying, nothing after the terminal Ok.
        let r = mock_write(&provider, &cap, &rev).await;
        assert!(r.is_ok());
        assert_eq!(
            cap.lock().unwrap().clone(),
            vec![RemoteEditPhase::Verifying]
        );
        assert_eq!(
            cap.lock()
                .unwrap()
                .iter()
                .filter(|p| **p == RemoteEditPhase::Verifying)
                .count(),
            1,
            "no duplicate Verifying"
        );

        // Warning: same single Verifying merge point, no extra phases.
        *provider.write_mode.lock().unwrap() = WriteMode::Warning;
        cap.lock().unwrap().clear();
        let r = mock_write(&provider, &cap, &rev).await;
        assert!(r.is_ok());
        assert_eq!(
            cap.lock().unwrap().clone(),
            vec![RemoteEditPhase::Verifying]
        );

        // Recovery: RollbackOrRecovery at the real transition, then terminal Err.
        *provider.write_mode.lock().unwrap() = WriteMode::RecoveryRequired;
        cap.lock().unwrap().clear();
        let r = mock_write(&provider, &cap, &rev).await;
        assert!(r.is_err());
        assert_eq!(
            cap.lock().unwrap().clone(),
            vec![RemoteEditPhase::RollbackOrRecovery]
        );

        // Pre-verify failure: no progress at all, no late Verifying.
        *provider.write_mode.lock().unwrap() = WriteMode::PreVerifyFailure;
        cap.lock().unwrap().clear();
        let r = mock_write(&provider, &cap, &rev).await;
        assert!(r.is_err());
        assert!(
            cap.lock().unwrap().is_empty(),
            "no progress before/after terminal on pre-verify failure"
        );
    }

    #[test]
    fn two_sftp_hosts_route_to_different_providers() {
        let r = ProviderRegistry::new();
        r.insert_sftp(
            "host-a",
            Box::new(RoutingMockProvider {
                host_label: "host-a".into(),
                read_result: Mutex::new(Some(Ok(BoundedRead {
                    bytes: b"content from host-a".to_vec(),
                    truncated: false,
                    unix_mode: None,
                    unix_uid: None,
                    unix_gid: None,
                }))),
                write_mode: Mutex::new(WriteMode::Success),
            }),
            capabilities::SFTP_CAPABILITIES,
        );
        r.insert_sftp(
            "host-b",
            Box::new(RoutingMockProvider {
                host_label: "host-b".into(),
                read_result: Mutex::new(Some(Ok(BoundedRead {
                    bytes: b"content from host-b".to_vec(),
                    truncated: false,
                    unix_mode: None,
                    unix_uid: None,
                    unix_gid: None,
                }))),
                write_mode: Mutex::new(WriteMode::Success),
            }),
            capabilities::SFTP_CAPABILITIES,
        );

        let loc_a = Location::Sftp {
            host: "host-a".into(),
            path: "/data.txt".into(),
        };
        let loc_b = Location::Sftp {
            host: "host-b".into(),
            path: "/data.txt".into(),
        };

        let rt = tokio::runtime::Runtime::new().unwrap();
        let bounded_a = rt
            .block_on(r.read_prefix_bytes_at(&loc_a, "data.txt", 1024))
            .unwrap();
        let bounded_b = rt
            .block_on(r.read_prefix_bytes_at(&loc_b, "data.txt", 1024))
            .unwrap();

        assert_eq!(bounded_a.bytes, b"content from host-a");
        assert!(!bounded_a.truncated);
        assert_eq!(bounded_b.bytes, b"content from host-b");
        assert!(!bounded_b.truncated);
        assert_ne!(bounded_a.bytes, bounded_b.bytes);
    }

    #[tokio::test]
    async fn cancellation_wakes_all_waiters_and_late_waiters() {
        let cancellation = CancellationFlag::default();
        let first = tokio::spawn({
            let cancellation = cancellation.clone();
            async move { cancellation.cancelled().await }
        });
        let second = tokio::spawn({
            let cancellation = cancellation.clone();
            async move { cancellation.cancelled().await }
        });
        tokio::task::yield_now().await;
        cancellation.cancel();

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            first.await.unwrap();
            second.await.unwrap();
            cancellation.cancelled().await;
        })
        .await
        .expect("all cancellation waiters must wake");
    }

    #[tokio::test]
    async fn write_boundary_requires_capability_before_provider_call() {
        let registry = ProviderRegistry::new();
        registry.insert_sftp(
            "read-only",
            Box::new(RoutingMockProvider {
                host_label: "read-only".into(),
                read_result: Mutex::new(None),
                write_mode: Mutex::new(WriteMode::Success),
            }),
            CapabilitySet::NONE.with(Capability::Read),
        );
        let location = Location::Sftp {
            host: "read-only".into(),
            path: "/srv".into(),
        };
        let revision = RemoteEditRevision {
            bytes: b"old".to_vec(),
            unix_mode: 0o600,
            unix_uid: 1000,
            unix_gid: 1000,
        };

        let error = registry
            .write_file_bytes_if_unchanged_at(
                &location,
                "file.txt",
                b"new",
                &revision,
                &CancellationFlag::default(),
                None,
            )
            .await
            .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
    }

    #[tokio::test]
    async fn write_boundary_rejects_invalid_child_before_provider_call() {
        let registry = ProviderRegistry::new();
        registry.insert_sftp(
            "host-a",
            Box::new(RoutingMockProvider {
                host_label: "host-a".into(),
                read_result: Mutex::new(None),
                write_mode: Mutex::new(WriteMode::Success),
            }),
            capabilities::SFTP_CAPABILITIES,
        );
        let location = Location::Sftp {
            host: "host-a".into(),
            path: "/srv".into(),
        };
        let revision = RemoteEditRevision {
            bytes: b"old".to_vec(),
            unix_mode: 0o600,
            unix_uid: 1000,
            unix_gid: 1000,
        };

        let error = registry
            .write_file_bytes_if_unchanged_at(
                &location,
                "../escape",
                b"new",
                &revision,
                &CancellationFlag::default(),
                None,
            )
            .await
            .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn sftp_host_routing_is_not_singleton() {
        // Two SFTP hosts must not resolve to the same singleton provider.
        let key_a = ProviderRegistry::instance_key_for_location(&Location::Sftp {
            host: "host-a".into(),
            path: "/".into(),
        });
        let key_b = ProviderRegistry::instance_key_for_location(&Location::Sftp {
            host: "host-b".into(),
            path: "/".into(),
        });

        assert_ne!(key_a, key_b);
        assert!(matches!(key_a, ProviderInstanceKey::SftpHost(h) if h == "host-a"));
        assert!(matches!(key_b, ProviderInstanceKey::SftpHost(h) if h == "host-b"));
    }

    // ── S3-25: S3TargetBinding inventory views ──

    fn s3_binding_registry() -> ProviderRegistry {
        let registry = ProviderRegistry::new();
        registry.register_s3_targets(&[
            crate::config::S3TargetConfig {
                id: "acc".into(),
                name: "acc".into(),
                bucket: None,
                region: None,
                profile: None,
                endpoint_url: None,
                force_path_style: false,
            },
            crate::config::S3TargetConfig {
                id: "bkt".into(),
                name: "bkt".into(),
                bucket: Some("company-artifacts".into()),
                region: None,
                profile: None,
                endpoint_url: None,
                force_path_style: false,
            },
        ]);
        registry
    }

    #[test]
    fn account_target_binding() {
        let registry = s3_binding_registry();
        assert_eq!(
            registry.s3_target_binding("acc"),
            Some(S3TargetBinding::AccountRoot)
        );
    }

    #[test]
    fn bucket_bound_target_binding() {
        let registry = s3_binding_registry();
        assert_eq!(
            registry.s3_target_binding("bkt"),
            Some(S3TargetBinding::BucketBound("company-artifacts".into()))
        );
    }

    #[test]
    fn unknown_target_binding_none() {
        let registry = s3_binding_registry();
        assert_eq!(registry.s3_target_binding("nope"), None);
    }

    #[test]
    fn exact_id_no_normalization() {
        let registry = ProviderRegistry::new();
        registry.register_s3_targets(&[crate::config::S3TargetConfig {
            id: "  prod  ".into(),
            name: "prod".into(),
            bucket: Some("b".into()),
            region: None,
            profile: None,
            endpoint_url: None,
            force_path_style: false,
        }]);
        // ids are stored verbatim: lookup by exact id succeeds, normalized id fails
        assert_eq!(
            registry.s3_target_binding("  prod  "),
            Some(S3TargetBinding::BucketBound("b".into()))
        );
        assert_eq!(registry.s3_target_binding("prod"), None);
    }
}

#[cfg(test)]
mod s3_identity_tests {
    use super::*;
    use crate::vfs::s3::{S3BucketRef, S3ObjectRef, S3PrefixRef};

    #[test]
    fn s3_presentation_differs_from_object_identity() {
        let le = ListedEntry {
            entry: Entry {
                name: "bar".into(),
                kind: EntryKind::File,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::S3Object(S3ObjectRef {
                target: "aws".into(),
                bucket: "b".into(),
                key: "foo//bar".into(),
            }),
        };
        assert_eq!(le.entry.name, "bar");
        match &le.identity {
            EntryIdentity::S3Object(r) => assert_eq!(r.key, "foo//bar"),
            _ => panic!("expected S3Object"),
        }
        assert_ne!(le.entry.name, "foo//bar");
    }

    #[test]
    fn s3_awkward_key_survives_exactly() {
        let le = ListedEntry {
            entry: Entry {
                name: "bar".into(),
                kind: EntryKind::File,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::S3Object(S3ObjectRef {
                target: "aws".into(),
                bucket: "b".into(),
                key: "foo/../bar".into(),
            }),
        };
        match &le.identity {
            EntryIdentity::S3Object(r) => assert_eq!(r.key, "foo/../bar"),
            _ => panic!("expected S3Object"),
        }
    }

    #[test]
    fn s3_unicode_operational_identity() {
        for key in ["каталог/файл.txt", "日本語/資料.txt"] {
            let le = ListedEntry {
                entry: Entry {
                    name: "файл.txt".into(),
                    kind: EntryKind::File,
                    size: None,
                    modified_unix_ms: None,
                },
                identity: EntryIdentity::S3Object(S3ObjectRef {
                    target: "aws".into(),
                    bucket: "b".into(),
                    key: key.into(),
                }),
            };
            match &le.identity {
                EntryIdentity::S3Object(r) => assert_eq!(r.key, key),
                _ => panic!("expected S3Object"),
            }
        }
    }

    #[test]
    fn s3_prefix_identity_exact_while_name_shortened() {
        let le = ListedEntry {
            entry: Entry {
                name: "releases".into(),
                kind: EntryKind::Directory,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::S3Prefix(S3PrefixRef {
                target: "aws".into(),
                bucket: "b".into(),
                prefix: "releases/2026".into(),
            }),
        };
        assert_eq!(le.entry.name, "releases");
        match &le.identity {
            EntryIdentity::S3Prefix(r) => assert_eq!(r.prefix, "releases/2026"),
            _ => panic!("expected S3Prefix"),
        }
    }

    #[test]
    fn s3_bucket_identity_not_label() {
        let le = ListedEntry {
            entry: Entry {
                name: "Production artifacts".into(),
                kind: EntryKind::Directory,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::S3Bucket(S3BucketRef {
                target: "aws".into(),
                bucket: "company-prod-artifacts".into(),
            }),
        };
        assert_eq!(le.entry.name, "Production artifacts");
        match &le.identity {
            EntryIdentity::S3Bucket(r) => assert_eq!(r.bucket, "company-prod-artifacts"),
            _ => panic!("expected S3Bucket"),
        }
        assert_ne!(le.entry.name, "company-prod-artifacts");
    }

    #[test]
    fn other_compatibility_preserves_entry() {
        let original = Entry {
            name: "local-file.txt".into(),
            kind: EntryKind::File,
            size: Some(1024),
            modified_unix_ms: Some(1_700_000_000_000),
        };
        let le = ListedEntry {
            entry: original.clone(),
            identity: EntryIdentity::Other,
        };
        assert_eq!(le.entry, original);
        assert_eq!(le.entry.name, "local-file.txt");
        assert_eq!(le.entry.size, Some(1024));
        assert_eq!(le.entry.modified_unix_ms, Some(1_700_000_000_000));
        assert!(matches!(le.identity, EntryIdentity::Other));
    }
}

#[cfg(test)]
mod s3_list_page_tests {
    use super::*;
    use std::fs;

    #[tokio::test]
    async fn opaque_token_roundtrip_exact() {
        let token = "  opaque+/=token 日本語  ";
        let cont = ProviderContinuation {
            token: token.to_string(),
        };
        assert_eq!(cont.token, token);
        let page = ProviderListingPage {
            entries: vec![],
            continuation: Some(cont.clone()),
        };
        match page.continuation {
            Some(c) => assert_eq!(c.token, token),
            None => panic!("continuation must be preserved exactly"),
        }
    }

    #[tokio::test]
    async fn local_first_page_equivalence() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        fs::write(dir.join("a.txt"), b"a").unwrap();
        fs::write(dir.join("b.log"), b"b").unwrap();
        fs::create_dir(dir.join("sub")).unwrap();

        let registry = ProviderRegistry::new();
        let loc = Location::Local(dir.to_path_buf());
        let legacy = registry.list_location_async(&loc).await.unwrap();

        let page = registry.list_page(&loc, None).await.unwrap();
        assert!(page.continuation.is_none());
        assert_eq!(page.entries.len(), legacy.len());
        for le in &page.entries {
            assert!(matches!(le.identity, EntryIdentity::Other));
            assert!(legacy.iter().any(|e| e == &le.entry));
        }
    }

    #[tokio::test]
    async fn unpaged_provider_continuation_fails_closed() {
        // LocalProvider is unpaged; passing a continuation must fail closed
        // rather than silently re-running page 1.
        let provider = local::LocalProvider;
        let loc = Location::Local(std::path::PathBuf::from("/"));
        let cont = ProviderContinuation { token: "x".into() };
        let err = provider.list_page(&loc, Some(&cont)).await;
        assert!(err.is_err());
        assert_eq!(err.unwrap_err().kind(), std::io::ErrorKind::Unsupported);
    }

    #[test]
    fn registry_resolves_exact_s3_target_provider_for_bucket_scope() {
        // Offline: provider_for_page_location must resolve the exact configured
        // S3Target instance for a bucket scope, without invoking list_page/AWS.
        let registry = ProviderRegistry::new();
        registry.register_s3_targets(&[mk_s3_target("t", None, None, None, false)]);
        let loc = Location::S3 {
            target: "t".into(),
            bucket: Some("some-bucket".into()),
            prefix: String::new(),
        };
        let provider = registry.provider_for_page_location(&loc).unwrap();
        // Exact S3Target instance identity (stable Arc, same as direct resolve).
        let direct = registry.resolve_s3_provider("t").unwrap();
        assert!(std::sync::Arc::ptr_eq(&provider, &direct));
    }

    // ── MAJOR-01: real SFTP registry route, both APIs observe "/srv/data" exactly ──

    use std::sync::Mutex;

    struct ListingMockProvider {
        seen_paths: std::sync::Arc<Mutex<Vec<String>>>,
        entries: Vec<Entry>,
    }

    impl std::fmt::Debug for ListingMockProvider {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("ListingMockProvider").finish()
        }
    }

    #[async_trait::async_trait]
    impl VfsProvider for ListingMockProvider {
        fn list(&self, path: &str) -> std::io::Result<Vec<Entry>> {
            self.seen_paths
                .lock()
                .expect("mock poisoned")
                .push(path.to_string());
            Ok(self.entries.clone())
        }
        fn read_head(&self, _path: &str, _lines: usize) -> std::io::Result<Vec<String>> {
            panic!("mock: read_head not called")
        }
        fn copy_files(&self, _src: &str, _dst: &str, _names: &[String]) -> std::io::Result<usize> {
            panic!("mock: copy_files not called")
        }
        fn move_files(&self, _src: &str, _dst: &str, _names: &[String]) -> std::io::Result<usize> {
            panic!("mock: move_files not called")
        }
        fn delete_files(&self, _dir: &str, _names: &[String]) -> std::io::Result<usize> {
            panic!("mock: delete_files not called")
        }
    }

    #[tokio::test]
    async fn sftp_registry_route_observes_exact_path_on_both_calls() {
        let registry = ProviderRegistry::new();
        let entries = vec![
            Entry {
                name: "a.txt".into(),
                kind: EntryKind::File,
                size: Some(1),
                modified_unix_ms: Some(1_700_000_000_000),
            },
            Entry {
                name: "b.txt".into(),
                kind: EntryKind::File,
                size: Some(2),
                modified_unix_ms: Some(1_700_000_000_000),
            },
        ];
        let seen_paths = std::sync::Arc::new(Mutex::new(Vec::new()));
        let mock = ListingMockProvider {
            seen_paths: seen_paths.clone(),
            entries: entries.clone(),
        };
        registry.insert_sftp("test-host", Box::new(mock), capabilities::SFTP_CAPABILITIES);

        let loc = Location::Sftp {
            host: "test-host".into(),
            path: "/srv/data".into(),
        };

        let legacy = registry.list_location_async(&loc).await.unwrap();
        let page = registry.list_page(&loc, None).await.unwrap();

        // identical entries after projecting ListedEntry.entry
        assert_eq!(legacy, entries);
        assert_eq!(page.entries.len(), entries.len());
        for le in &page.entries {
            assert!(matches!(le.identity, EntryIdentity::Other));
            assert!(legacy.iter().any(|e| e == &le.entry));
        }
        assert!(page.continuation.is_none());

        // mock observed "/srv/data" exactly on BOTH calls — proves real SFTP
        // routing, no Local substitution, no extra path transformation
        assert_eq!(
            *seen_paths.lock().expect("mock poisoned"),
            vec!["/srv/data".to_string(), "/srv/data".to_string()]
        );
    }

    // ── MAJOR-02: non-UTF8 Local path must not fall back to "/" (Unix only) ──

    #[cfg(unix)]
    #[tokio::test]
    async fn local_non_utf8_page_does_not_retarget_to_root() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let temp_root = tempfile::tempdir().unwrap();
        // Fresh empty dir → neither the non-UTF8 path nor its lossy form exists.
        let bad_component = OsString::from_vec(vec![b'b', b'a', b'd', b'-', 0xff]);
        let non_utf8_path = temp_root.path().join(bad_component);
        assert!(non_utf8_path.to_str().is_none());

        let loc = Location::Local(non_utf8_path.clone());

        // legacy semantics: registry routes Location::Local via to_string_lossy()
        let legacy_path = non_utf8_path.to_string_lossy().into_owned();
        let legacy = local::LocalProvider.list_async(&legacy_path).await;
        // page semantics: default adapter now uses the SAME legacy_listing_path()
        let page = local::LocalProvider.list_page(&loc, None).await;

        // Regression: old path_for_listing().unwrap_or("/") would have listed "/"
        // and likely returned Ok. Fixed code propagates the lossy/non-existent
        // path, so both APIs error for this non-existent location.
        assert!(
            legacy.is_err(),
            "legacy listing must fail for non-UTF8 path"
        );
        assert!(page.is_err(), "page listing must NOT fall back to '/'");
    }

    // ── S3-17: registry inventory + typed provider lifecycle ──

    fn mk_s3_target(
        id: &str,
        profile: Option<&str>,
        region: Option<&str>,
        endpoint_url: Option<&str>,
        force_path_style: bool,
    ) -> crate::config::S3TargetConfig {
        crate::config::S3TargetConfig {
            id: id.to_string(),
            name: id.to_string(),
            bucket: None,
            region: region.map(|s| s.to_string()),
            profile: profile.map(|s| s.to_string()),
            endpoint_url: endpoint_url.map(|s| s.to_string()),
            force_path_style,
        }
    }

    #[test]
    fn two_target_inventory_exact() {
        let a = mk_s3_target("aws-prod", Some("prod"), Some("eu-central-1"), None, false);
        let b = mk_s3_target(
            "minio-lab",
            Some("lab"),
            None,
            Some("http://127.0.0.1:9000"),
            true,
        );
        let registry = ProviderRegistry::new();
        registry.register_s3_targets(&[a.clone(), b.clone()]);

        let inv = registry.s3_targets.read().expect("poisoned");
        assert_eq!(inv.len(), 2);
        assert_eq!(
            inv.get("aws-prod").unwrap().region.as_deref(),
            Some("eu-central-1")
        );
        assert_eq!(
            inv.get("aws-prod").unwrap().profile.as_deref(),
            Some("prod")
        );
        assert_eq!(
            inv.get("minio-lab").unwrap().endpoint_url.as_deref(),
            Some("http://127.0.0.1:9000")
        );
        assert!(inv.get("minio-lab").unwrap().force_path_style);
    }

    #[test]
    fn exact_target_id_preserved() {
        // Harmless surrounding characters must be preserved verbatim.
        let t = mk_s3_target(" My Target_01 ", Some("p"), None, None, false);
        let registry = ProviderRegistry::new();
        registry.register_s3_targets(&[t]);
        let inv = registry.s3_targets.read().expect("poisoned");
        assert!(inv.contains_key(" My Target_01 "));
        assert!(!inv.contains_key("my target_01"));
    }

    #[test]
    fn unknown_s3_target_fails_closed() {
        let registry = ProviderRegistry::new();
        registry.register_s3_targets(&[mk_s3_target(
            "aws-prod",
            Some("prod"),
            Some("eu-central-1"),
            None,
            false,
        )]);
        let loc = Location::S3 {
            target: "missing".to_string(),
            bucket: None,
            prefix: "".to_string(),
        };
        let err = registry.provider_for_page_location(&loc);
        assert!(err.is_err());
        assert!(
            !matches!(err.unwrap_err().kind(), std::io::ErrorKind::Unsupported),
            "must not be the legacy Unsupported routing error; unknown target is NotFound/factual"
        );
    }

    #[tokio::test]
    async fn same_target_reuses_provider_instance() {
        let registry = ProviderRegistry::new();
        registry.register_s3_targets(&[mk_s3_target(
            "aws-prod",
            Some("prod"),
            Some("eu-central-1"),
            None,
            false,
        )]);
        let loc = Location::S3 {
            target: "aws-prod".to_string(),
            bucket: None,
            prefix: "".to_string(),
        };
        let first = registry.provider_for_page_location(&loc).unwrap();
        let second = registry.provider_for_page_location(&loc).unwrap();
        assert!(
            Arc::ptr_eq(&first, &second),
            "same target must reuse provider Arc"
        );
    }

    #[tokio::test]
    async fn different_targets_are_not_singleton() {
        let registry = ProviderRegistry::new();
        registry.register_s3_targets(&[
            mk_s3_target("aws-prod", Some("prod"), Some("eu-central-1"), None, false),
            mk_s3_target(
                "minio-lab",
                Some("lab"),
                None,
                Some("http://127.0.0.1:9000"),
                true,
            ),
        ]);
        let a = registry
            .provider_for_page_location(&Location::S3 {
                target: "aws-prod".to_string(),
                bucket: None,
                prefix: "".to_string(),
            })
            .unwrap();
        let b = registry
            .provider_for_page_location(&Location::S3 {
                target: "minio-lab".to_string(),
                bucket: None,
                prefix: "".to_string(),
            })
            .unwrap();
        assert!(!Arc::ptr_eq(&a, &b), "different targets must differ");
        // No Singleton(ProviderId::S3) instance exists.
        assert!(!registry.contains_instance(&ProviderInstanceKey::Singleton(ProviderId::S3)));
        assert!(registry.contains_instance(&ProviderInstanceKey::S3Target("aws-prod".to_string())));
        assert!(
            registry.contains_instance(&ProviderInstanceKey::S3Target("minio-lab".to_string()))
        );
    }

    #[test]
    fn s3_provider_for_page_location_resolves_configured_target() {
        // Offline: a configured S3 target (with profile/region) resolves to its
        // exact S3Target provider instance for a bucket scope, no AWS call.
        let registry = ProviderRegistry::new();
        registry.register_s3_targets(&[mk_s3_target(
            "aws-prod",
            Some("prod"),
            Some("eu-central-1"),
            None,
            false,
        )]);
        let loc = Location::S3 {
            target: "aws-prod".to_string(),
            bucket: Some("some-bucket".into()),
            prefix: "".to_string(),
        };
        let provider = registry.provider_for_page_location(&loc).unwrap();
        let direct = registry.resolve_s3_provider("aws-prod").unwrap();
        assert!(std::sync::Arc::ptr_eq(&provider, &direct));
    }

    // ── S3-27R: identity-aware preview seam (B, C, D, E, F) ──

    struct PreviewSeamMockProvider {
        read_calls: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl std::fmt::Debug for PreviewSeamMockProvider {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("PreviewSeamMockProvider").finish()
        }
    }

    #[async_trait::async_trait]
    impl VfsProvider for PreviewSeamMockProvider {
        fn list(&self, _path: &str) -> std::io::Result<Vec<Entry>> {
            panic!("mock: list not called")
        }
        fn read_head(&self, _path: &str, _lines: usize) -> std::io::Result<Vec<String>> {
            panic!("mock: read_head not called")
        }
        fn copy_files(&self, _src: &str, _dst: &str, _names: &[String]) -> std::io::Result<usize> {
            panic!("mock: copy_files not called")
        }
        fn move_files(&self, _src: &str, _dst: &str, _names: &[String]) -> std::io::Result<usize> {
            panic!("mock: move_files not called")
        }
        fn delete_files(&self, _dir: &str, _names: &[String]) -> std::io::Result<usize> {
            panic!("mock: delete_files not called")
        }
        async fn read_prefix_bytes(
            &self,
            path: &str,
            _max_bytes: usize,
        ) -> std::io::Result<BoundedRead> {
            self.read_calls
                .lock()
                .expect("mock poisoned")
                .push(path.to_string());
            Ok(BoundedRead {
                bytes: b"data".to_vec(),
                truncated: false,
                unix_mode: None,
                unix_uid: None,
                unix_gid: None,
            })
        }
    }

    fn listed_other(name: &str, size: Option<u64>) -> ListedEntry {
        ListedEntry {
            entry: Entry {
                name: name.into(),
                kind: EntryKind::File,
                size,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::Other,
        }
    }

    fn listed_s3_object(target: &str, bucket: &str, key: &str) -> ListedEntry {
        ListedEntry {
            entry: Entry {
                name: "display.txt".into(),
                kind: EntryKind::File,
                size: Some(10),
                modified_unix_ms: None,
            },
            identity: EntryIdentity::S3Object(s3::S3ObjectRef {
                target: target.into(),
                bucket: bucket.into(),
                key: key.into(),
            }),
        }
    }

    // S3-27R B: SFTP (Other) still uses the legacy name path.
    #[tokio::test]
    async fn s3_27r_other_delegates_to_legacy_name() {
        let mock = PreviewSeamMockProvider {
            read_calls: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        };
        let loc = Location::Sftp {
            host: "h".into(),
            path: "/srv/data".into(),
        };
        let listed = listed_other("file.txt", Some(4));
        let res = mock.read_listed_prefix_bytes(&loc, &listed, 100).await;
        assert!(res.is_ok());
        let calls = mock.read_calls.lock().unwrap();
        assert_eq!(*calls, vec!["/srv/data/file.txt".to_string()]);
    }

    // S3-27R C: structured S3 identity never invokes validated_child_path(name).
    #[tokio::test]
    async fn s3_27r_s3object_never_reconstructs_name() {
        let mock = PreviewSeamMockProvider {
            read_calls: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        };
        let loc = Location::Sftp {
            host: "h".into(),
            path: "/srv/data".into(),
        };
        let listed = listed_s3_object("Prod", "Bucket", "foo/../REAL//x.txt");
        let err = mock
            .read_listed_prefix_bytes(&loc, &listed, 100)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::Unsupported);
        assert!(mock.read_calls.lock().unwrap().is_empty());
    }

    // S3-27R D: S3Prefix fails closed.
    #[tokio::test]
    async fn s3_27r_s3prefix_fails_closed() {
        let mock = PreviewSeamMockProvider {
            read_calls: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        };
        let listed = ListedEntry {
            entry: Entry {
                name: "p".into(),
                kind: EntryKind::Directory,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::S3Prefix(s3::S3PrefixRef {
                target: "Prod".into(),
                bucket: "B".into(),
                prefix: "x/".into(),
            }),
        };
        let err = mock
            .read_listed_prefix_bytes(
                &Location::Sftp {
                    host: "h".into(),
                    path: "/".into(),
                },
                &listed,
                100,
            )
            .await
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::Unsupported);
        assert!(mock.read_calls.lock().unwrap().is_empty());
    }

    // S3-27R E: S3Bucket fails closed.
    #[tokio::test]
    async fn s3_27r_s3bucket_fails_closed() {
        let mock = PreviewSeamMockProvider {
            read_calls: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        };
        let listed = ListedEntry {
            entry: Entry {
                name: "b".into(),
                kind: EntryKind::Directory,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::S3Bucket(s3::S3BucketRef {
                target: "Prod".into(),
                bucket: "B".into(),
            }),
        };
        let err = mock
            .read_listed_prefix_bytes(
                &Location::Sftp {
                    host: "h".into(),
                    path: "/".into(),
                },
                &listed,
                100,
            )
            .await
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::Unsupported);
        assert!(mock.read_calls.lock().unwrap().is_empty());
    }

    // S3-27R F: S3Object + mismatched Location target fails closed, identity/location untouched.
    #[tokio::test]
    async fn s3_27r_mismatched_target_fails_closed_untouched() {
        let mock = PreviewSeamMockProvider {
            read_calls: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        };
        let loc = Location::Sftp {
            host: "other".into(),
            path: "/".into(),
        };
        let listed = listed_s3_object("Prod", "Bucket", "foo/../REAL//x.txt");
        let err = mock
            .read_listed_prefix_bytes(&loc, &listed, 100)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::Unsupported);
        assert!(mock.read_calls.lock().unwrap().is_empty());
        // identity and location untouched — seam never rewrites either value
        assert_eq!(
            listed.identity,
            EntryIdentity::S3Object(s3::S3ObjectRef {
                target: "Prod".into(),
                bucket: "Bucket".into(),
                key: "foo/../REAL//x.txt".into(),
            })
        );
    }

    // S3-27R F (registry): S3 location with unknown target fails closed at provider boundary.
    #[tokio::test]
    async fn s3_27r_registry_s3_location_fails_closed() {
        let registry = ProviderRegistry::new();
        let listed = listed_s3_object("Prod", "Bucket", "k.txt");
        let loc = Location::S3 {
            target: "unknown".into(),
            bucket: Some("Bucket".into()),
            prefix: String::new(),
        };
        let err = registry
            .read_listed_prefix_bytes_at(&loc, &listed, 100)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unknown S3 target"));
    }
}

#[cfg(test)]
mod s3_54r_tests {
    use super::*;
    use crate::config::S3TargetConfig;

    fn bound_registry() -> ProviderRegistry {
        let r = ProviderRegistry::new();
        r.register_s3_targets(&[S3TargetConfig {
            id: "bkt".into(),
            name: "bkt".into(),
            bucket: Some("company-artifacts".into()),
            region: None,
            profile: None,
            endpoint_url: None,
            force_path_style: false,
        }]);
        r
    }

    // 1. target root (bucket None) rejected before any client/provider work.
    #[tokio::test]
    async fn target_root_bucket_none_rejected() {
        let registry = ProviderRegistry::new();
        let loc = Location::S3 {
            target: "x".into(),
            bucket: None,
            prefix: String::new(),
        };
        let err = registry
            .create_s3_prefix_marker_at(&loc, "new")
            .await
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::Unsupported);
        assert!(err.to_string().contains("bucket creation is not supported"));
    }

    // 9/10/11. invalid child name rejected before resolving the provider.
    #[tokio::test]
    async fn invalid_child_dot_rejected() {
        let registry = ProviderRegistry::new();
        let loc = Location::S3 {
            target: "x".into(),
            bucket: Some("b".into()),
            prefix: String::new(),
        };
        let err = registry
            .create_s3_prefix_marker_at(&loc, ".")
            .await
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }
    #[tokio::test]
    async fn invalid_child_dotdot_rejected() {
        let registry = ProviderRegistry::new();
        let loc = Location::S3 {
            target: "x".into(),
            bucket: Some("b".into()),
            prefix: String::new(),
        };
        let err = registry
            .create_s3_prefix_marker_at(&loc, "..")
            .await
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }
    #[tokio::test]
    async fn invalid_child_slash_rejected() {
        let registry = ProviderRegistry::new();
        let loc = Location::S3 {
            target: "x".into(),
            bucket: Some("b".into()),
            prefix: String::new(),
        };
        let err = registry
            .create_s3_prefix_marker_at(&loc, "a/b")
            .await
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    // 12. unknown target => Err (no provider constructed).
    #[tokio::test]
    async fn unknown_target_rejected() {
        let registry = ProviderRegistry::new();
        let loc = Location::S3 {
            target: "nope".into(),
            bucket: Some("b".into()),
            prefix: String::new(),
        };
        let err = registry
            .create_s3_prefix_marker_at(&loc, "new")
            .await
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
        assert!(err.to_string().contains("unknown S3 target"));
    }

    // 13. bucket-bound escape rejected: Location bucket differs from the bound
    // target's bucket. The provider for the target refuses the write.
    #[tokio::test]
    async fn bucket_bound_escape_rejected() {
        let registry = bound_registry();
        let loc = Location::S3 {
            target: "bkt".into(),
            bucket: Some("evil".into()),
            prefix: String::new(),
        };
        let err = registry
            .create_s3_prefix_marker_at(&loc, "new")
            .await
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
        assert!(err.to_string().contains("bucket escape rejected"));
    }

    // non-S3 location rejected (typed seam only).
    #[tokio::test]
    async fn non_s3_location_rejected() {
        let registry = ProviderRegistry::new();
        let loc = Location::Local(std::path::PathBuf::from("/tmp"));
        let err = registry
            .create_s3_prefix_marker_at(&loc, "new")
            .await
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::Unsupported);
    }
}
