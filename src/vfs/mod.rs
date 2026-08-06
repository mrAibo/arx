use std::fmt;
use std::path::PathBuf;

pub mod archive;
pub mod local;
pub mod sftp;

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
