/// Side effects requested by application intent.
///
/// Effects are data. UI/input code may construct them, but only an adapter
/// such as `ProcessService` is allowed to perform the external operation.
use std::path::PathBuf;

use crate::vfs::{Location, RemoteEditSession};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    RunShellCapture {
        command: String,
    },
    SpawnShell {
        command: String,
    },
    AttachTmux {
        session: String,
    },
    AttachScreen {
        session: String,
    },
    ListTmuxSessions,
    DirectoryChildrenSizes {
        path: PathBuf,
    },
    UnifiedDiff {
        left: PathBuf,
        right: PathBuf,
    },
    InfrastructureSnapshot,
    TreeSnapshot {
        location: Location,
        filter: String,
    },
    PreviewFile {
        path: PathBuf,
    },
    PreviewLocation {
        location: Location,
        name: String,
        total_size: Option<u64>,
    },
    /// Download a remote file to a secure temp directory for editing.
    DownloadRemoteFile {
        location: Location,
        name: String,
        editor: String,
    },
    /// Write edited content back to a remote file (atomic staging).
    WriteBackRemoteFile {
        session: RemoteEditSession,
    },
    OpenPath {
        path: PathBuf,
    },
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
    /// Remote file downloaded to temp path for editing.
    Downloaded {
        session: RemoteEditSession,
    },
    /// Edited content successfully written back to remote.
    WrittenBack {
        name: String,
    },
    /// Remote file changed during edit — write-back refused.
    RemoteConflict {
        name: String,
    },
    Failed {
        label: String,
        error: String,
    },
}
