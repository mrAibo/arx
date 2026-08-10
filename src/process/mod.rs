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
use crate::vfs::ProviderRegistry;

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
                let bytes = match registry
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
                    &bytes,
                    total_size,
                    bytes.len() >= preview::MAX_TEXT_PREVIEW_BYTES,
                    &name,
                    preview::MAX_TEXT_PREVIEW_LINES,
                )
                .unwrap_or_else(|e| vec![format!("Error: {e}")]);
                EffectEvent::ViewerLines {
                    title: format!("View: {name}"),
                    lines,
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
}
