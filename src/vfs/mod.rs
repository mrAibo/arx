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
