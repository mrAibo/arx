/// Side effects requested by application intent.
///
/// Effects are data. UI/input code may construct them, but only an adapter
/// such as `ProcessService` is allowed to perform the external operation.
use std::path::PathBuf;

use crate::services::{QuickActionFailure, QuickActionOutcome, QuickActionRequest};
use crate::vfs::{ListedEntry, Location, RemoteEditProgressFn, RemoteEditSession};

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
        listed: ListedEntry,
    },
    QuickAction {
        request: QuickActionRequest,
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
        /// Narrow typed progress callback (Verifying / RollbackOrRecovery) emitted
        /// by the provider at the real transaction boundary. TUI supplies the
        /// closure that publishes to JobManager; the executor just forwards it.
        progress: ProgressSlot,
    },
    OpenPath {
        path: PathBuf,
    },
}

/// ponytail: closure can't be Debug/Eq/Clone-trivially; wrap so Effect keeps its
/// ponytail: progress is a narrow Send+Sync callback (Arc<dyn Fn>); it carries no
/// data for Eq, so ProgressSlot stays PartialEq while the seam stays synchronous
/// and ordering-deterministic.
#[derive(Clone)]
pub struct ProgressSlot(pub Option<RemoteEditProgressFn>);
impl std::fmt::Debug for ProgressSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProgressSlot(..)")
    }
}
impl PartialEq for ProgressSlot {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}
impl Eq for ProgressSlot {}

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
    QuickActionFinished {
        result: Result<QuickActionOutcome, QuickActionFailure>,
    },
    /// Remote file downloaded to temp path for editing.
    Downloaded {
        session: RemoteEditSession,
    },
    /// Edited content successfully written back to remote.
    WrittenBack {
        name: String,
    },
    NoChange {
        name: String,
    },
    RemoteConflict {
        name: String,
        reason: String,
    },
    RecoveryRequired {
        name: String,
        details: String,
    },
    WrittenBackWarning {
        name: String,
        warning: String,
    },
    /// Typed remote-edit cancellation (queued or stale-origin). Surfaces as the
    /// typed RemoteEditOutcome::Cancelled — never inferred from Failed text.
    RemoteEditCancelled {
        name: String,
        reason: crate::jobs::RemoteEditCancelReason,
    },
    Failed {
        label: String,
        error: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::s3::S3ObjectRef;
    use crate::vfs::{Entry, EntryIdentity, EntryKind, ListedEntry, Location};

    // S3-27R A: identity survives the UI/effect boundary verbatim. A wrong
    // presentation name must never overwrite the exact provider-native key.
    #[test]
    fn preview_location_carries_exact_s3_identity() {
        let listed = ListedEntry {
            entry: Entry {
                name: "DISPLAY-WRONG.txt".into(),
                kind: EntryKind::File,
                size: Some(42),
                modified_unix_ms: None,
            },
            identity: EntryIdentity::S3Object(S3ObjectRef {
                target: "Prod".into(),
                bucket: "Bucket".into(),
                key: "foo/../REAL//����‍�����.txt".into(),
            }),
        };
        let effect = Effect::PreviewLocation {
            location: Location::S3 {
                target: "Prod".into(),
                bucket: Some("Bucket".into()),
                prefix: String::new(),
            },
            listed,
        };
        match effect {
            Effect::PreviewLocation { listed, .. } => {
                match listed.identity {
                    EntryIdentity::S3Object(ref r) => {
                        assert_eq!(r.key, "foo/../REAL//����‍�����.txt")
                    }
                    other => panic!("expected S3Object identity, got {other:?}"),
                }
                assert_eq!(listed.entry.name, "DISPLAY-WRONG.txt");
            }
            _ => panic!("expected PreviewLocation"),
        }
    }

    // S3-27R H: duplicate presentation names with distinct S3 identities stay distinct.
    #[test]
    fn preview_location_distinct_identities_same_name() {
        let mk = |key: &str| ListedEntry {
            entry: Entry {
                name: "dup.txt".into(),
                kind: EntryKind::File,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::S3Object(S3ObjectRef {
                target: "Prod".into(),
                bucket: "Bucket".into(),
                key: key.into(),
            }),
        };
        let loc = Location::S3 {
            target: "Prod".into(),
            bucket: Some("Bucket".into()),
            prefix: String::new(),
        };
        let a = Effect::PreviewLocation {
            location: loc.clone(),
            listed: mk("a/one.txt"),
        };
        let b = Effect::PreviewLocation {
            location: loc,
            listed: mk("b/two.txt"),
        };
        let listed_a = match a {
            Effect::PreviewLocation { listed, .. } => listed,
            _ => panic!("expected PreviewLocation"),
        };
        let listed_b = match b {
            Effect::PreviewLocation { listed, .. } => listed,
            _ => panic!("expected PreviewLocation"),
        };
        assert_ne!(listed_a.identity, listed_b.identity);
        assert_eq!(listed_a.entry.name, listed_b.entry.name);
    }
}
