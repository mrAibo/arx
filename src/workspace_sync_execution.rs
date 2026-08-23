use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::vfs::{Capability, Location, ProviderId, ProviderRegistry};
use crate::workspace_sync::{
    SyncDirection, SyncMode, WorkspaceDiff, WorkspaceDiffEntry, WorkspaceFingerprint,
    WorkspaceSide, WorkspaceSyncOperation, WorkspaceSyncPlan,
};

static NEXT_SYNC_PLAN_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SyncPlanId(u64);

impl SyncPlanId {
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlanDigest([u64; 2]);

impl PlanDigest {
    pub fn as_hex(&self) -> String {
        format!("{:016x}{:016x}", self.0[0], self.0[1])
    }
}

impl fmt::Display for PlanDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}{:016x}", self.0[0], self.0[1])
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrozenSyncOperation {
    Copy {
        relative_path: String,
        from: WorkspaceSide,
        to: WorkspaceSide,
        expected_source: WorkspaceFingerprint,
        expected_destination: Option<WorkspaceFingerprint>,
    },
    Delete {
        relative_path: String,
        from: WorkspaceSide,
        expected_target: WorkspaceFingerprint,
    },
}

impl FrozenSyncOperation {
    pub fn relative_path(&self) -> &str {
        match self {
            Self::Copy { relative_path, .. } | Self::Delete { relative_path, .. } => relative_path,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrozenWorkspaceSyncPlan {
    id: SyncPlanId,
    left_root: Location,
    right_root: Location,
    direction: SyncDirection,
    mode: SyncMode,
    operations: Vec<FrozenSyncOperation>,
    total_bytes: u64,
    destructive_operations: usize,
    digest: PlanDigest,
}

impl FrozenWorkspaceSyncPlan {
    pub const fn id(&self) -> SyncPlanId {
        self.id
    }

    pub fn left_root(&self) -> &Location {
        &self.left_root
    }

    pub fn right_root(&self) -> &Location {
        &self.right_root
    }

    pub const fn direction(&self) -> SyncDirection {
        self.direction
    }

    pub const fn mode(&self) -> SyncMode {
        self.mode
    }

    pub fn operations(&self) -> &[FrozenSyncOperation] {
        &self.operations
    }

    pub const fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    pub const fn destructive_operations(&self) -> usize {
        self.destructive_operations
    }

    pub const fn digest(&self) -> PlanDigest {
        self.digest
    }

    pub fn requires_confirmation(&self) -> bool {
        self.mode == SyncMode::Mirror || self.destructive_operations > 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SyncValidationError {
    #[error("sync plan still contains {count} unresolved conflict(s)")]
    ConflictsRemaining { count: usize },
    #[error("source provider {provider:?} does not support required capability {capability:?}")]
    UnsupportedSourceCapability {
        provider: ProviderId,
        capability: Capability,
    },
    #[error(
        "destination provider {provider:?} does not support required capability {capability:?}"
    )]
    UnsupportedDestinationCapability {
        provider: ProviderId,
        capability: Capability,
    },
    #[error("no safe sync transfer path for {source_provider:?} -> {destination_provider:?}")]
    UnsupportedTransferPath {
        source_provider: ProviderId,
        destination_provider: ProviderId,
    },
    #[error("unsafe delete path in sync plan: {path}")]
    UnsafeDelete { path: String },
    #[error("unsafe relative path in sync plan: {path}")]
    UnsafePath { path: String },
    #[error("source changed after sync preview: {path}")]
    SourceChanged { path: String },
    #[error("destination changed after sync preview: {path}")]
    DestinationChanged { path: String },
    #[error("workspace roots changed after sync preview")]
    RootChanged,
    #[error("sync plan no longer matches the workspace preview")]
    PlanChanged,
    #[error("sync plan contains no executable operations")]
    EmptyPlan,
}

#[derive(Debug, Default)]
pub struct SyncPlanValidator;

impl SyncPlanValidator {
    pub fn freeze(
        plan: &WorkspaceSyncPlan,
        preview: &WorkspaceDiff,
        registry: &ProviderRegistry,
    ) -> Result<FrozenWorkspaceSyncPlan, SyncValidationError> {
        Self::validate_roots(
            &plan.left_root,
            &plan.right_root,
            &preview.left_root,
            &preview.right_root,
        )?;

        if plan.conflicts > 0 {
            return Err(SyncValidationError::ConflictsRemaining {
                count: plan.conflicts,
            });
        }

        if WorkspaceSyncPlan::build(preview, plan.policy) != *plan {
            return Err(SyncValidationError::PlanChanged);
        }

        let entries = index_entries(preview);
        let mut operations = Vec::new();

        for operation in &plan.operations {
            match operation {
                WorkspaceSyncOperation::Copy {
                    relative_path,
                    from,
                    to,
                    ..
                } => {
                    validate_relative_path(relative_path, false)?;
                    Self::validate_transfer_pair(plan, *from, *to)?;

                    let entry = entries
                        .get(relative_path.as_str())
                        .copied()
                        .ok_or(SyncValidationError::PlanChanged)?;
                    let expected_source =
                        fingerprint_for(entry, *from).cloned().ok_or_else(|| {
                            SyncValidationError::SourceChanged {
                                path: relative_path.clone(),
                            }
                        })?;
                    let expected_destination = fingerprint_for(entry, *to).cloned();

                    operations.push(FrozenSyncOperation::Copy {
                        relative_path: relative_path.clone(),
                        from: *from,
                        to: *to,
                        expected_source,
                        expected_destination,
                    });
                }
                WorkspaceSyncOperation::Delete {
                    relative_path,
                    from,
                } => {
                    validate_relative_path(relative_path, true)?;
                    Self::validate_delete_capability(plan, *from, registry)?;

                    let entry = entries
                        .get(relative_path.as_str())
                        .copied()
                        .ok_or(SyncValidationError::PlanChanged)?;
                    let expected_target =
                        fingerprint_for(entry, *from).cloned().ok_or_else(|| {
                            SyncValidationError::DestinationChanged {
                                path: relative_path.clone(),
                            }
                        })?;

                    operations.push(FrozenSyncOperation::Delete {
                        relative_path: relative_path.clone(),
                        from: *from,
                        expected_target,
                    });
                }
                WorkspaceSyncOperation::Skip { .. } => {}
                WorkspaceSyncOperation::Conflict { .. } => {
                    return Err(SyncValidationError::ConflictsRemaining { count: 1 });
                }
            }
        }

        if operations.is_empty() {
            return Err(SyncValidationError::EmptyPlan);
        }

        let destructive_operations = operations
            .iter()
            .filter(|operation| matches!(operation, FrozenSyncOperation::Delete { .. }))
            .count();
        let digest = digest_plan(
            &plan.left_root,
            &plan.right_root,
            plan.policy.direction,
            plan.policy.mode,
            &operations,
            plan.bytes_to_transfer,
            destructive_operations,
        );

        Ok(FrozenWorkspaceSyncPlan {
            id: SyncPlanId(NEXT_SYNC_PLAN_ID.fetch_add(1, Ordering::Relaxed)),
            left_root: plan.left_root.clone(),
            right_root: plan.right_root.clone(),
            direction: plan.policy.direction,
            mode: plan.policy.mode,
            operations,
            total_bytes: plan.bytes_to_transfer,
            destructive_operations,
            digest,
        })
    }

    pub fn validate_frozen(
        plan: &FrozenWorkspaceSyncPlan,
        current: &WorkspaceDiff,
    ) -> Result<(), SyncValidationError> {
        Self::validate_roots(
            &plan.left_root,
            &plan.right_root,
            &current.left_root,
            &current.right_root,
        )?;

        let entries = index_entries(current);
        for operation in &plan.operations {
            Self::validate_operation_against_entries(operation, &entries)?;
        }

        Ok(())
    }

    /// Re-check one frozen operation against a fresh workspace snapshot.
    ///
    /// The executor uses this immediately before destructive operations so a
    /// target that changed mid-run is never deleted under stale assumptions.
    pub fn validate_operation(
        operation: &FrozenSyncOperation,
        current: &WorkspaceDiff,
    ) -> Result<(), SyncValidationError> {
        let entries = index_entries(current);
        Self::validate_operation_against_entries(operation, &entries)
    }

    fn validate_operation_against_entries(
        operation: &FrozenSyncOperation,
        entries: &BTreeMap<&str, &WorkspaceDiffEntry>,
    ) -> Result<(), SyncValidationError> {
        match operation {
            FrozenSyncOperation::Copy {
                relative_path,
                from,
                to,
                expected_source,
                expected_destination,
            } => {
                let entry = entries.get(relative_path.as_str()).copied();
                let actual_source = entry.and_then(|item| fingerprint_for(item, *from));
                if actual_source != Some(expected_source) {
                    return Err(SyncValidationError::SourceChanged {
                        path: relative_path.clone(),
                    });
                }

                let actual_destination = entry.and_then(|item| fingerprint_for(item, *to));
                if actual_destination != expected_destination.as_ref() {
                    return Err(SyncValidationError::DestinationChanged {
                        path: relative_path.clone(),
                    });
                }
            }
            FrozenSyncOperation::Delete {
                relative_path,
                from,
                expected_target,
            } => {
                let entry = entries.get(relative_path.as_str()).copied();
                let actual_target = entry.and_then(|item| fingerprint_for(item, *from));
                if actual_target != Some(expected_target) {
                    return Err(SyncValidationError::DestinationChanged {
                        path: relative_path.clone(),
                    });
                }
            }
        }

        Ok(())
    }

    fn validate_roots(
        expected_left: &Location,
        expected_right: &Location,
        actual_left: &Location,
        actual_right: &Location,
    ) -> Result<(), SyncValidationError> {
        if expected_left == actual_left && expected_right == actual_right {
            return Ok(());
        }

        Err(SyncValidationError::RootChanged)
    }

    fn validate_transfer_pair(
        plan: &WorkspaceSyncPlan,
        from: WorkspaceSide,
        to: WorkspaceSide,
    ) -> Result<(), SyncValidationError> {
        let source = root_for_side(plan, from).provider_id();
        let destination = root_for_side(plan, to).provider_id();

        match (source, destination) {
            (ProviderId::Local, ProviderId::Local)
            | (ProviderId::Local, ProviderId::Sftp)
            | (ProviderId::Sftp, ProviderId::Local) => Ok(()),
            (ProviderId::Archive, _) => Err(SyncValidationError::UnsupportedSourceCapability {
                provider: source,
                capability: Capability::Read,
            }),
            (_, ProviderId::Archive) => {
                Err(SyncValidationError::UnsupportedDestinationCapability {
                    provider: destination,
                    capability: Capability::Write,
                })
            }
            _ => Err(SyncValidationError::UnsupportedTransferPath {
                source_provider: source,
                destination_provider: destination,
            }),
        }
    }

    fn validate_delete_capability(
        plan: &WorkspaceSyncPlan,
        side: WorkspaceSide,
        registry: &ProviderRegistry,
    ) -> Result<(), SyncValidationError> {
        let location = root_for_side(plan, side).clone();
        let provider = location.provider_id();
        if registry.supports_at(&location, Capability::Delete) {
            Ok(())
        } else {
            Err(SyncValidationError::UnsupportedDestinationCapability {
                provider,
                capability: Capability::Delete,
            })
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncConfirmationToken {
    plan_id: SyncPlanId,
    digest: PlanDigest,
    destructive_operations: usize,
}

impl SyncConfirmationToken {
    pub fn from_explicit_confirmation(plan: &FrozenWorkspaceSyncPlan) -> Self {
        Self {
            plan_id: plan.id,
            digest: plan.digest,
            destructive_operations: plan.destructive_operations,
        }
    }

    pub const fn plan_id(self) -> SyncPlanId {
        self.plan_id
    }

    pub const fn digest(self) -> PlanDigest {
        self.digest
    }

    pub const fn destructive_operations(self) -> usize {
        self.destructive_operations
    }

    fn matches(self, plan: &FrozenWorkspaceSyncPlan) -> bool {
        self.plan_id == plan.id
            && self.digest == plan.digest
            && self.destructive_operations == plan.destructive_operations
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SyncExecutionGateError {
    #[error("sync plan requires explicit confirmation")]
    ConfirmationRequired,
    #[error("sync confirmation does not match the frozen plan")]
    StaleConfirmation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableSyncPlan {
    plan: FrozenWorkspaceSyncPlan,
    confirmation: Option<SyncConfirmationToken>,
}

impl ExecutableSyncPlan {
    pub fn new(
        plan: FrozenWorkspaceSyncPlan,
        confirmation: Option<SyncConfirmationToken>,
    ) -> Result<Self, SyncExecutionGateError> {
        if plan.requires_confirmation() {
            let token = confirmation.ok_or(SyncExecutionGateError::ConfirmationRequired)?;
            if !token.matches(&plan) {
                return Err(SyncExecutionGateError::StaleConfirmation);
            }
            return Ok(Self {
                plan,
                confirmation: Some(token),
            });
        }

        if confirmation.is_some_and(|token| !token.matches(&plan)) {
            return Err(SyncExecutionGateError::StaleConfirmation);
        }

        Ok(Self { plan, confirmation })
    }

    pub fn plan(&self) -> &FrozenWorkspaceSyncPlan {
        &self.plan
    }

    pub const fn confirmation(&self) -> Option<SyncConfirmationToken> {
        self.confirmation
    }

    pub fn into_plan(self) -> FrozenWorkspaceSyncPlan {
        self.plan
    }
}

fn root_for_side(plan: &WorkspaceSyncPlan, side: WorkspaceSide) -> &Location {
    match side {
        WorkspaceSide::Left => &plan.left_root,
        WorkspaceSide::Right => &plan.right_root,
    }
}

fn fingerprint_for(
    entry: &WorkspaceDiffEntry,
    side: WorkspaceSide,
) -> Option<&WorkspaceFingerprint> {
    match side {
        WorkspaceSide::Left => entry.left.as_ref(),
        WorkspaceSide::Right => entry.right.as_ref(),
    }
}

fn index_entries(diff: &WorkspaceDiff) -> BTreeMap<&str, &WorkspaceDiffEntry> {
    diff.entries
        .iter()
        .map(|entry| (entry.relative_path.as_str(), entry))
        .collect()
}

fn validate_relative_path(path: &str, delete: bool) -> Result<(), SyncValidationError> {
    let normalized = path.replace('\\', "/");
    let unsafe_path = normalized.is_empty()
        || normalized.starts_with('/')
        || normalized.contains('\0')
        || normalized
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..");

    if unsafe_path {
        if delete {
            Err(SyncValidationError::UnsafeDelete {
                path: path.to_string(),
            })
        } else {
            Err(SyncValidationError::UnsafePath {
                path: path.to_string(),
            })
        }
    } else {
        Ok(())
    }
}

fn digest_plan(
    left_root: &Location,
    right_root: &Location,
    direction: SyncDirection,
    mode: SyncMode,
    operations: &[FrozenSyncOperation],
    total_bytes: u64,
    destructive_operations: usize,
) -> PlanDigest {
    let mut digest = DigestBuilder::new();
    digest.bytes(b"arx.workspace-sync-plan.v1");
    digest.location(left_root);
    digest.location(right_root);
    digest.u8(match direction {
        SyncDirection::LeftToRight => 0,
        SyncDirection::RightToLeft => 1,
    });
    digest.u8(match mode {
        SyncMode::Update => 0,
        SyncMode::Mirror => 1,
    });
    digest.u64(total_bytes);
    digest.u64(destructive_operations as u64);
    digest.u64(operations.len() as u64);

    for operation in operations {
        match operation {
            FrozenSyncOperation::Copy {
                relative_path,
                from,
                to,
                expected_source,
                expected_destination,
            } => {
                digest.u8(0);
                digest.bytes(relative_path.as_bytes());
                digest.side(*from);
                digest.side(*to);
                digest.fingerprint(expected_source);
                digest.optional_fingerprint(expected_destination.as_ref());
            }
            FrozenSyncOperation::Delete {
                relative_path,
                from,
                expected_target,
            } => {
                digest.u8(1);
                digest.bytes(relative_path.as_bytes());
                digest.side(*from);
                digest.fingerprint(expected_target);
            }
        }
    }

    digest.finish()
}

struct DigestBuilder {
    left: u64,
    right: u64,
}

impl DigestBuilder {
    const FNV_PRIME: u64 = 1_099_511_628_211;

    fn new() -> Self {
        Self {
            left: 0xcbf2_9ce4_8422_2325,
            right: 0x8422_2325_cbf2_9ce4,
        }
    }

    fn finish(self) -> PlanDigest {
        PlanDigest([self.left, self.right])
    }

    fn raw(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.left ^= u64::from(byte);
            self.left = self.left.wrapping_mul(Self::FNV_PRIME);

            self.right ^= u64::from(byte ^ 0xa5);
            self.right = self.right.wrapping_mul(Self::FNV_PRIME);
        }
    }

    fn bytes(&mut self, bytes: &[u8]) {
        self.u64(bytes.len() as u64);
        self.raw(bytes);
    }

    fn u8(&mut self, value: u8) {
        self.raw(&[value]);
    }

    fn u64(&mut self, value: u64) {
        self.raw(&value.to_le_bytes());
    }

    fn side(&mut self, side: WorkspaceSide) {
        self.u8(match side {
            WorkspaceSide::Left => 0,
            WorkspaceSide::Right => 1,
        });
    }

    fn location(&mut self, location: &Location) {
        match location {
            Location::Local(path) => {
                self.u8(0);
                self.bytes(path.to_string_lossy().as_bytes());
            }
            Location::Sftp { host, path } => {
                self.u8(1);
                self.bytes(host.as_bytes());
                self.bytes(path.as_bytes());
            }
            Location::Archive {
                archive,
                inner_path,
            } => {
                self.u8(2);
                self.bytes(archive.to_string_lossy().as_bytes());
                self.bytes(inner_path.as_bytes());
            }
            // ponytail: total collision-safe S3 identity (tag 3); None vs Some("")
            // get distinct discriminants; no normalization
            Location::S3 {
                target,
                bucket,
                prefix,
            } => {
                self.u8(3);
                self.bytes(target.as_bytes());
                match bucket {
                    Some(bucket) => {
                        self.u8(1);
                        self.bytes(bucket.as_bytes());
                    }
                    None => self.u8(0),
                }
                self.bytes(prefix.as_bytes());
            }
            Location::WebDav { target, path } => {
                self.u8(4);
                self.bytes(target.as_bytes());
                self.bytes(path.as_bytes());
            }
        }
    }

    fn fingerprint(&mut self, fingerprint: &WorkspaceFingerprint) {
        self.u8(match fingerprint.kind {
            crate::vfs::EntryKind::File => 0,
            crate::vfs::EntryKind::Directory => 1,
            crate::vfs::EntryKind::Symlink => 2,
            crate::vfs::EntryKind::Other => 3,
        });
        self.optional_u64(fingerprint.size);
        self.optional_u64(fingerprint.modified_unix_ms);
        self.optional_str(fingerprint.content_hash.as_deref());
    }

    fn optional_fingerprint(&mut self, fingerprint: Option<&WorkspaceFingerprint>) {
        match fingerprint {
            Some(fingerprint) => {
                self.u8(1);
                self.fingerprint(fingerprint);
            }
            None => self.u8(0),
        }
    }

    fn optional_u64(&mut self, value: Option<u64>) {
        match value {
            Some(value) => {
                self.u8(1);
                self.u64(value);
            }
            None => self.u8(0),
        }
    }

    fn optional_str(&mut self, value: Option<&str>) {
        match value {
            Some(value) => {
                self.u8(1);
                self.bytes(value.as_bytes());
            }
            None => self.u8(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::{EntryKind, default_registry};
    use crate::workspace_sync::{ConflictPolicy, SyncPolicy, WorkspaceEntry, WorkspaceFingerprint};
    use std::path::PathBuf;

    fn local(path: &str) -> Location {
        Location::Local(PathBuf::from(path))
    }

    fn sftp(host: &str, path: &str) -> Location {
        Location::Sftp {
            host: host.into(),
            path: path.into(),
        }
    }

    fn fp(size: u64, modified_unix_ms: u64) -> WorkspaceFingerprint {
        WorkspaceFingerprint {
            kind: EntryKind::File,
            size: Some(size),
            modified_unix_ms: Some(modified_unix_ms),
            content_hash: None,
        }
    }

    fn entry(path: &str, fingerprint: WorkspaceFingerprint) -> WorkspaceEntry {
        WorkspaceEntry {
            relative_path: path.into(),
            fingerprint,
        }
    }

    fn diff(
        left_entries: Vec<WorkspaceEntry>,
        right_entries: Vec<WorkspaceEntry>,
    ) -> WorkspaceDiff {
        WorkspaceDiff::compare(local("/left"), local("/right"), left_entries, right_entries)
    }

    #[test]
    fn freeze_captures_exact_copy_preconditions() {
        let preview = diff(
            vec![entry("a.txt", fp(10, 20))],
            vec![entry("a.txt", fp(9, 10))],
        );
        let plan = WorkspaceSyncPlan::build(
            &preview,
            SyncPolicy {
                conflicts: ConflictPolicy::PreferSource,
                ..SyncPolicy::default()
            },
        );

        let frozen = SyncPlanValidator::freeze(&plan, &preview, &default_registry()).unwrap();

        assert_eq!(frozen.operations().len(), 1);
        assert_eq!(frozen.total_bytes(), 10);
        assert_eq!(frozen.destructive_operations(), 0);
        assert!(!frozen.requires_confirmation());
        assert!(matches!(
            &frozen.operations()[0],
            FrozenSyncOperation::Copy {
                expected_source,
                expected_destination: Some(expected_destination),
                ..
            } if *expected_source == fp(10, 20) && *expected_destination == fp(9, 10)
        ));
    }

    #[test]
    fn freeze_rejects_unresolved_conflicts() {
        let preview = diff(
            vec![entry("a.txt", fp(10, 10))],
            vec![entry("a.txt", fp(11, 10))],
        );
        let plan = WorkspaceSyncPlan::build(&preview, SyncPolicy::default());

        assert!(matches!(
            SyncPlanValidator::freeze(&plan, &preview, &default_registry()),
            Err(SyncValidationError::ConflictsRemaining { count: 1 })
        ));
    }

    #[test]
    fn freeze_rejects_plan_that_no_longer_matches_preview() {
        let preview = diff(vec![entry("a.txt", fp(10, 20))], Vec::new());
        let mut plan = WorkspaceSyncPlan::build(&preview, SyncPolicy::default());
        plan.policy.direction = SyncDirection::RightToLeft;

        assert_eq!(
            SyncPlanValidator::freeze(&plan, &preview, &default_registry()),
            Err(SyncValidationError::PlanChanged)
        );
    }

    #[test]
    fn frozen_plan_detects_source_change() {
        let preview = diff(vec![entry("a.txt", fp(10, 20))], Vec::new());
        let plan = WorkspaceSyncPlan::build(&preview, SyncPolicy::default());
        let frozen = SyncPlanValidator::freeze(&plan, &preview, &default_registry()).unwrap();

        let current = diff(vec![entry("a.txt", fp(11, 21))], Vec::new());
        assert_eq!(
            SyncPlanValidator::validate_frozen(&frozen, &current),
            Err(SyncValidationError::SourceChanged {
                path: "a.txt".into()
            })
        );
    }

    #[test]
    fn frozen_plan_detects_destination_change() {
        let preview = diff(vec![entry("a.txt", fp(10, 20))], Vec::new());
        let plan = WorkspaceSyncPlan::build(&preview, SyncPolicy::default());
        let frozen = SyncPlanValidator::freeze(&plan, &preview, &default_registry()).unwrap();

        let current = diff(
            vec![entry("a.txt", fp(10, 20))],
            vec![entry("a.txt", fp(5, 5))],
        );
        assert_eq!(
            SyncPlanValidator::validate_frozen(&frozen, &current),
            Err(SyncValidationError::DestinationChanged {
                path: "a.txt".into()
            })
        );
    }

    #[test]
    fn mirror_delete_requires_matching_confirmation() {
        let preview = diff(Vec::new(), vec![entry("old.txt", fp(5, 5))]);
        let plan = WorkspaceSyncPlan::build(
            &preview,
            SyncPolicy {
                mode: SyncMode::Mirror,
                ..SyncPolicy::default()
            },
        );
        let frozen = SyncPlanValidator::freeze(&plan, &preview, &default_registry()).unwrap();

        assert!(matches!(
            ExecutableSyncPlan::new(frozen.clone(), None),
            Err(SyncExecutionGateError::ConfirmationRequired)
        ));

        let token = SyncConfirmationToken::from_explicit_confirmation(&frozen);
        let executable = ExecutableSyncPlan::new(frozen.clone(), Some(token)).unwrap();
        assert_eq!(executable.plan().id(), frozen.id());
    }

    #[test]
    fn confirmation_is_bound_to_plan_id_and_digest() {
        let preview = diff(Vec::new(), vec![entry("old.txt", fp(5, 5))]);
        let plan = WorkspaceSyncPlan::build(
            &preview,
            SyncPolicy {
                mode: SyncMode::Mirror,
                ..SyncPolicy::default()
            },
        );
        let first = SyncPlanValidator::freeze(&plan, &preview, &default_registry()).unwrap();
        let second = SyncPlanValidator::freeze(&plan, &preview, &default_registry()).unwrap();

        assert_ne!(first.id(), second.id());
        assert_eq!(first.digest(), second.digest());

        let stale = SyncConfirmationToken::from_explicit_confirmation(&first);
        assert!(matches!(
            ExecutableSyncPlan::new(second, Some(stale)),
            Err(SyncExecutionGateError::StaleConfirmation)
        ));
    }

    #[test]
    fn non_destructive_update_needs_no_confirmation() {
        let preview = diff(vec![entry("new.txt", fp(5, 5))], Vec::new());
        let plan = WorkspaceSyncPlan::build(&preview, SyncPolicy::default());
        let frozen = SyncPlanValidator::freeze(&plan, &preview, &default_registry()).unwrap();

        assert!(ExecutableSyncPlan::new(frozen, None).is_ok());
    }

    #[test]
    fn unsafe_delete_path_is_rejected() {
        let preview = diff(Vec::new(), vec![entry("../escape", fp(5, 5))]);
        let plan = WorkspaceSyncPlan::build(
            &preview,
            SyncPolicy {
                mode: SyncMode::Mirror,
                ..SyncPolicy::default()
            },
        );

        assert!(matches!(
            SyncPlanValidator::freeze(&plan, &preview, &default_registry()),
            Err(SyncValidationError::UnsafeDelete { .. })
        ));
    }

    #[test]
    fn sftp_mirror_passes_validator_now_that_delete_capability_exists() {
        // SFTP_CAPABILITIES now includes Delete, so the validator accepts SFTP
        // deletes. The compiler still requires Local (`require_local_file_mutation`).
        let preview = WorkspaceDiff::compare(
            local("/left"),
            sftp("prod", "/srv/app"),
            Vec::<WorkspaceEntry>::new(),
            vec![entry("old.txt", fp(5, 5))],
        );
        let plan = WorkspaceSyncPlan::build(
            &preview,
            SyncPolicy {
                mode: SyncMode::Mirror,
                ..SyncPolicy::default()
            },
        );

        assert!(SyncPlanValidator::freeze(&plan, &preview, &default_registry()).is_ok());
    }

    #[test]
    fn empty_plan_is_not_executable() {
        let preview = diff(Vec::new(), Vec::new());
        let plan = WorkspaceSyncPlan::build(&preview, SyncPolicy::default());

        assert_eq!(
            SyncPlanValidator::freeze(&plan, &preview, &default_registry()),
            Err(SyncValidationError::EmptyPlan)
        );
    }

    fn s3(target: &str, bucket: Option<&str>, prefix: &str) -> Location {
        Location::S3 {
            target: target.into(),
            bucket: bucket.map(|b| b.into()),
            prefix: prefix.into(),
        }
    }

    fn s3_digest(target: &str, bucket: Option<&str>, prefix: &str) -> PlanDigest {
        let mut b = DigestBuilder::new();
        b.location(&s3(target, bucket, prefix));
        b.finish()
    }

    #[test]
    fn s3_location_digests_are_distinct() {
        let a = s3_digest("aws", None, "");
        let b = s3_digest("aws", Some("bucket"), "");
        let c = s3_digest("aws", Some("bucket"), "foo");
        let d = s3_digest("aws", Some("bucket"), "foo//bar");
        let e = s3_digest("prod", Some("bucket"), "foo");
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(c, d);
        assert_ne!(d, e);
        assert_ne!(a, e);
    }

    #[test]
    fn s3_none_bucket_distinct_from_empty_bucket() {
        let none = s3_digest("aws", None, "x");
        let some_empty = s3_digest("aws", Some(""), "x");
        assert_ne!(none, some_empty);
    }

    #[test]
    fn s3_double_slash_prefix_not_normalized() {
        let dbl = s3_digest("aws", Some("bucket"), "foo//bar");
        let single = s3_digest("aws", Some("bucket"), "foo/bar");
        assert_ne!(dbl, single);
    }
}
