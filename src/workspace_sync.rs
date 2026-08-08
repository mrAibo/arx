use std::collections::{BTreeMap, BTreeSet};

use crate::vfs::{EntryKind, Location};

/// Provider-neutral metadata used to compare workspace trees.
///
/// `modified_unix_ms` and `content_hash` are optional on purpose: the current
/// VFS entry model does not expose them for every provider. The comparison
/// engine never invents ordering or equality when the provider cannot prove it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceFingerprint {
    pub kind: EntryKind,
    pub size: Option<u64>,
    pub modified_unix_ms: Option<u64>,
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceEntry {
    pub relative_path: String,
    pub fingerprint: WorkspaceFingerprint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiffState {
    SameFingerprint,
    OnlyLeft,
    OnlyRight,
    LeftNewer,
    RightNewer,
    Different,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceDiffEntry {
    pub relative_path: String,
    pub state: DiffState,
    pub left: Option<WorkspaceFingerprint>,
    pub right: Option<WorkspaceFingerprint>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceDiff {
    pub left_root: Location,
    pub right_root: Location,
    pub entries: Vec<WorkspaceDiffEntry>,
}

impl WorkspaceDiff {
    pub fn compare(
        left_root: Location,
        right_root: Location,
        left_entries: impl IntoIterator<Item = WorkspaceEntry>,
        right_entries: impl IntoIterator<Item = WorkspaceEntry>,
    ) -> Self {
        let left: BTreeMap<String, WorkspaceFingerprint> = left_entries
            .into_iter()
            .map(|entry| {
                (
                    normalize_relative_path(&entry.relative_path),
                    entry.fingerprint,
                )
            })
            .collect();
        let right: BTreeMap<String, WorkspaceFingerprint> = right_entries
            .into_iter()
            .map(|entry| {
                (
                    normalize_relative_path(&entry.relative_path),
                    entry.fingerprint,
                )
            })
            .collect();

        let paths: BTreeSet<String> = left.keys().chain(right.keys()).cloned().collect();
        let entries = paths
            .into_iter()
            .map(|relative_path| {
                let left_fingerprint = left.get(&relative_path).cloned();
                let right_fingerprint = right.get(&relative_path).cloned();
                let state = classify(left_fingerprint.as_ref(), right_fingerprint.as_ref());
                WorkspaceDiffEntry {
                    relative_path,
                    state,
                    left: left_fingerprint,
                    right: right_fingerprint,
                }
            })
            .collect();

        Self {
            left_root,
            right_root,
            entries,
        }
    }

    pub fn changed_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.state != DiffState::SameFingerprint)
            .count()
    }
}

fn normalize_relative_path(path: &str) -> String {
    path.trim_start_matches("./")
        .trim_start_matches('/')
        .replace('\\', "/")
}

fn classify(
    left: Option<&WorkspaceFingerprint>,
    right: Option<&WorkspaceFingerprint>,
) -> DiffState {
    match (left, right) {
        (Some(_), None) => DiffState::OnlyLeft,
        (None, Some(_)) => DiffState::OnlyRight,
        (None, None) => unreachable!("comparison only classifies discovered paths"),
        (Some(left), Some(right)) => {
            if left.kind != right.kind {
                return DiffState::Different;
            }

            if let (Some(left_hash), Some(right_hash)) = (&left.content_hash, &right.content_hash) {
                if left_hash == right_hash {
                    return DiffState::SameFingerprint;
                }
                return DiffState::Different;
            }

            // Size alone is never enough evidence for equality. For providers
            // that do not expose hashes, require both size and timestamp to
            // agree before presenting an entry as equal.
            if let (Some(left_size), Some(right_size), Some(left_time), Some(right_time)) = (
                left.size,
                right.size,
                left.modified_unix_ms,
                right.modified_unix_ms,
            ) && left_size == right_size
                && left_time == right_time
            {
                return DiffState::SameFingerprint;
            }

            match (left.modified_unix_ms, right.modified_unix_ms) {
                (Some(left_time), Some(right_time)) if left_time > right_time => {
                    DiffState::LeftNewer
                }
                (Some(left_time), Some(right_time)) if right_time > left_time => {
                    DiffState::RightNewer
                }
                _ => DiffState::Different,
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncDirection {
    LeftToRight,
    RightToLeft,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncMode {
    /// Copy/update source-side changes, never delete destination-only entries.
    Update,
    /// Make destination match source; destination-only entries become deletes.
    Mirror,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictPolicy {
    RequireResolution,
    PreferSource,
    PreferDestination,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncPolicy {
    pub direction: SyncDirection,
    pub mode: SyncMode,
    pub conflicts: ConflictPolicy,
}

impl Default for SyncPolicy {
    fn default() -> Self {
        Self {
            direction: SyncDirection::LeftToRight,
            mode: SyncMode::Update,
            conflicts: ConflictPolicy::RequireResolution,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceSide {
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceSyncOperation {
    Copy {
        relative_path: String,
        from: WorkspaceSide,
        to: WorkspaceSide,
        bytes: Option<u64>,
    },
    Delete {
        relative_path: String,
        from: WorkspaceSide,
    },
    Skip {
        relative_path: String,
        reason: &'static str,
    },
    Conflict {
        relative_path: String,
        reason: &'static str,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSyncPlan {
    pub left_root: Location,
    pub right_root: Location,
    pub policy: SyncPolicy,
    pub operations: Vec<WorkspaceSyncOperation>,
    pub bytes_to_transfer: u64,
    pub destructive_operations: usize,
    pub conflicts: usize,
}

impl WorkspaceSyncPlan {
    pub fn build(diff: &WorkspaceDiff, policy: SyncPolicy) -> Self {
        let mut operations = Vec::with_capacity(diff.entries.len());
        let mut bytes_to_transfer = 0u64;
        let mut destructive_operations = 0usize;
        let mut conflicts = 0usize;

        for entry in &diff.entries {
            let operation = plan_entry(entry, policy);
            match &operation {
                WorkspaceSyncOperation::Copy { bytes, .. } => {
                    bytes_to_transfer = bytes_to_transfer.saturating_add(bytes.unwrap_or(0));
                }
                WorkspaceSyncOperation::Delete { .. } => {
                    destructive_operations += 1;
                }
                WorkspaceSyncOperation::Conflict { .. } => {
                    conflicts += 1;
                }
                WorkspaceSyncOperation::Skip { .. } => {}
            }
            operations.push(operation);
        }

        Self {
            left_root: diff.left_root.clone(),
            right_root: diff.right_root.clone(),
            policy,
            operations,
            bytes_to_transfer,
            destructive_operations,
            conflicts,
        }
    }

    /// Execution UI must explicitly confirm any mirror/delete/conflict plan.
    pub fn requires_confirmation(&self) -> bool {
        self.policy.mode == SyncMode::Mirror
            || self.destructive_operations > 0
            || self.conflicts > 0
    }

    pub fn can_execute(&self) -> bool {
        self.conflicts == 0
    }
}

fn plan_entry(entry: &WorkspaceDiffEntry, policy: SyncPolicy) -> WorkspaceSyncOperation {
    let (source_side, destination_side) = match policy.direction {
        SyncDirection::LeftToRight => (WorkspaceSide::Left, WorkspaceSide::Right),
        SyncDirection::RightToLeft => (WorkspaceSide::Right, WorkspaceSide::Left),
    };

    let source = match source_side {
        WorkspaceSide::Left => entry.left.as_ref(),
        WorkspaceSide::Right => entry.right.as_ref(),
    };
    let destination = match destination_side {
        WorkspaceSide::Left => entry.left.as_ref(),
        WorkspaceSide::Right => entry.right.as_ref(),
    };

    let source_only = match policy.direction {
        SyncDirection::LeftToRight => entry.state == DiffState::OnlyLeft,
        SyncDirection::RightToLeft => entry.state == DiffState::OnlyRight,
    };
    let destination_only = match policy.direction {
        SyncDirection::LeftToRight => entry.state == DiffState::OnlyRight,
        SyncDirection::RightToLeft => entry.state == DiffState::OnlyLeft,
    };
    let source_newer = match policy.direction {
        SyncDirection::LeftToRight => entry.state == DiffState::LeftNewer,
        SyncDirection::RightToLeft => entry.state == DiffState::RightNewer,
    };
    let destination_newer = match policy.direction {
        SyncDirection::LeftToRight => entry.state == DiffState::RightNewer,
        SyncDirection::RightToLeft => entry.state == DiffState::LeftNewer,
    };

    if entry.state == DiffState::SameFingerprint {
        return WorkspaceSyncOperation::Skip {
            relative_path: entry.relative_path.clone(),
            reason: "same fingerprint",
        };
    }

    if source_only || source_newer {
        return WorkspaceSyncOperation::Copy {
            relative_path: entry.relative_path.clone(),
            from: source_side,
            to: destination_side,
            bytes: source.and_then(|fingerprint| fingerprint.size),
        };
    }

    if destination_only {
        return match policy.mode {
            SyncMode::Update => WorkspaceSyncOperation::Skip {
                relative_path: entry.relative_path.clone(),
                reason: "destination-only entry preserved by update mode",
            },
            SyncMode::Mirror => WorkspaceSyncOperation::Delete {
                relative_path: entry.relative_path.clone(),
                from: destination_side,
            },
        };
    }

    if destination_newer || entry.state == DiffState::Different {
        return match policy.conflicts {
            ConflictPolicy::RequireResolution => WorkspaceSyncOperation::Conflict {
                relative_path: entry.relative_path.clone(),
                reason: if destination_newer {
                    "destination is newer than source"
                } else {
                    "entries differ and ordering cannot be proven"
                },
            },
            ConflictPolicy::PreferSource => WorkspaceSyncOperation::Copy {
                relative_path: entry.relative_path.clone(),
                from: source_side,
                to: destination_side,
                bytes: source.and_then(|fingerprint| fingerprint.size),
            },
            ConflictPolicy::PreferDestination => WorkspaceSyncOperation::Skip {
                relative_path: entry.relative_path.clone(),
                reason: "destination preferred by conflict policy",
            },
        };
    }

    WorkspaceSyncOperation::Conflict {
        relative_path: entry.relative_path.clone(),
        reason: if destination.is_some() {
            "unclassified workspace difference"
        } else {
            "source entry is unavailable"
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn local(path: &str) -> Location {
        Location::Local(PathBuf::from(path))
    }

    fn fp(size: u64, modified_unix_ms: Option<u64>) -> WorkspaceFingerprint {
        WorkspaceFingerprint {
            kind: EntryKind::File,
            size: Some(size),
            modified_unix_ms,
            content_hash: None,
        }
    }

    fn entry(path: &str, fingerprint: WorkspaceFingerprint) -> WorkspaceEntry {
        WorkspaceEntry {
            relative_path: path.into(),
            fingerprint,
        }
    }

    #[test]
    fn comparison_is_deterministic_and_detects_sides() {
        let diff = WorkspaceDiff::compare(
            local("/left"),
            local("/right"),
            vec![
                entry("b.txt", fp(2, Some(10))),
                entry("a.txt", fp(1, Some(10))),
            ],
            vec![
                entry("b.txt", fp(2, Some(10))),
                entry("c.txt", fp(3, Some(10))),
            ],
        );

        let states: Vec<(&str, DiffState)> = diff
            .entries
            .iter()
            .map(|item| (item.relative_path.as_str(), item.state))
            .collect();

        assert_eq!(
            states,
            vec![
                ("a.txt", DiffState::OnlyLeft),
                ("b.txt", DiffState::SameFingerprint),
                ("c.txt", DiffState::OnlyRight),
            ]
        );
    }

    #[test]
    fn equal_size_without_equal_fingerprint_is_not_assumed_equal() {
        let diff = WorkspaceDiff::compare(
            local("/left"),
            local("/right"),
            vec![entry("config.toml", fp(100, None))],
            vec![entry("config.toml", fp(100, None))],
        );

        assert_eq!(diff.entries[0].state, DiffState::Different);
    }

    #[test]
    fn equal_hash_is_proof_even_when_timestamps_are_missing() {
        let fingerprint = WorkspaceFingerprint {
            kind: EntryKind::File,
            size: Some(100),
            modified_unix_ms: None,
            content_hash: Some("sha256:abc".into()),
        };
        let diff = WorkspaceDiff::compare(
            local("/left"),
            local("/right"),
            vec![entry("config.toml", fingerprint.clone())],
            vec![entry("config.toml", fingerprint)],
        );

        assert_eq!(diff.entries[0].state, DiffState::SameFingerprint);
    }

    #[test]
    fn equal_size_and_timestamp_is_provider_neutral_fingerprint_proof() {
        let diff = WorkspaceDiff::compare(
            local("/left"),
            local("/right"),
            vec![entry("config.toml", fp(100, Some(42)))],
            vec![entry("config.toml", fp(100, Some(42)))],
        );

        assert_eq!(diff.entries[0].state, DiffState::SameFingerprint);
    }

    #[test]
    fn update_mode_preserves_destination_only_entries() {
        let diff = WorkspaceDiff::compare(
            local("/left"),
            local("/right"),
            Vec::<WorkspaceEntry>::new(),
            vec![entry("remote.log", fp(200, Some(10)))],
        );
        let plan = WorkspaceSyncPlan::build(&diff, SyncPolicy::default());

        assert!(matches!(
            &plan.operations[0],
            WorkspaceSyncOperation::Skip { .. }
        ));
        assert_eq!(plan.destructive_operations, 0);
    }

    #[test]
    fn mirror_mode_turns_destination_only_entries_into_deletes() {
        let diff = WorkspaceDiff::compare(
            local("/left"),
            local("/right"),
            Vec::<WorkspaceEntry>::new(),
            vec![entry("old-build.tar", fp(200, Some(10)))],
        );
        let plan = WorkspaceSyncPlan::build(
            &diff,
            SyncPolicy {
                mode: SyncMode::Mirror,
                ..SyncPolicy::default()
            },
        );

        assert!(matches!(
            &plan.operations[0],
            WorkspaceSyncOperation::Delete {
                from: WorkspaceSide::Right,
                ..
            }
        ));
        assert_eq!(plan.destructive_operations, 1);
        assert!(plan.requires_confirmation());
    }

    #[test]
    fn destination_newer_is_conflict_by_default() {
        let diff = WorkspaceDiff::compare(
            local("/left"),
            local("/right"),
            vec![entry("config.toml", fp(100, Some(10)))],
            vec![entry("config.toml", fp(120, Some(20)))],
        );
        let plan = WorkspaceSyncPlan::build(&diff, SyncPolicy::default());

        assert!(matches!(
            &plan.operations[0],
            WorkspaceSyncOperation::Conflict { .. }
        ));
        assert_eq!(plan.conflicts, 1);
        assert!(!plan.can_execute());
    }

    #[test]
    fn right_to_left_reverses_copy_direction() {
        let diff = WorkspaceDiff::compare(
            local("/left"),
            local("/right"),
            Vec::<WorkspaceEntry>::new(),
            vec![entry("artifact.bin", fp(4096, Some(20)))],
        );
        let plan = WorkspaceSyncPlan::build(
            &diff,
            SyncPolicy {
                direction: SyncDirection::RightToLeft,
                ..SyncPolicy::default()
            },
        );

        assert!(matches!(
            &plan.operations[0],
            WorkspaceSyncOperation::Copy {
                from: WorkspaceSide::Right,
                to: WorkspaceSide::Left,
                ..
            }
        ));
        assert_eq!(plan.bytes_to_transfer, 4096);
    }
}
