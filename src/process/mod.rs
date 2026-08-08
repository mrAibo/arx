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
    DesktopService, DiffService, FileInfoService, InfrastructureService, TreeService,
};

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
