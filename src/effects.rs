/// Side effects requested by application intent.
///
/// Effects are data. UI/input code may construct them, but only an adapter
/// such as `ProcessService` is allowed to perform the external operation.
use std::path::PathBuf;

use crate::vfs::Location;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    RunShellCapture { command: String },
    SpawnShell { command: String },
    AttachTmux { session: String },
    AttachScreen { session: String },
    ListTmuxSessions,
    DirectoryChildrenSizes { path: PathBuf },
    UnifiedDiff { left: PathBuf, right: PathBuf },
    InfrastructureSnapshot,
    TreeSnapshot { location: Location, filter: String },
    OpenPath { path: PathBuf },
}

/// Typed result sent back across the effect boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectEvent {
    ShellCaptured {
        command: String,
        success: bool,
        stdout: String,
        stderr: String,
    },
    ProcessExited {
        label: String,
        success: bool,
    },
    Spawned {
        label: String,
    },
    TmuxSessions {
        sessions: Vec<String>,
    },
    ViewerLines {
        title: String,
        lines: Vec<String>,
    },
    InfrastructureLines {
        lines: Vec<String>,
    },
    TreeLines {
        lines: Vec<String>,
    },
    PathOpened {
        path: PathBuf,
    },
    Failed {
        label: String,
        error: String,
    },
}
