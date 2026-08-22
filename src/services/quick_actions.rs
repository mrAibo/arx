use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::vfs::CancellationFlag;

const HASH_BUFFER_BYTES: usize = 1024 * 1024;
const TAR_STDERR_LIMIT: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickActionKind {
    Sha256,
    Touch,
    CompressTarGz,
}

impl QuickActionKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Sha256 => "SHA-256",
            Self::Touch => "Touch file",
            Self::CompressTarGz => "Compress to tar.gz",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuickActionRequest {
    Sha256 {
        dir: PathBuf,
        names: Vec<String>,
    },
    Touch {
        dir: PathBuf,
        name: String,
    },
    CompressTarGz {
        dir: PathBuf,
        names: Vec<String>,
        output_name: String,
    },
}

impl QuickActionRequest {
    pub const fn kind(&self) -> QuickActionKind {
        match self {
            Self::Sha256 { .. } => QuickActionKind::Sha256,
            Self::Touch { .. } => QuickActionKind::Touch,
            Self::CompressTarGz { .. } => QuickActionKind::CompressTarGz,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChecksumResult {
    pub name: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuickActionOutcome {
    Sha256 {
        dir: PathBuf,
        checksums: Vec<ChecksumResult>,
    },
    Touched {
        path: PathBuf,
    },
    Compressed {
        path: PathBuf,
        entries: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickActionFailureKind {
    InvalidName,
    NotRegularFile,
    AlreadyExists,
    ToolUnavailable,
    ToolFailed,
    Cancelled,
    Io,
    Worker,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickActionFailure {
    pub action: QuickActionKind,
    pub kind: QuickActionFailureKind,
    pub message: String,
}

#[derive(Debug, thiserror::Error)]
enum QuickActionError {
    #[error("invalid child name: {0}")]
    InvalidName(String),
    #[error("not a regular file: {0}")]
    NotRegularFile(String),
    #[error("destination already exists: {0}")]
    AlreadyExists(String),
    #[error("required tool is unavailable: {0}")]
    ToolUnavailable(String),
    #[error("external tool failed: {0}")]
    ToolFailed(String),
    #[error("operation cancelled")]
    Cancelled,
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("worker failed: {0}")]
    Worker(String),
}

impl QuickActionError {
    fn into_failure(self, action: QuickActionKind) -> QuickActionFailure {
        let kind = match &self {
            Self::InvalidName(_) => QuickActionFailureKind::InvalidName,
            Self::NotRegularFile(_) => QuickActionFailureKind::NotRegularFile,
            Self::AlreadyExists(_) => QuickActionFailureKind::AlreadyExists,
            Self::ToolUnavailable(_) => QuickActionFailureKind::ToolUnavailable,
            Self::ToolFailed(_) => QuickActionFailureKind::ToolFailed,
            Self::Cancelled => QuickActionFailureKind::Cancelled,
            Self::Io(_) => QuickActionFailureKind::Io,
            Self::Worker(_) => QuickActionFailureKind::Worker,
        };
        QuickActionFailure {
            action,
            kind,
            message: self.to_string(),
        }
    }
}

pub struct QuickActionService;

impl QuickActionService {
    pub async fn execute(
        request: QuickActionRequest,
        cancellation: &CancellationFlag,
    ) -> Result<QuickActionOutcome, QuickActionFailure> {
        let kind = request.kind();
        let result = match request {
            QuickActionRequest::Sha256 { dir, names } => {
                Self::sha256_local(dir, names, cancellation.clone()).await
            }
            QuickActionRequest::Touch { dir, name } => {
                Self::touch_local(dir, name, cancellation.clone()).await
            }
            QuickActionRequest::CompressTarGz {
                dir,
                names,
                output_name,
            } => Self::compress_tar_gz_local(dir, names, output_name, cancellation).await,
        };
        result.map_err(|error| error.into_failure(kind))
    }

    async fn sha256_local(
        dir: PathBuf,
        names: Vec<String>,
        cancellation: CancellationFlag,
    ) -> Result<QuickActionOutcome, QuickActionError> {
        if names.is_empty() {
            return Err(QuickActionError::InvalidName("no files selected".into()));
        }
        tokio::task::spawn_blocking(move || {
            let mut checksums = Vec::with_capacity(names.len());
            for name in names {
                if cancellation.is_cancelled() {
                    return Err(QuickActionError::Cancelled);
                }
                validate_child_name(&name)?;
                let path = dir.join(&name);
                let mut file = open_regular_nofollow(&path, false)?;
                let mut hasher = Sha256::new();
                let mut buffer = vec![0u8; HASH_BUFFER_BYTES];
                loop {
                    if cancellation.is_cancelled() {
                        return Err(QuickActionError::Cancelled);
                    }
                    let read = file.read(&mut buffer)?;
                    if read == 0 {
                        break;
                    }
                    hasher.update(&buffer[..read]);
                }
                let digest = hasher.finalize();
                let mut hex = String::with_capacity(64);
                for byte in digest {
                    use std::fmt::Write as _;
                    write!(&mut hex, "{byte:02x}").expect("writing to String cannot fail");
                }
                checksums.push(ChecksumResult { name, sha256: hex });
            }
            Ok(QuickActionOutcome::Sha256 { dir, checksums })
        })
        .await
        .map_err(|error| QuickActionError::Worker(error.to_string()))?
    }

    async fn touch_local(
        dir: PathBuf,
        name: String,
        cancellation: CancellationFlag,
    ) -> Result<QuickActionOutcome, QuickActionError> {
        tokio::task::spawn_blocking(move || {
            if cancellation.is_cancelled() {
                return Err(QuickActionError::Cancelled);
            }
            validate_child_name(&name)?;
            let path = dir.join(&name);
            let file = open_regular_nofollow(&path, true)?;
            if cancellation.is_cancelled() {
                return Err(QuickActionError::Cancelled);
            }
            // SAFETY: `file` owns a valid open fd for a verified regular file.
            // A null times pointer asks futimens(2) to set atime/mtime to now.
            let rc = unsafe { libc::futimens(file.as_raw_fd(), std::ptr::null()) };
            if rc != 0 {
                return Err(QuickActionError::Io(io::Error::last_os_error()));
            }
            file.sync_all()?;
            Ok(QuickActionOutcome::Touched { path })
        })
        .await
        .map_err(|error| QuickActionError::Worker(error.to_string()))?
    }

    async fn compress_tar_gz_local(
        dir: PathBuf,
        names: Vec<String>,
        output_name: String,
        cancellation: &CancellationFlag,
    ) -> Result<QuickActionOutcome, QuickActionError> {
        if cancellation.is_cancelled() {
            return Err(QuickActionError::Cancelled);
        }
        if names.is_empty() {
            return Err(QuickActionError::InvalidName("no entries selected".into()));
        }
        for name in &names {
            validate_child_name(name)?;
            // Validate existence without following symlinks. tar archives symlinks
            // as links by default; ARX must not dereference them during validation.
            std::fs::symlink_metadata(dir.join(name))?;
        }

        let output_name = normalized_tar_gz_name(&output_name)?;
        let final_path = dir.join(&output_name);
        let staged = tempfile::NamedTempFile::new_in(&dir)?;
        let staged_path = staged.path().to_path_buf();

        let output = tokio::process::Command::new("tar")
            .current_dir(&dir)
            .arg("-czf")
            .arg(&staged_path)
            .arg("--")
            .args(&names)
            .output()
            .await
            .map_err(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    QuickActionError::ToolUnavailable("tar".into())
                } else {
                    QuickActionError::Io(error)
                }
            })?;

        if !output.status.success() {
            let stderr = bounded_stderr(&output.stderr);
            return Err(QuickActionError::ToolFailed(if stderr.is_empty() {
                format!("tar exited with {}", output.status)
            } else {
                format!("tar exited with {}: {stderr}", output.status)
            }));
        }
        if cancellation.is_cancelled() {
            return Err(QuickActionError::Cancelled);
        }

        staged.as_file().sync_all()?;
        let persisted = staged.persist_noclobber(&final_path).map_err(|error| {
            if error.error.kind() == io::ErrorKind::AlreadyExists {
                QuickActionError::AlreadyExists(output_name.clone())
            } else {
                QuickActionError::Io(error.error)
            }
        })?;
        persisted.sync_all()?;

        Ok(QuickActionOutcome::Compressed {
            path: final_path,
            entries: names.len(),
        })
    }
}

fn validate_child_name(name: &str) -> Result<(), QuickActionError> {
    if name.is_empty() || name.as_bytes().contains(&0) {
        return Err(QuickActionError::InvalidName(name.to_string()));
    }
    let path = Path::new(name);
    let mut components = path.components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) => Ok(()),
        _ => Err(QuickActionError::InvalidName(name.to_string())),
    }
}

fn normalized_tar_gz_name(input: &str) -> Result<String, QuickActionError> {
    if input.trim().is_empty() {
        return Err(QuickActionError::InvalidName(input.to_string()));
    }
    let name = if input.ends_with(".tar.gz") {
        input.to_string()
    } else {
        format!("{input}.tar.gz")
    };
    validate_child_name(&name)?;
    Ok(name)
}

fn open_regular_nofollow(path: &Path, create: bool) -> Result<File, QuickActionError> {
    let mut options = OpenOptions::new();
    options
        .read(!create)
        .write(create)
        .create(create)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(QuickActionError::NotRegularFile(
            path.to_string_lossy().into_owned(),
        ));
    }
    Ok(file)
}

fn bounded_stderr(bytes: &[u8]) -> String {
    let bounded = &bytes[..bytes.len().min(TAR_STDERR_LIMIT)];
    String::from_utf8_lossy(bounded).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[tokio::test]
    async fn sha256_matches_known_vector_and_handles_metacharacters() {
        let dir = tempfile::tempdir().unwrap();
        let name = "a b;$'x'.txt";
        std::fs::write(dir.path().join(name), b"abc").unwrap();

        let result = QuickActionService::execute(
            QuickActionRequest::Sha256 {
                dir: dir.path().to_path_buf(),
                names: vec![name.into()],
            },
            &CancellationFlag::default(),
        )
        .await
        .unwrap();

        let QuickActionOutcome::Sha256 { checksums, .. } = result else {
            panic!("expected SHA-256 outcome");
        };
        assert_eq!(checksums.len(), 1);
        assert_eq!(checksums[0].name, name);
        assert_eq!(
            checksums[0].sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[tokio::test]
    async fn sha256_refuses_directory_and_symlink() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("subdir")).unwrap();
        std::fs::write(dir.path().join("target"), b"x").unwrap();
        symlink("target", dir.path().join("link")).unwrap();

        for name in ["subdir", "link"] {
            let result = QuickActionService::execute(
                QuickActionRequest::Sha256 {
                    dir: dir.path().to_path_buf(),
                    names: vec![name.into()],
                },
                &CancellationFlag::default(),
            )
            .await;
            assert!(result.is_err(), "{name} must fail closed");
        }
    }

    #[tokio::test]
    async fn touch_creates_and_updates_regular_file_without_following_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let cancellation = CancellationFlag::default();

        QuickActionService::execute(
            QuickActionRequest::Touch {
                dir: dir.path().to_path_buf(),
                name: "new file.txt".into(),
            },
            &cancellation,
        )
        .await
        .unwrap();
        assert!(dir.path().join("new file.txt").is_file());

        std::fs::write(dir.path().join("target"), b"unchanged").unwrap();
        symlink("target", dir.path().join("link")).unwrap();
        let result = QuickActionService::execute(
            QuickActionRequest::Touch {
                dir: dir.path().to_path_buf(),
                name: "link".into(),
            },
            &cancellation,
        )
        .await;
        assert!(result.is_err());
        assert_eq!(std::fs::read(dir.path().join("target")).unwrap(), b"unchanged");
    }

