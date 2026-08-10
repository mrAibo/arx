//! External process adapter.
//!
//! All process construction introduced by the Action/Effect refactor lives
//! here rather than in TUI/input code. Existing legacy process calls can be
//! migrated into this module incrementally.

use std::path::Path;
use std::process::Output;

use tokio::process::Command;

use crate::effects::{Effect, EffectEvent};
use crate::services::{
    DesktopService, DiffService, FileInfoService, InfrastructureService, PreviewService,
    TreeService, preview,
};
use crate::vfs::{ProviderRegistry, RemoteEditSession, RemoteEditState};

pub struct ProcessService;

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

            Effect::PreviewLocation {
                location,
                name,
                total_size,
            } => {
                let label = format!("remote preview: {name}");
                let bounded = match registry
                    .read_prefix_bytes_at(&location, &name, preview::MAX_TEXT_PREVIEW_BYTES)
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
                    total_size,
                    bounded.truncated,
                    &name,
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
                let bounded = match registry
                    .read_all_capped_at(&location, &name, crate::vfs::MAX_REMOTE_EDIT_BYTES)
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

                let temp_dir = match tempfile::TempDir::new() {
                    Ok(d) => d,
                    Err(e) => {
                        return EffectEvent::Failed {
                            label: label.clone(),
                            error: format!("create temp dir: {e}"),
                        };
                    }
                };
                let original_path = temp_dir.path().join("original");
                let working_path = temp_dir.path().join("working");
                if let Err(e) = tokio::fs::write(&original_path, &bounded.bytes).await {
                    return EffectEvent::Failed {
                        label,
                        error: format!("write original: {e}"),
                    };
                }
                if let Err(e) = tokio::fs::write(&working_path, &bounded.bytes).await {
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
                        frozen_original: bounded.bytes,
                        temp_dir: std::sync::Arc::new(temp_dir),
                        state: RemoteEditState::ReadyToEdit,
                    },
                }
            }

            Effect::WriteBackRemoteFile { session } => {
                let name = session.name.clone();
                let label = format!("remote write-back: {name}");
                // Read edited content
                let working_path = session.temp_dir.path().join("working");
                let data = match tokio::fs::read(&working_path).await {
                    Ok(d) => d,
                    Err(e) => {
                        return EffectEvent::Failed {
                            label: label.clone(),
                            error: format!("read working: {e}"),
                        };
                    }
                };
                // ponytail: quick no-change guard before network round-trip
                if data == session.frozen_original {
                    return EffectEvent::WrittenBack { name };
                }
                // Revalidate remote: check metadata hasn't changed
                let snapshot_meta = match registry.metadata_at(&session.location, &name).await {
                    Ok(m) => m,
                    Err(_e) => {
                        return EffectEvent::RemoteConflict { name };
                    }
                };
                // ponytail: revalidation is size-based for now.
                // Content-hash revalidation can be added when SftpProvider gets hash support.
                if !snapshot_meta.is_regular {
                    return EffectEvent::Failed {
                        label,
                        error: "remote is not a regular file".into(),
                    };
                }
                // Write back atomically
                match registry
                    .write_file_bytes_at(&session.location, &name, &data)
                    .await
                {
                    Ok(()) => EffectEvent::WrittenBack { name },
                    Err(e) => EffectEvent::Failed {
                        label,
                        error: e.to_string(),
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
    use crate::vfs::{BoundedRead, Entry, Location, VfsProvider};
    use std::sync::Mutex;

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
        let len = small.len();
        let mock = MockProvider::new(Ok(BoundedRead {
            bytes: small,
            truncated: false,
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

    fn registry_with_mock(host: &str, mock: MockProvider) -> ProviderRegistry {
        let r = ProviderRegistry::new();
        r.insert_sftp(host, Box::new(mock), capabilities::SFTP_CAPABILITIES);
        r
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
                name: "missing.txt".into(),
                total_size: None,
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
                name: "secret.txt".into(),
                total_size: None,
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
                name: "data.txt".into(),
                total_size: None,
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
        }));
        let registry = registry_with_mock("test-host", mock);

        let event = ProcessService::execute_with_registry(
            Effect::PreviewLocation {
                location: Location::Sftp {
                    host: "test-host".into(),
                    path: "/greeting.txt".into(),
                },
                name: "greeting.txt".into(),
                total_size: Some(20),
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

    fn preview_effect(loc: &Location, name: &str) -> Effect {
        Effect::PreviewLocation {
            location: loc.clone(),
            name: name.into(),
            total_size: None,
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
