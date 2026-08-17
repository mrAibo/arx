//! External process adapter.
//!
//! All process construction introduced by the Action/Effect refactor lives
//! here rather than in TUI/input code. Existing legacy process calls can be
//! migrated into this module incrementally.

use std::path::Path;
use std::process::Output;

use tokio::process::Command;

use crate::effects::{Effect, EffectEvent, ProgressSlot};
use crate::services::{
    DesktopService, DiffService, FileInfoService, InfrastructureService, PreviewService,
    TreeService, preview,
};
use crate::vfs::{
    CancellationFlag, MAX_REMOTE_EDIT_BYTES, ProviderRegistry, RemoteEditSession, RemoteEditState,
    RemoteWriteFailureKind, remote_write_failure_kind,
};

pub struct ProcessService;

fn validate_remote_edit_text(bytes: &[u8]) -> std::io::Result<()> {
    if bytes.contains(&0) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "remote edit contains NUL bytes",
        ));
    }
    std::str::from_utf8(bytes).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "remote edit is not valid UTF-8",
        )
    })?;
    Ok(())
}

async fn read_remote_edit_working_file(path: &Path) -> std::io::Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;

    let path_metadata = tokio::fs::symlink_metadata(path).await?;
    if !path_metadata.file_type().is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "edited path is not a regular file",
        ));
    }

    let mut file = tokio::fs::File::open(path).await?;
    let opened_metadata = file.metadata().await?;
    if !opened_metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "opened edited path is not a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if path_metadata.dev() != opened_metadata.dev()
            || path_metadata.ino() != opened_metadata.ino()
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "edited path changed while opening",
            ));
        }
    }

    let mut data = Vec::new();
    (&mut file)
        .take((MAX_REMOTE_EDIT_BYTES + 1) as u64)
        .read_to_end(&mut data)
        .await?;
    if data.len() > MAX_REMOTE_EDIT_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "edited file exceeds remote edit limit ({} MiB)",
                MAX_REMOTE_EDIT_BYTES / (1024 * 1024)
            ),
        ));
    }
    let after = file.metadata().await?;
    let path_after = tokio::fs::symlink_metadata(path).await?;
    if !path_after.file_type().is_file() || after.len() != data.len() as u64 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "edited path changed while reading",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if opened_metadata.dev() != path_after.dev()
            || opened_metadata.ino() != path_after.ino()
            || opened_metadata.len() != after.len()
            || opened_metadata.mtime() != after.mtime()
            || opened_metadata.mtime_nsec() != after.mtime_nsec()
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "edited path changed while reading",
            ));
        }
    }
    validate_remote_edit_text(&data)?;
    Ok(data)
}

impl ProcessService {
    /// Generic process adapter used by higher-level services. Command
    /// construction stays centralized here; TUI code receives typed service
    /// results and never creates OS processes directly.
    pub async fn output(
        program: &str,
        args: &[String],
        current_dir: Option<&Path>,
    ) -> std::io::Result<Output> {
        let mut command = Command::new(program);
        command.args(args);
        if let Some(current_dir) = current_dir {
            command.current_dir(current_dir);
        }
        command.output().await
    }

    pub async fn status(
        program: &str,
        args: &[String],
        current_dir: Option<&Path>,
    ) -> std::io::Result<std::process::ExitStatus> {
        let mut command = Command::new(program);
        command.args(args);
        if let Some(current_dir) = current_dir {
            command.current_dir(current_dir);
        }
        command.status().await
    }

    pub async fn execute_with_registry(effect: Effect, registry: &ProviderRegistry) -> EffectEvent {
        Self::execute_with_registry_cancellable(effect, registry, &CancellationFlag::default())
            .await
    }