    #[tokio::test]
    async fn touch_and_archive_names_reject_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let cancellation = CancellationFlag::default();
        let touch = QuickActionService::execute(
            QuickActionRequest::Touch {
                dir: dir.path().to_path_buf(),
                name: "../escape".into(),
            },
            &cancellation,
        )
        .await;
        assert!(matches!(
            touch.unwrap_err().kind,
            QuickActionFailureKind::InvalidName
        ));
        assert!(normalized_tar_gz_name("../escape").is_err());
    }

    #[tokio::test]
    async fn compress_tar_gz_handles_dash_space_unicode_and_noclobber() {
        let dir = tempfile::tempdir().unwrap();
        let names = vec!["-dash.txt".to_string(), "space ü.txt".to_string()];
        for name in &names {
            std::fs::write(dir.path().join(name), name.as_bytes()).unwrap();
        }

        let first = QuickActionService::execute(
            QuickActionRequest::CompressTarGz {
                dir: dir.path().to_path_buf(),
                names: names.clone(),
                output_name: "bundle".into(),
            },
            &CancellationFlag::default(),
        )
        .await;
        if std::process::Command::new("tar")
            .arg("--version")
            .output()
            .is_err()
        {
            assert!(matches!(
                first.unwrap_err().kind,
                QuickActionFailureKind::ToolUnavailable
            ));
            return;
        }
        let QuickActionOutcome::Compressed { path, entries } = first.unwrap() else {
            panic!("expected compressed outcome");
        };
        assert_eq!(path.file_name().unwrap(), "bundle.tar.gz");
        assert_eq!(entries, 2);

        let listing = std::process::Command::new("tar")
            .args(["-tzf"])
            .arg(&path)
            .output()
            .unwrap();
        assert!(listing.status.success());
        let listing = String::from_utf8_lossy(&listing.stdout);
        for name in &names {
            assert!(listing.lines().any(|line| line == name));
        }

        let second = QuickActionService::execute(
            QuickActionRequest::CompressTarGz {
                dir: dir.path().to_path_buf(),
                names,
                output_name: "bundle.tar.gz".into(),
            },
            &CancellationFlag::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(second.kind, QuickActionFailureKind::AlreadyExists);
    }
}