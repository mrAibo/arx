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
pub trait VfsProvider: Send + Sync {
    fn list(&self, path: &str) -> std::io::Result<Vec<Entry>>;
    fn read_head(&self, path: &str, lines: usize) -> std::io::Result<Vec<String>>;
    fn copy_files(&self, src: &str, dst: &str, names: &[String]) -> std::io::Result<usize>;
    fn move_files(&self, src: &str, dst: &str, names: &[String]) -> std::io::Result<usize>;
    fn delete_files(&self, dir: &str, names: &[String]) -> std::io::Result<usize>;
}

pub type ProviderRegistry = HashMap<ProviderId, Box<dyn VfsProvider>>;

/// Build default registry with local backend. SFTP/Archive registered per-connection.
pub fn default_registry() -> ProviderRegistry {
    let mut r = ProviderRegistry::new();
    r.insert(ProviderId::Local, Box::new(local::LocalProvider));
    r
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
    fn read_head(&self, path: &std::path::Path, lines: usize) -> anyhow::Result<Vec<String>>;
    fn copy_files(
        &self,
        src_dir: &std::path::Path,
        dst_dir: &std::path::Path,
        names: &[String],
    ) -> std::io::Result<usize>;
    fn move_files(
        &self,
        src_dir: &std::path::Path,
        dst_dir: &std::path::Path,
        names: &[String],
    ) -> std::io::Result<usize>;
    fn delete_files(&self, dir: &std::path::Path, names: &[String]) -> std::io::Result<usize>;
}

impl VfsOps for Location {
    fn list(&self) -> anyhow::Result<Vec<Entry>> {
        let result: std::io::Result<Vec<Entry>> = match self {
            Location::Local(p) => local::LocalFs::list(p),
            Location::Sftp { host, path } => {
                let h = crate::remote::Host::from_alias(host);
                sftp::SftpFs::list(&h, path)
            }
            Location::Archive {
                archive,
                inner_path,
            } => archive::ArchiveFs::list(archive, inner_path),
        };
        result.map_err(|e| anyhow::anyhow!("{e}"))
    }

    fn read_head(&self, path: &std::path::Path, lines: usize) -> anyhow::Result<Vec<String>> {
        match self {
            Location::Local(_) => {
                local::LocalFs::read_head(path, lines).map_err(|e| anyhow::anyhow!("{e}"))
            }
            _ => Err(anyhow::anyhow!("read_head only supported for Local paths")),
        }
    }

    fn copy_files(
        &self,
        src_dir: &std::path::Path,
        dst_dir: &std::path::Path,
        names: &[String],
    ) -> std::io::Result<usize> {
        match self {
            Location::Local(_) => local::LocalFs::copy_files(src_dir, dst_dir, names),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "copy only supported for Local",
            )),
        }
    }

    fn move_files(
        &self,
        src_dir: &std::path::Path,
        dst_dir: &std::path::Path,
        names: &[String],
    ) -> std::io::Result<usize> {
        match self {
            Location::Local(_) => local::LocalFs::move_files(src_dir, dst_dir, names),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "move only supported for Local",
            )),
        }
    }

    fn delete_files(&self, dir: &std::path::Path, names: &[String]) -> std::io::Result<usize> {
        match self {
            Location::Local(_) => local::LocalFs::delete_files(dir, names),
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