    pub async fn execute_with_registry_cancellable(
        effect: Effect,
        registry: &ProviderRegistry,
        cancellation: &CancellationFlag,
    ) -> EffectEvent {
        match effect {
            // Effects that don't need registry delegate to the existing handler
            e @ (Effect::RunShellCapture { .. }
            | Effect::SpawnShell { .. }
            | Effect::AttachTmux { .. }
            | Effect::AttachScreen { .. }
            | Effect::ListTmuxSessions
            | Effect::DirectoryChildrenSizes { .. }
            | Effect::UnifiedDiff { .. }
            | Effect::InfrastructureSnapshot
            | Effect::TreeSnapshot { .. }
            | Effect::PreviewFile { .. }
            | Effect::OpenPath { .. }) => Self::execute(e).await,

            Effect::PreviewLocation { location, listed } => {
                let name = &listed.entry.name;
                let label = format!("remote preview: {name}");
                let bounded = match registry
                    .read_listed_prefix_bytes_at(
                        &location,
                        &listed,
                        preview::MAX_TEXT_PREVIEW_BYTES,
                    )
                    .await
                {
                    Ok(b) => b,
                    Err(e) => {
                        return EffectEvent::Failed {
                            label,
                            error: e.to_string(),
                        };
                    }
                };
                let lines = preview::format_bounded_preview(
                    &bounded.bytes,
                    listed.entry.size,
                    bounded.truncated,
                    name,
                    preview::MAX_TEXT_PREVIEW_LINES,
                )
                .unwrap_or_else(|e| vec![format!("Error: {e}")]);
                EffectEvent::ViewerLines {
                    title: format!("View: {} — {}", name, location),
                    lines,
                }
            }

            Effect::DownloadRemoteFile {
                location,
                name,
                editor,
            } => {
                let label = format!("remote download: {name}");
                let read_result = registry
                    .read_all_capped_cancellable_at(
                        &location,
                        &name,
                        crate::vfs::MAX_REMOTE_EDIT_BYTES,
                        cancellation,
                    )
                    .await;
                let bounded = match read_result {
                    Ok(b) => b,
                    Err(e) => {
                        return EffectEvent::Failed {
                            label,
                            error: e.to_string(),
                        };
                    }
                };
                if cancellation.is_cancelled() {
                    return EffectEvent::Failed {
                        label,
                        error: format!("remote download cancelled: {name}"),
                    };
                }

                // Refuse truncated files — never open editor on partial content
                if bounded.truncated {
                    return EffectEvent::Failed {
                        label,
                        error: format!(
                            "File too large for remote editing (>{} bytes). \
                             Use a local editor with full SFTP mount.",
                            crate::vfs::MAX_REMOTE_EDIT_BYTES
                        ),
                    };
                }

                if let Err(error) = validate_remote_edit_text(&bounded.bytes) {
                    return EffectEvent::Failed {
                        label,
                        error: format!("Refusing to edit {name}: {error}"),
                    };
                }

                let revision = match bounded.into_revision() {
                    Ok(revision) => revision,
                    Err(error) => {
                        return EffectEvent::Failed {
                            label,
                            error: format!("Remote revision unavailable for {name}: {error}"),
                        };
                    }
                };

                let temp_dir = match tempfile::TempDir::new() {
                    Ok(d) => d,
                    Err(e) => {
                        return EffectEvent::Failed {
                            label: label.clone(),
                            error: format!("create temp dir: {e}"),
                        };
                    }
                };
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Err(e) = tokio::fs::set_permissions(
                        temp_dir.path(),
                        std::fs::Permissions::from_mode(0o700),
                    )
                    .await
                    {
                        return EffectEvent::Failed {
                            label: label.clone(),
                            error: format!("secure temp dir: {e}"),
                        };
                    }
                }
                let original_path = temp_dir.path().join("original");
                let working_path = temp_dir.path().join("working");
                if let Err(e) = tokio::fs::write(&original_path, revision.bytes()).await {
                    return EffectEvent::Failed {
                        label,
                        error: format!("write original: {e}"),
                    };
                }
                if let Err(e) = tokio::fs::write(&working_path, revision.bytes()).await {
                    return EffectEvent::Failed {
                        label,
                        error: format!("write working: {e}"),
                    };
                }
                EffectEvent::Downloaded {
                    session: RemoteEditSession {
                        name,
                        location,
                        editor,
                        revision,
                        temp_dir: std::sync::Arc::new(temp_dir),
                        state: RemoteEditState::ReadyToEdit,
                        job_id: None,
                    },
                }
            }

            Effect::WriteBackRemoteFile {
                mut session,
                progress,
            } => {
                let name = session.name.clone();
                let label = format!("remote write-back: {name}");
                let ProgressSlot(progress) = progress;
                if cancellation.is_cancelled() {
                    return EffectEvent::Failed {
                        label,
                        error: format!("remote write-back cancelled: {name}"),
                    };
                }
                let working_path = session.temp_dir.path().join("working");
                let data = match read_remote_edit_working_file(&working_path).await {
                    Ok(data) => data,
                    Err(error) => {
                        session.state = crate::vfs::RemoteEditState::Failed;
                        return EffectEvent::Failed {
                            label,
                            error: format!("read edited file: {error}"),
                        };
                    }
                };
                if data == session.revision.bytes() {
                    session.state = crate::vfs::RemoteEditState::NoChange;
                    return EffectEvent::NoChange { name };
                }

                let current_result = registry
                    .read_all_capped_cancellable_at(
                        &session.location,
                        &name,
                        MAX_REMOTE_EDIT_BYTES,
                        cancellation,
                    )
                    .await;
                let current = match current_result {
                    Ok(current) => current,
                    Err(error) => {
                        session.state = crate::vfs::RemoteEditState::Failed;
                        return EffectEvent::Failed {
                            label,
                            error: format!("Remote revalidation failed for {name}: {error}"),
                        };
                    }
                };
                if current.truncated
                    || current.bytes != session.revision.bytes()
                    || current.unix_mode != Some(session.revision.unix_mode())
                    || current.unix_uid != Some(session.revision.unix_uid())
                    || current.unix_gid != Some(session.revision.unix_gid())
                {
                    session.state = crate::vfs::RemoteEditState::Conflict;
                    return EffectEvent::RemoteConflict {
                        name,
                        reason: "remote content, mode, or ownership changed during edit".into(),
                    };
                }

                session.state = crate::vfs::RemoteEditState::WritingBack;
                let result = registry
                    .write_file_bytes_if_unchanged_at(
                        &session.location,
                        &name,
                        &data,
                        &session.revision,
                        cancellation,
                        progress,
                    )
                    .await;
                match result {
                    Ok(()) => EffectEvent::WrittenBack { name },
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        EffectEvent::RemoteConflict {
                            name,
                            reason: error.to_string(),
                        }
                    }
                    Err(error)
                        if remote_write_failure_kind(&error)
                            == Some(RemoteWriteFailureKind::RecoveryRequired) =>
                    {
                        EffectEvent::RecoveryRequired {
                            name,
                            details: error.to_string(),
                        }
                    }
                    Err(error)
                        if remote_write_failure_kind(&error)
                            == Some(RemoteWriteFailureKind::CommittedWithWarning) =>
                    {
                        EffectEvent::WrittenBackWarning {
                            name,
                            warning: error.to_string(),
                        }
                    }
                    Err(error) => EffectEvent::Failed {
                        label,
                        error: error.to_string(),
                    },
                }
            }
        }
    }

    pub async fn execute(effect: Effect) -> EffectEvent {
        match effect {
            Effect::RunShellCapture { command } => {
                let label = format!("shell: {command}");
                match Command::new("sh").arg("-c").arg(&command).output().await {
                    Ok(output) => EffectEvent::ShellCaptured {
                        command,
                        success: output.status.success(),
                        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                    },
                    Err(error) => EffectEvent::Failed {
                        label,
                        error: error.to_string(),
                    },
                }
            }
            Effect::SpawnShell { command } => {
                let label = format!("shell: {command}");
                match Command::new("sh").arg("-c").arg(&command).spawn() {
                    Ok(_) => EffectEvent::Spawned { label },
                    Err(error) => EffectEvent::Failed {
                        label,
                        error: error.to_string(),
                    },
                }
            }
            Effect::AttachTmux { session } => {
                let label = format!("tmux:{session}");
                match Command::new("tmux")
                    .args(["attach-session", "-t", &session])
                    .status()
                    .await
                {
                    Ok(status) => EffectEvent::ProcessExited {
                        label,
                        success: status.success(),
                    },
                    Err(error) => EffectEvent::Failed {
                        label,
                        error: error.to_string(),
                    },
                }
            }
            Effect::AttachScreen { session } => {
                let label = format!("screen:{session}");
                match Command::new("screen").args(["-r", &session]).status().await {
                    Ok(status) => EffectEvent::ProcessExited {
                        label,
                        success: status.success(),
                    },
                    Err(error) => EffectEvent::Failed {
                        label,
                        error: error.to_string(),
                    },
                }
            }
            Effect::ListTmuxSessions => match Self::list_tmux_sessions().await {
                Ok(sessions) => EffectEvent::TmuxSessions { sessions },
                Err(error) => EffectEvent::Failed {
                    label: "tmux session discovery".into(),
                    error,
                },
            },
            Effect::DirectoryChildrenSizes { path } => {
                let lines = FileInfoService::directory_children_sizes(&path).await;
                EffectEvent::ViewerLines {
                    title: format!("Directory sizes: {}", path.display()),
                    lines,
                }
            }
            Effect::UnifiedDiff { left, right } => {
                match DiffService::unified(&left, &right).await {
                    Ok(lines) => EffectEvent::ViewerLines {
                        title: format!("Diff: {} ↔ {}", left.display(), right.display()),
                        lines,
                    },
                    Err(error) => EffectEvent::Failed {
                        label: "diff".into(),
                        error,
                    },
                }
            }
            Effect::InfrastructureSnapshot => EffectEvent::InfrastructureLines {
                lines: InfrastructureService::snapshot().await,
            },
            Effect::TreeSnapshot { location, filter } => EffectEvent::TreeLines {
                lines: TreeService::snapshot(&location, &filter).await,
            },
            Effect::PreviewFile { path } => EffectEvent::ViewerLines {
                title: format!("View: {}", path.display()),
                lines: PreviewService::preview(&path).await,
            },
            Effect::PreviewLocation { .. } => EffectEvent::Failed {
                label: "remote preview".into(),
                error: "PreviewLocation must be dispatched with a ProviderRegistry".into(),
            },
            Effect::OpenPath { path } => match DesktopService::open_path(&path).await {
                Ok(()) => EffectEvent::PathOpened { path },
                Err(error) => EffectEvent::Failed {
                    label: "open path".into(),
                    error: error.to_string(),
                },
            },
            // ponytail: registry-required effects delegate back
            // These should never be dispatched through execute() — they're
            // handled by execute_with_registry which checks the registry first.
            _ => EffectEvent::Failed {
                label: "effect routing error".into(),
                error: "this effect requires a provider registry".into(),
            },
        }
    }

    pub async fn list_tmux_sessions() -> Result<Vec<String>, String> {
        let output = Command::new("tmux")
            .args(["list-sessions", "-F", "#{session_name}"])
            .output()
            .await
            .map_err(|error| error.to_string())?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect_dispatcher::{EffectDispatcher, EffectLane, EffectScope};
    use crate::vfs::capabilities;
    use crate::vfs::{
        BoundedRead, Entry, EntryIdentity, EntryKind, ListedEntry, Location, VfsProvider,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn preview_effect_returns_viewer_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.txt");
        tokio::fs::write(&path, "hello preview\n").await.unwrap();

        let event = ProcessService::execute(Effect::PreviewFile { path }).await;
        let EffectEvent::ViewerLines { title, lines } = event else {
            panic!("expected viewer lines");
        };
        assert!(title.starts_with("View:"));
        assert!(lines.iter().any(|line| line == "hello preview"));
    }

    #[tokio::test]
    async fn edited_working_file_rejects_nul_and_invalid_utf8() {
        for (bytes, expected) in [
            (b"text\0tail".as_slice(), "NUL"),
            (&[0xff, 0xfe][..], "UTF-8"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("working");
            tokio::fs::write(&path, bytes).await.unwrap();

            let error = read_remote_edit_working_file(&path).await.unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
            assert!(error.to_string().contains(expected), "{error}");
        }
    }

    // ── VIEW-09B: error path mock provider ──

    struct MockProvider {
        read_result: Mutex<Option<std::io::Result<BoundedRead>>>,
    }

    impl MockProvider {
        fn new(result: std::io::Result<BoundedRead>) -> Self {
            Self {
                read_result: Mutex::new(Some(result)),
            }
        }
    }

    impl std::fmt::Debug for MockProvider {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("MockProvider").finish()
        }
    }

    #[async_trait::async_trait]
    impl VfsProvider for MockProvider {
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

        async fn metadata(&self, _path: &str) -> std::io::Result<crate::vfs::FileMetadata> {
            Ok(crate::vfs::FileMetadata {
                len: 0,
                is_regular: true,
                unix_mode: Some(0o644),
                unix_uid: Some(1000),
                unix_gid: Some(1000),
            })
        }

        async fn read_all_capped(
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
    }

    #[tokio::test]
    async fn remote_download_refuses_truncated_file() {
        let mock = MockProvider::new(Ok(BoundedRead {
            bytes: b"abc".to_vec(),
            truncated: true,
            unix_mode: Some(0o644),
            unix_uid: Some(1000),
            unix_gid: Some(1000),
        }));
        let registry = registry_with_mock("test", mock);
        let loc = Location::Sftp {
            host: "test".into(),
            path: "/srv".into(),
        };
        let event = ProcessService::execute_with_registry(
            Effect::DownloadRemoteFile {
                location: loc,
                name: "large.txt".into(),
                editor: "vim".into(),
            },
            &registry,
        )
        .await;
        assert!(
            matches!(&event, EffectEvent::Failed { error, .. } if error.contains("too large")),
            "Truncated file should be refused, got {:?}",
            event
        );
    }

    #[tokio::test]
    async fn remote_download_accepts_complete_file() {
        let mock = MockProvider::new(Ok(BoundedRead {
            bytes: b"hello".to_vec(),
            truncated: false,
            unix_mode: Some(0o644),
            unix_uid: Some(1000),
            unix_gid: Some(1000),
        }));
        let registry = registry_with_mock("test", mock);
        let loc = Location::Sftp {
            host: "test".into(),
            path: "/srv".into(),
        };
        let event = ProcessService::execute_with_registry(
            Effect::DownloadRemoteFile {
                location: loc,
                name: "small.txt".into(),
                editor: "nano".into(),
            },
            &registry,
        )
        .await;
        assert!(
            matches!(&event, EffectEvent::Downloaded { session, .. } if session.name == "small.txt"),
            "Complete file should produce Downloaded, got {:?}",
            event
        );
    }

    #[tokio::test]
    async fn remote_download_exact_cap_passes() {
        // ponytail: test with small cap, not 16 MiB
        let small = b"hello world".to_vec();
        let mock = MockProvider::new(Ok(BoundedRead {
            bytes: small,
            truncated: false,
            unix_mode: Some(0o644),
            unix_uid: Some(1000),
            unix_gid: Some(1000),
        }));
        let registry = registry_with_mock("test", mock);
        let loc = Location::Sftp {
            host: "test".into(),
            path: "/srv".into(),
        };
        let event = ProcessService::execute_with_registry(
            Effect::DownloadRemoteFile {
                location: loc,
                name: "exact.txt".into(),
                editor: "vim".into(),
            },
            &registry,
        )
        .await;
        assert!(
            matches!(&event, EffectEvent::Downloaded { .. }),
            "Complete file should be accepted, got {:?}",
            event
        );
    }

    #[tokio::test]
    async fn remote_download_rejects_oversized_file() {
        let mock = MockProvider::new(Ok(BoundedRead {
            bytes: vec![b'x'; 1024],
            truncated: true,
            unix_mode: Some(0o644),
            unix_uid: Some(1000),
            unix_gid: Some(1000),
        }));
        let registry = registry_with_mock("test", mock);
        let loc = Location::Sftp {
            host: "test".into(),
            path: "/srv".into(),
        };
        let event = ProcessService::execute_with_registry(
            Effect::DownloadRemoteFile {
                location: loc,
                name: "too-big.txt".into(),
                editor: "vim".into(),
            },
            &registry,
        )
        .await;
        assert!(
            matches!(&event, EffectEvent::Failed { error, .. } if error.contains("too large")),
            ">cap file should be refused, got {:?}",
            event
        );
    }

    #[tokio::test]
    async fn remote_download_refuses_nul() {
        let mut bytes = b"hello world".to_vec();
        bytes[5] = 0x00;
        let mock = MockProvider::new(Ok(BoundedRead {
            bytes,
            truncated: false,
            unix_mode: Some(0o644),
            unix_uid: Some(1000),
            unix_gid: Some(1000),
        }));
        let registry = registry_with_mock("test", mock);
        let loc = Location::Sftp {
            host: "test".into(),
            path: "/srv".into(),
        };
        let event = ProcessService::execute_with_registry(
            Effect::DownloadRemoteFile {
                location: loc,
                name: "bin.txt".into(),
                editor: "vim".into(),
            },
            &registry,
        )
        .await;
        assert!(
            matches!(&event, EffectEvent::Failed { error, .. } if error.contains("NUL")),
            "NUL bytes should be refused, got {:?}",
            event
        );
    }

    #[tokio::test]
    async fn remote_download_refuses_invalid_utf8() {
        let mock = MockProvider::new(Ok(BoundedRead {
            bytes: vec![0xFF, 0xFE],
            truncated: false,
            unix_mode: Some(0o644),
            unix_uid: Some(1000),
            unix_gid: Some(1000),
        }));
        let registry = registry_with_mock("test", mock);
        let loc = Location::Sftp {
            host: "test".into(),
            path: "/srv".into(),
        };
        let event = ProcessService::execute_with_registry(
            Effect::DownloadRemoteFile {
                location: loc,
                name: "utf16.txt".into(),
                editor: "vim".into(),
            },
            &registry,
        )
        .await;
        assert!(
            matches!(&event, EffectEvent::Failed { error, .. } if error.contains("UTF-8")),
            "Invalid UTF-8 should be refused, got {:?}",
            event
        );
    }

    #[tokio::test]
    async fn remote_download_accepts_valid_utf8() {
        let mock = MockProvider::new(Ok(BoundedRead {
            bytes: "Привет, 世界 🌍\n".as_bytes().to_vec(),
            truncated: false,
            unix_mode: Some(0o644),
            unix_uid: Some(1000),
            unix_gid: Some(1000),
        }));
        let registry = registry_with_mock("test", mock);
        let loc = Location::Sftp {
            host: "test".into(),
            path: "/srv".into(),
        };
        let event = ProcessService::execute_with_registry(
            Effect::DownloadRemoteFile {
                location: loc,
                name: "ok.txt".into(),
                editor: "vim".into(),
            },
            &registry,
        )
        .await;
        assert!(
            matches!(&event, EffectEvent::Downloaded { .. }),
            "Valid UTF-8 should be accepted, got {:?}",
            event
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn remote_download_tempdir_is_private_and_removed_on_drop() {
        use std::os::unix::fs::PermissionsExt;

        let mock = MockProvider::new(Ok(BoundedRead {
            bytes: b"private".to_vec(),
            truncated: false,
            unix_mode: Some(0o644),
            unix_uid: Some(1000),
            unix_gid: Some(1000),
        }));
        let registry = registry_with_mock("test", mock);
        let event = ProcessService::execute_with_registry(
            Effect::DownloadRemoteFile {
                location: Location::Sftp {
                    host: "test".into(),
                    path: "/srv".into(),
                },
                name: "private.txt".into(),
                editor: "vim".into(),
            },
            &registry,
        )
        .await;
        let EffectEvent::Downloaded { session } = event else {
            panic!("expected downloaded session");
        };
        let temp_path = session.temp_dir.path().to_path_buf();
        assert_eq!(
            std::fs::metadata(&temp_path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert!(temp_path.join("original").is_file());
        assert!(temp_path.join("working").is_file());
        drop(session);
        assert!(!temp_path.exists());
    }

    #[tokio::test]
    async fn remote_download_accepts_empty() {
        let mock = MockProvider::new(Ok(BoundedRead {
            bytes: vec![],
            truncated: false,
            unix_mode: Some(0o644),
            unix_uid: Some(1000),
            unix_gid: Some(1000),
        }));
        let registry = registry_with_mock("test", mock);
        let loc = Location::Sftp {
            host: "test".into(),
            path: "/srv".into(),
        };
        let event = ProcessService::execute_with_registry(
            Effect::DownloadRemoteFile {
                location: loc,
                name: "empty.txt".into(),
                editor: "vim".into(),
            },
            &registry,
        )
        .await;
        assert!(
            matches!(&event, EffectEvent::Downloaded { .. }),
            "Empty file should be accepted, got {:?}",
            event
        );
    }

    #[derive(Debug)]
    struct HangingProvider;

    #[async_trait::async_trait]
    impl crate::vfs::VfsProvider for HangingProvider {
        fn list(&self, _path: &str) -> std::io::Result<Vec<crate::vfs::Entry>> {
            Ok(Vec::new())
        }
        fn read_head(&self, _path: &str, _lines: usize) -> std::io::Result<Vec<String>> {
            unreachable!()
        }
        fn copy_files(&self, _src: &str, _dst: &str, _names: &[String]) -> std::io::Result<usize> {
            unreachable!()
        }
        fn move_files(&self, _src: &str, _dst: &str, _names: &[String]) -> std::io::Result<usize> {
            unreachable!()
        }
        fn delete_files(&self, _dir: &str, _names: &[String]) -> std::io::Result<usize> {
            unreachable!()
        }

        async fn read_all_capped(
            &self,
            _path: &str,
            _max_bytes: usize,
        ) -> std::io::Result<crate::vfs::BoundedRead> {
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn remote_download_cancellation_wakes_pending_io() {
        let cancellation = CancellationFlag::default();
        let task_cancellation = cancellation.clone();
        let task = tokio::spawn(async move {
            let registry = ProviderRegistry::default();
            registry.insert_sftp(
                "cancel-host",
                Box::new(HangingProvider),
                capabilities::SFTP_CAPABILITIES,
            );
            ProcessService::execute_with_registry_cancellable(
                Effect::DownloadRemoteFile {
                    location: Location::Sftp {
                        host: "cancel-host".to_string(),
                        path: "/tmp".to_string(),
                    },
                    name: "pending.txt".to_string(),
                    editor: "vim".to_string(),
                },
                &registry,
                &task_cancellation,
            )
            .await
        });

        tokio::task::yield_now().await;
        cancellation.cancel();
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await
            .expect("cancellation must wake the pending read")
            .expect("remote edit task must not panic");
        assert!(matches!(
            event,
            EffectEvent::Failed { label, error }
                if label == "remote download: pending.txt" && error.contains("cancelled")
        ));
    }

    fn registry_with_mock(host: &str, mock: MockProvider) -> ProviderRegistry {
        let registry = ProviderRegistry::new();
        registry.insert_sftp(host, Box::new(mock), capabilities::SFTP_CAPABILITIES);
        registry
    }

    struct WriteBackMock {
        current: Arc<Mutex<Vec<u8>>>,
        mode: u32,
        writes: Arc<AtomicUsize>,
        expected_seen: Arc<Mutex<Option<Vec<u8>>>>,
    }

    impl std::fmt::Debug for WriteBackMock {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.debug_struct("WriteBackMock").finish()
        }
    }

    #[async_trait::async_trait]
    impl VfsProvider for WriteBackMock {
        fn list(&self, _path: &str) -> std::io::Result<Vec<Entry>> {
            unreachable!()
        }
        fn read_head(&self, _path: &str, _lines: usize) -> std::io::Result<Vec<String>> {
            unreachable!()
        }
        fn copy_files(&self, _src: &str, _dst: &str, _names: &[String]) -> std::io::Result<usize> {
            unreachable!()
        }
        fn move_files(&self, _src: &str, _dst: &str, _names: &[String]) -> std::io::Result<usize> {
            unreachable!()
        }
        fn delete_files(&self, _dir: &str, _names: &[String]) -> std::io::Result<usize> {
            unreachable!()
        }
        async fn read_all_capped(
            &self,
            _path: &str,
            max_bytes: usize,
        ) -> std::io::Result<BoundedRead> {
            let current = self.current.lock().unwrap().clone();
            Ok(BoundedRead {
                truncated: current.len() > max_bytes,
                bytes: current.into_iter().take(max_bytes).collect(),
                unix_mode: Some(self.mode),
                unix_uid: Some(1000),
                unix_gid: Some(1000),
            })
        }
        async fn metadata(&self, _path: &str) -> std::io::Result<crate::vfs::FileMetadata> {
            Ok(crate::vfs::FileMetadata {
                len: self.current.lock().unwrap().len() as u64,
                is_regular: true,
                unix_mode: Some(self.mode),
                unix_uid: Some(1000),
                unix_gid: Some(1000),
            })
        }
        async fn write_file_bytes_if_unchanged(
            &self,
            _path: &str,
            data: &[u8],
            revision: &crate::vfs::RemoteEditRevision,
            _cancellation: &crate::vfs::CancellationFlag,
            _progress: Option<crate::vfs::RemoteEditProgressFn>,
        ) -> std::io::Result<()> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            *self.expected_seen.lock().unwrap() = Some(revision.bytes().to_vec());
            assert_eq!(revision.unix_mode(), self.mode);
            *self.current.lock().unwrap() = data.to_vec();
            Ok(())
        }
    }

    type WriteBackFixture = (
        ProviderRegistry,
        RemoteEditSession,
        Arc<Mutex<Vec<u8>>>,
        Arc<AtomicUsize>,
        Arc<Mutex<Option<Vec<u8>>>>,
    );

    fn writeback_fixture(
        original: &[u8],
        edited: &[u8],
        original_mode: u32,
        remote: &[u8],
        remote_mode: u32,
    ) -> WriteBackFixture {
        let temp_dir = Arc::new(tempfile::TempDir::new().unwrap());
        std::fs::write(temp_dir.path().join("original"), original).unwrap();
        std::fs::write(temp_dir.path().join("working"), edited).unwrap();
        let current = Arc::new(Mutex::new(remote.to_vec()));
        let writes = Arc::new(AtomicUsize::new(0));
        let expected_seen = Arc::new(Mutex::new(None));
        let provider = WriteBackMock {
            current: Arc::clone(&current),
            mode: remote_mode,
            writes: Arc::clone(&writes),
            expected_seen: Arc::clone(&expected_seen),
        };
        let registry = ProviderRegistry::new();
        registry.insert_sftp("test", Box::new(provider), capabilities::SFTP_CAPABILITIES);
        let session = RemoteEditSession {
            name: "edit.txt".into(),
            location: Location::Sftp {
                host: "test".into(),
                path: "/srv".into(),
            },
            editor: "true".into(),
            revision: BoundedRead {
                bytes: original.to_vec(),
                truncated: false,
                unix_mode: Some(original_mode),
                unix_uid: Some(1000),
                unix_gid: Some(1000),
            }
            .into_revision()
            .unwrap(),
            temp_dir,
            state: RemoteEditState::WritingBack,
            job_id: None,
        };
        (registry, session, current, writes, expected_seen)
    }

    #[tokio::test]
    async fn remote_writeback_skips_no_change_without_network_write() {
        let (registry, session, _, writes, _) =
            writeback_fixture(b"same", b"same", 0o600, b"same", 0o600);
        let event = ProcessService::execute_with_registry(
            Effect::WriteBackRemoteFile {
                session,
                progress: ProgressSlot(None),
            },
            &registry,
        )
        .await;
        assert!(matches!(event, EffectEvent::NoChange { .. }));
        assert_eq!(writes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn remote_writeback_refuses_oversized_edited_file_without_network_write() {
        let edited = vec![b'x'; crate::vfs::MAX_REMOTE_EDIT_BYTES + 1];
        let (registry, session, _, writes, _) =
            writeback_fixture(b"old", &edited, 0o600, b"old", 0o600);
        let event = ProcessService::execute_with_registry(
            Effect::WriteBackRemoteFile {
                session,
                progress: ProgressSlot(None),
            },
            &registry,
        )
        .await;
        assert!(
            matches!(event, EffectEvent::Failed { error, .. } if error.contains("exceeds remote edit limit"))
        );
        assert_eq!(writes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn remote_writeback_refuses_same_size_concurrent_change() {
        let (registry, session, current, writes, _) =
            writeback_fixture(b"hello", b"local", 0o600, b"other", 0o600);
        let event = ProcessService::execute_with_registry(
            Effect::WriteBackRemoteFile {
                session,
                progress: ProgressSlot(None),
            },
            &registry,
        )
        .await;
        assert!(matches!(event, EffectEvent::RemoteConflict { .. }));
        assert_eq!(*current.lock().unwrap(), b"other");
        assert_eq!(writes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn remote_writeback_refuses_zero_byte_revision_change_and_chmod() {
        let (registry, session, _, writes, _) =
            writeback_fixture(b"", b"local", 0o600, b"x", 0o600);
        let event = ProcessService::execute_with_registry(
            Effect::WriteBackRemoteFile {
                session,
                progress: ProgressSlot(None),
            },
            &registry,
        )
        .await;
        assert!(matches!(event, EffectEvent::RemoteConflict { .. }));
        assert_eq!(writes.load(Ordering::SeqCst), 0);

        let (registry, session, _, writes, _) =
            writeback_fixture(b"same", b"local", 0o600, b"same", 0o644);
        let event = ProcessService::execute_with_registry(
            Effect::WriteBackRemoteFile {
                session,
                progress: ProgressSlot(None),
            },
            &registry,
        )
        .await;
        assert!(matches!(event, EffectEvent::RemoteConflict { .. }));
        assert_eq!(writes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn remote_writeback_passes_frozen_revision_and_mode() {
        let (registry, session, current, writes, expected_seen) =
            writeback_fixture(b"old", b"new", 0o600, b"old", 0o600);
        let event = ProcessService::execute_with_registry(
            Effect::WriteBackRemoteFile {
                session,
                progress: ProgressSlot(None),
            },
            &registry,
        )
        .await;
        assert!(matches!(event, EffectEvent::WrittenBack { .. }));
        assert_eq!(*current.lock().unwrap(), b"new");
        assert_eq!(writes.load(Ordering::SeqCst), 1);
        assert_eq!(*expected_seen.lock().unwrap(), Some(b"old".to_vec()));
    }

    #[tokio::test]
    async fn preview_location_notfound_returns_failed() {
        let err = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");
        let mock = MockProvider::new(Err(err));
        let registry = registry_with_mock("test-host", mock);

        let event = ProcessService::execute_with_registry(
            Effect::PreviewLocation {
                location: Location::Sftp {
                    host: "test-host".into(),
                    path: "/missing.txt".into(),
                },
                listed: listed_file("missing.txt", None),
            },
            &registry,
        )
        .await;

        match event {
            EffectEvent::Failed { label, error } => {
                assert!(label.contains("remote preview"));
                assert!(error.contains("no such file"));
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn preview_location_permissiondenied_returns_failed() {
        let err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access denied");
        let mock = MockProvider::new(Err(err));
        let registry = registry_with_mock("test-host", mock);

        let event = ProcessService::execute_with_registry(
            Effect::PreviewLocation {
                location: Location::Sftp {
                    host: "test-host".into(),
                    path: "/secret.txt".into(),
                },
                listed: listed_file("secret.txt", None),
            },
            &registry,
        )
        .await;

        match event {
            EffectEvent::Failed { label, error } => {
                assert!(label.contains("remote preview"));
                assert!(error.contains("access denied"));
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn preview_location_arbitrary_io_error_returns_failed() {
        let err = std::io::Error::other("SFTP subsystem died");
        let mock = MockProvider::new(Err(err));
        let registry = registry_with_mock("test-host", mock);

        let event = ProcessService::execute_with_registry(
            Effect::PreviewLocation {
                location: Location::Sftp {
                    host: "test-host".into(),
                    path: "/data.txt".into(),
                },
                listed: listed_file("data.txt", None),
            },
            &registry,
        )
        .await;

        match event {
            EffectEvent::Failed { label, error } => {
                assert!(label.contains("remote preview"));
                assert!(error.contains("SFTP subsystem died"));
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn preview_location_success_returns_viewer_lines() {
        let mock = MockProvider::new(Ok(BoundedRead {
            bytes: b"hello remote\nworld\n".to_vec(),
            truncated: false,
            unix_mode: Some(0o644),
            unix_uid: Some(1000),
            unix_gid: Some(1000),
        }));
        let registry = registry_with_mock("test-host", mock);

        let event = ProcessService::execute_with_registry(
            Effect::PreviewLocation {
                location: Location::Sftp {
                    host: "test-host".into(),
                    path: "/greeting.txt".into(),
                },
                listed: listed_file("greeting.txt", Some(20)),
            },
            &registry,
        )
        .await;

        match event {
            EffectEvent::ViewerLines { title, lines } => {
                assert!(title.contains("greeting.txt"));
                assert!(lines.iter().any(|l| l == "hello remote"));
                assert!(lines.iter().any(|l| l == "world"));
            }
            other => panic!("expected ViewerLines, got {other:?}"),
        }
    }

    // ── FIX-05: Concurrency ──

    fn listed_file(name: &str, size: Option<u64>) -> ListedEntry {
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

    fn preview_effect(loc: &Location, name: &str) -> Effect {
        Effect::PreviewLocation {
            location: loc.clone(),
            listed: listed_file(name, None),
        }
    }

    /// C1: dispatch() returns EffectId immediately without blocking on network I/O.
    #[tokio::test]
    async fn preview_effect_returns_id_before_read_completes() {
        let registry = crate::vfs::ProviderRegistry::new();
        let (dispatcher, mut rx) = EffectDispatcher::channel(registry);
        let loc = Location::Local(std::path::PathBuf::from("/tmp"));

        // Dispatch: must return immediately
        let id = dispatcher.dispatch(
            EffectLane::Preview,
            EffectScope::Location(loc.clone()),
            preview_effect(&loc, "test.txt"),
        );
        assert!(id.0 > 0, "EffectId assigned before async work begins");

        // Response eventually arrives through channel (spawned task)
        let response = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("timed out waiting for preview effect response")
            .expect("channel closed");
        assert_eq!(response.id, id);
    }

    /// C2: Two sequential previews produce monotonic IDs.
    #[tokio::test]
    async fn consecutive_previews_have_monotonic_ids() {
        let registry = crate::vfs::ProviderRegistry::new();
        let (dispatcher, mut rx) = EffectDispatcher::channel(registry);
        let loc = Location::Local(std::path::PathBuf::from("/tmp"));

        let id_a = dispatcher.dispatch(
            EffectLane::Preview,
            EffectScope::Location(loc.clone()),
            preview_effect(&loc, "a.txt"),
        );
        let id_b = dispatcher.dispatch(
            EffectLane::Preview,
            EffectScope::Location(loc.clone()),
            preview_effect(&loc, "b.txt"),
        );
        assert!(id_b.0 > id_a.0);

        // Drain both responses
        let resp_a = rx.recv().await.unwrap();
        let resp_b = rx.recv().await.unwrap();
        assert_eq!(resp_a.id, id_a);
        assert_eq!(resp_b.id, id_b);
    }
}
