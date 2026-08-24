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
    ListScreenSessions,
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
/// One GNU Screen session from `screen -ls` (#7).
///
/// `id` preserves the EXACT raw identifier required by `screen -r <id>`;
/// presentation must never reconstruct or reformat it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenSessionInfo {
    pub id: String,
    pub status: ScreenSessionStatus,
}

/// Real GNU screen session states; anything unproven stays Unknown (fail-closed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenSessionStatus {
    Detached,
    Attached,
    Multi,
    Dead,
    Unreachable,
    Unknown,
}

impl ScreenSessionStatus {
    /// Factual, user-facing reason for non-attachable states.
    pub fn unavailable_reason(self) -> Option<&'static str> {
        match self {
            ScreenSessionStatus::Detached => None,
            ScreenSessionStatus::Attached => Some("session is attached elsewhere"),
            ScreenSessionStatus::Multi => Some("session is multi-attached"),
            ScreenSessionStatus::Dead => Some("session is dead"),
            ScreenSessionStatus::Unreachable => Some("session is unreachable"),
            ScreenSessionStatus::Unknown => Some("session state unknown"),
        }
    }
}

/// Pure parser for `screen -ls` output (#7).
///
/// - preserves the exact raw `<pid>.<name>` id used by `screen -r`
/// - recognizes Detached/Attached/Multi/Dead/Unreachable
/// - header/summary/malformed lines never become targets
/// - formatting whitespace never alters the raw id
pub fn parse_screen_ls(output: &str) -> Vec<ScreenSessionInfo> {
    const TAB: char = '\u{0009}';
    let mut sessions = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut fields = trimmed.split_whitespace();
        // Session rows look like: "<pid>.<name>\t(Status)" (possibly with
        // date/extra columns). Header/summary lines have no tab-separated
        // "<digits>.<name>" first field and never become targets.
        let Some(raw_id) = fields.next() else {
            continue;
        };
        let Some((pid, name)) = raw_id.split_once('.') else {
            continue;
        };
        if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) || name.is_empty() {
            continue;
        }
        let _ = TAB; // formatting whitespace is handled by split_whitespace

        // Status token: "(Status)" anywhere after the id.
        let status = fields
            .find_map(|token| {
                let inner = token.trim_start_matches('(').trim_end_matches(')');
                match inner {
                    "Detached" => Some(ScreenSessionStatus::Detached),
                    "Attached" => Some(ScreenSessionStatus::Attached),
                    "Multi" | "Multiuser" => Some(ScreenSessionStatus::Multi),
                    "Dead" => Some(ScreenSessionStatus::Dead),
                    "Unreachable" => Some(ScreenSessionStatus::Unreachable),
                    _ => None,
                }
            })
            .unwrap_or(ScreenSessionStatus::Unknown);
        // Preserve the EXACT raw id — `screen -r <id>` needs pid.name verbatim.
        sessions.push(ScreenSessionInfo {
            id: raw_id.to_string(),
            status,
        });
    }
    sessions
}

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
    ScreenSessions {
        sessions: Vec<ScreenSessionInfo>,
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
