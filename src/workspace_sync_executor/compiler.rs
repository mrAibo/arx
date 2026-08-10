use std::collections::{BTreeMap, BTreeSet};

use crate::transfer::{
    ExecutorAvailability, TransferIntent, TransferPlan, TransferPlanner, TransferRequest,
};
use crate::vfs::{CapabilitySet, EntryKind, Location, ProviderId, ProviderRegistry};
use crate::workspace_sync::{
    WorkspaceDiff, WorkspaceDiffEntry, WorkspaceFingerprint, WorkspaceSide,
};
use crate::workspace_sync_execution::{
    ExecutableSyncPlan, FrozenSyncOperation, FrozenWorkspaceSyncPlan, SyncPlanValidator,
    SyncValidationError,
};

use super::{CompiledSyncPlan, CompiledSyncStep, PhysicalStepId, PhysicalSyncStep};

#[derive(Debug, Clone)]
pub struct SyncExecutorMatrix {
    local: ExecutorAvailability,
    remote: BTreeMap<String, ExecutorAvailability>,
}

impl SyncExecutorMatrix {
    pub fn local_only() -> Self {
        Self {
            local: ExecutorAvailability::local(),
            remote: BTreeMap::new(),
        }
    }

    pub fn with_remote(
        mut self,
        host: impl Into<String>,
        availability: ExecutorAvailability,
    ) -> Self {
        self.remote.insert(host.into(), availability);
        self
    }

    fn for_transfer(
        &self,
        source: &Location,
        destination: &Location,
    ) -> Result<ExecutorAvailability, SyncCompileError> {
        match (source, destination) {
            (Location::Local(_), Location::Local(_)) => Ok(self.local),
            (Location::Local(_), Location::Sftp { host, .. })
            | (Location::Sftp { host, .. }, Location::Local(_)) => {
                self.remote.get(host).copied().ok_or_else(|| {
                    SyncCompileError::MissingExecutorAvailability { host: host.clone() }
                })
            }
            (Location::Sftp { .. }, Location::Sftp { .. }) => {
                Err(SyncCompileError::RemoteToRemoteUnsupported {
                    source_location: source.clone(),
                    destination_location: destination.clone(),
                })
            }
            _ => Err(SyncCompileError::UnsupportedTransferPair {
                source_location: source.clone(),
                destination_location: destination.clone(),
            }),
        }
    }
}

impl Default for SyncExecutorMatrix {
    fn default() -> Self {
        Self::local_only()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SyncCompileError {
    #[error(transparent)]
    Validation(#[from] SyncValidationError),
    #[error("invalid relative sync path: {path}")]
    InvalidRelativePath { path: String },
    #[error("sync plan contains duplicate normalized path: {path}")]
    DuplicateNormalizedPath { path: String },
    #[error("workspace tree contains descendants below non-directory entry: {path}")]
    AncestorTypeConflict { path: String },
    #[error("unsupported sync entry kind at {path}: {kind:?}")]
    UnsupportedEntryKind { path: String, kind: EntryKind },
    #[error(
        "remote-to-remote sync is not implemented: {source_location} -> {destination_location}"
    )]
    RemoteToRemoteUnsupported {
        source_location: Location,
        destination_location: Location,
    },
    #[error("unsupported transfer pair: {source_location} -> {destination_location}")]
    UnsupportedTransferPair {
        source_location: Location,
        destination_location: Location,
    },
    #[error("missing transfer executor availability for SSH host {host}")]
    MissingExecutorAvailability { host: String },
    #[error("provider {provider:?} cannot safely create/remove directories required by {path}")]
    UnsupportedDirectoryMutation { provider: ProviderId, path: String },
    #[error("provider {provider:?} cannot safely delete files required by {path}")]
    UnsupportedFileMutation { provider: ProviderId, path: String },
    #[error("structural replacement at {path} requires explicit confirmation")]
    StructuralReplacementConfirmationRequired { path: String },
    #[error("structural replacement at {path} requires explicit deletion of destination children")]
    StructuralReplacementRequiresDeletes { path: String },
    #[error("sync transfer planning failed for {path}: {error}")]
    TransferPlan { path: String, error: String },
    #[error("sync preview no longer contains {path}")]
    MissingPreviewEntry { path: String },
    #[error("compiled sync plan contains no physical steps")]
    EmptyCompiledPlan,
}

pub struct SyncExecutionCompiler;

impl SyncExecutionCompiler {
    pub fn compile(
        executable: ExecutableSyncPlan,
        current: &WorkspaceDiff,
        registry: &ProviderRegistry,
        executors: &SyncExecutorMatrix,
    ) -> Result<CompiledSyncPlan, SyncCompileError> {
        SyncPlanValidator::validate_frozen(executable.plan(), current)?;
        validate_tree_shape(current)?;

        let normalized_paths = validate_operation_paths(executable.plan().operations())?;
        let entries = index_entries(current);
        let delete_paths = executable
            .plan()
            .operations()
            .iter()
            .filter_map(|operation| match operation {
                FrozenSyncOperation::Delete { relative_path, .. } => {
                    Some(normalize_execution_path(relative_path))
                }
                _ => None,
            })
            .collect::<Result<BTreeSet<_>, _>>()?;

        let mut blocking_delete_paths = BTreeSet::new();
        let mut staged = Vec::new();

        for (operation, path) in executable
            .plan()
            .operations()
            .iter()
            .zip(normalized_paths.iter())
        {
            if let FrozenSyncOperation::Copy {
                from,
                to,
                expected_source,
                expected_destination,
                ..
            } = operation
            {
                compile_copy(
                    &executable,
                    current,
                    registry,
                    executors,
                    &entries,
                    &delete_paths,
                    &mut blocking_delete_paths,
                    &mut staged,
                    path,
                    *from,
                    *to,
                    expected_source,
                    expected_destination.as_ref(),
                )?;
            }
        }

        for (operation, path) in executable
            .plan()
            .operations()
            .iter()
            .zip(normalized_paths.iter())
        {
            if let FrozenSyncOperation::Delete {
                from,
                expected_target,
                ..
            } = operation
            {
                compile_delete(
                    executable.plan(),
                    &mut staged,
                    path,
                    *from,
                    expected_target,
                    blocking_delete_paths.contains(path),
                )?;
            }
        }

        staged.sort_by(|left, right| {
            left.phase
                .cmp(&right.phase)
                .then_with(|| match left.phase {
                    StepPhase::BlockingDirectoryRemove | StepPhase::DirectoryRemove => {
                        right.depth.cmp(&left.depth)
                    }
                    _ => left.depth.cmp(&right.depth),
                })
                .then_with(|| left.path.cmp(&right.path))
        });

        if staged.is_empty() {
            return Err(SyncCompileError::EmptyCompiledPlan);
        }

        let steps = staged
            .into_iter()
            .enumerate()
            .map(|(index, staged)| CompiledSyncStep {
                id: PhysicalStepId(index + 1),
                step: staged.step,
            })
            .collect::<Vec<_>>();
        let destructive_steps = steps
            .iter()
            .filter(|step| step.step.is_destructive())
            .count();
        let total_bytes = steps.iter().map(|step| step.step.bytes()).sum();

        if destructive_steps > 0 && executable.confirmation().is_none() {
            let path = steps
                .iter()
                .find(|step| step.step.is_destructive())
                .map(|step| step.step.relative_path().to_string())
                .unwrap_or_default();
            return Err(SyncCompileError::StructuralReplacementConfirmationRequired { path });
        }

        Ok(CompiledSyncPlan::new(
            executable,
            steps,
            total_bytes,
            destructive_steps,
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum StepPhase {
    BlockingFileDelete,
    BlockingDirectoryRemove,
    EnsureDirectory,
    Transfer,
    FileDelete,
    DirectoryRemove,
}

struct StagedStep {
    phase: StepPhase,
    depth: usize,
    path: String,
    step: PhysicalSyncStep,
}

#[allow(clippy::too_many_arguments)]
fn compile_copy(
    executable: &ExecutableSyncPlan,
    current: &WorkspaceDiff,
    registry: &ProviderRegistry,
    executors: &SyncExecutorMatrix,
    entries: &BTreeMap<&str, &WorkspaceDiffEntry>,
    delete_paths: &BTreeSet<String>,
    blocking_delete_paths: &mut BTreeSet<String>,
    staged: &mut Vec<StagedStep>,
    path: &str,
    from: WorkspaceSide,
    to: WorkspaceSide,
    expected_source: &WorkspaceFingerprint,
    expected_destination: Option<&WorkspaceFingerprint>,
) -> Result<(), SyncCompileError> {
    match expected_source.kind {
        EntryKind::Symlink | EntryKind::Other => Err(SyncCompileError::UnsupportedEntryKind {
            path: path.to_string(),
            kind: expected_source.kind,
        }),
        EntryKind::Directory => {
            compile_directory_copy(executable, staged, path, to, expected_destination)
        }
        EntryKind::File => compile_file_copy(
            executable,
            current,
            registry,
            executors,
            entries,
            delete_paths,
            blocking_delete_paths,
            staged,
            path,
            from,
            to,
            expected_source,
            expected_destination,
        ),
    }
}

fn compile_directory_copy(
    executable: &ExecutableSyncPlan,
    staged: &mut Vec<StagedStep>,
    path: &str,
    to: WorkspaceSide,
    expected_destination: Option<&WorkspaceFingerprint>,
) -> Result<(), SyncCompileError> {
    let target = resolve_relative(root_for_side(executable.plan(), to), path);
    if let Some(destination) = expected_destination {
        return match destination.kind {
            EntryKind::Directory => Ok(()),
            EntryKind::File => {
                require_structural_confirmation(executable, path)?;
                require_local_file_mutation(&target, path)?;
                staged.push(staged_step(
                    StepPhase::BlockingFileDelete,
                    path,
                    PhysicalSyncStep::DeleteFile {
                        relative_path: path.to_string(),
                        target: target.clone(),
                        expected_target: destination.clone(),
                    },
                ));
                staged.push(staged_step(
                    StepPhase::EnsureDirectory,
                    path,
                    PhysicalSyncStep::EnsureDirectory {
                        relative_path: path.to_string(),
                        target,
                        expected_target: None,
                    },
                ));
                Ok(())
            }
            kind => Err(SyncCompileError::UnsupportedEntryKind {
                path: path.to_string(),
                kind,
            }),
        };
    }

    require_local_directory_mutation(&target, path)?;
    staged.push(staged_step(
        StepPhase::EnsureDirectory,
        path,
        PhysicalSyncStep::EnsureDirectory {
            relative_path: path.to_string(),
            target,
            expected_target: None,
        },
    ));
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn compile_file_copy(
    executable: &ExecutableSyncPlan,
    current: &WorkspaceDiff,
    registry: &ProviderRegistry,
    executors: &SyncExecutorMatrix,
    entries: &BTreeMap<&str, &WorkspaceDiffEntry>,
    delete_paths: &BTreeSet<String>,
    blocking_delete_paths: &mut BTreeSet<String>,
    staged: &mut Vec<StagedStep>,
    path: &str,
    from: WorkspaceSide,
    to: WorkspaceSide,
    expected_source: &WorkspaceFingerprint,
    expected_destination: Option<&WorkspaceFingerprint>,
) -> Result<(), SyncCompileError> {
    let mut physical_expected_destination = expected_destination.cloned();
    if let Some(destination) = expected_destination {
        match destination.kind {
            EntryKind::File => {}
            EntryKind::Directory => {
                require_structural_confirmation(executable, path)?;
                let prefix = format!("{path}/");
                let descendants = current
                    .entries
                    .iter()
                    .filter(|entry| entry.relative_path.starts_with(&prefix))
                    .filter(|entry| fingerprint_for(entry, to).is_some())
                    .map(|entry| entry.relative_path.clone())
                    .collect::<Vec<_>>();
                if descendants
                    .iter()
                    .any(|descendant| !delete_paths.contains(descendant))
                {
                    return Err(SyncCompileError::StructuralReplacementRequiresDeletes {
                        path: path.to_string(),
                    });
                }
                blocking_delete_paths.extend(descendants);
                let target = resolve_relative(root_for_side(executable.plan(), to), path);
                require_local_directory_mutation(&target, path)?;
                staged.push(staged_step(
                    StepPhase::BlockingDirectoryRemove,
                    path,
                    PhysicalSyncStep::RemoveDirectory {
                        relative_path: path.to_string(),
                        target,
                        expected_target: destination.clone(),
                    },
                ));
                physical_expected_destination = None;
            }
            kind => {
                return Err(SyncCompileError::UnsupportedEntryKind {
                    path: path.to_string(),
                    kind,
                });
            }
        }
    }

    if !entries.contains_key(path) {
        return Err(SyncCompileError::MissingPreviewEntry {
            path: path.to_string(),
        });
    }

    let source_root = root_for_side(executable.plan(), from);
    let destination_root = root_for_side(executable.plan(), to);
    let source = resolve_relative(source_root, path);
    let destination = resolve_relative(destination_root, path);
    let (parent, name) = split_parent_name(path)?;
    let transfer_source = resolve_parent(source_root, &parent);
    let transfer_destination = resolve_parent(destination_root, &parent);
    let transfer = compile_transfer_plan(
        registry,
        executors,
        &transfer_source,
        &transfer_destination,
        path,
    )?;

    staged.push(staged_step(
        StepPhase::Transfer,
        path,
        PhysicalSyncStep::TransferFile {
            relative_path: path.to_string(),
            source,
            destination,
            expected_source: expected_source.clone(),
            expected_destination: physical_expected_destination,
            transfer: Box::new(transfer),
            name,
            bytes: expected_source.size.unwrap_or(0),
        },
    ));
    Ok(())
}

fn compile_delete(
    plan: &FrozenWorkspaceSyncPlan,
    staged: &mut Vec<StagedStep>,
    path: &str,
    from: WorkspaceSide,
    expected_target: &WorkspaceFingerprint,
    blocking: bool,
) -> Result<(), SyncCompileError> {
    let target = resolve_relative(root_for_side(plan, from), path);
    match expected_target.kind {
        EntryKind::File => {
            require_local_file_mutation(&target, path)?;
            staged.push(staged_step(
                if blocking {
                    StepPhase::BlockingFileDelete
                } else {
                    StepPhase::FileDelete
                },
                path,
                PhysicalSyncStep::DeleteFile {
                    relative_path: path.to_string(),
                    target,
                    expected_target: expected_target.clone(),
                },
            ));
            Ok(())
        }
        EntryKind::Directory => {
            require_local_directory_mutation(&target, path)?;
            staged.push(staged_step(
                if blocking {
                    StepPhase::BlockingDirectoryRemove
                } else {
                    StepPhase::DirectoryRemove
                },
                path,
                PhysicalSyncStep::RemoveDirectory {
                    relative_path: path.to_string(),
                    target,
                    expected_target: expected_target.clone(),
                },
            ));
            Ok(())
        }
        kind => Err(SyncCompileError::UnsupportedEntryKind {
            path: path.to_string(),
            kind,
        }),
    }
}

fn compile_transfer_plan(
    registry: &ProviderRegistry,
    executors: &SyncExecutorMatrix,
    source: &Location,
    destination: &Location,
    path: &str,
) -> Result<TransferPlan, SyncCompileError> {
    let source_provider = source.provider_id();
    let destination_provider = destination.provider_id();
    let availability = executors.for_transfer(source, destination)?;
    let source_capabilities = registry
        .capabilities(&source_provider)
        .unwrap_or(CapabilitySet::NONE);
    let destination_capabilities = registry
        .capabilities(&destination_provider)
        .unwrap_or(CapabilitySet::NONE);

    TransferPlanner::plan(TransferRequest {
        source: source.clone(),
        destination: destination.clone(),
        source_provider,
        destination_provider,
        source_capabilities,
        destination_capabilities,
        intent: TransferIntent::Copy,
        executors: availability,
        delete_extraneous: false,
    })
    .map_err(|error| SyncCompileError::TransferPlan {
        path: path.to_string(),
        error: error.to_string(),
    })
}

fn require_structural_confirmation(
    executable: &ExecutableSyncPlan,
    path: &str,
) -> Result<(), SyncCompileError> {
    if executable.confirmation().is_some() {
        Ok(())
    } else {
        Err(
            SyncCompileError::StructuralReplacementConfirmationRequired {
                path: path.to_string(),
            },
        )
    }
}

fn require_local_directory_mutation(target: &Location, path: &str) -> Result<(), SyncCompileError> {
    if target.provider_id() == ProviderId::Local {
        Ok(())
    } else {
        Err(SyncCompileError::UnsupportedDirectoryMutation {
            provider: target.provider_id(),
            path: path.to_string(),
        })
    }
}

fn require_local_file_mutation(target: &Location, path: &str) -> Result<(), SyncCompileError> {
    if target.provider_id() == ProviderId::Local {
        Ok(())
    } else {
        Err(SyncCompileError::UnsupportedFileMutation {
            provider: target.provider_id(),
            path: path.to_string(),
        })
    }
}

fn staged_step(phase: StepPhase, path: &str, step: PhysicalSyncStep) -> StagedStep {
    StagedStep {
        phase,
        depth: path.split('/').count(),
        path: path.to_string(),
        step,
    }
}

fn validate_operation_paths(
    operations: &[FrozenSyncOperation],
) -> Result<Vec<String>, SyncCompileError> {
    let mut seen = BTreeSet::new();
    let mut normalized = Vec::with_capacity(operations.len());
    for operation in operations {
        let path = normalize_execution_path(operation.relative_path())?;
        if !seen.insert(path.clone()) {
            return Err(SyncCompileError::DuplicateNormalizedPath { path });
        }
        normalized.push(path);
    }
    Ok(normalized)
}

fn normalize_execution_path(path: &str) -> Result<String, SyncCompileError> {
    if path.is_empty() || path.starts_with('/') || path.starts_with('\\') || path.contains('\0') {
        return Err(SyncCompileError::InvalidRelativePath {
            path: path.to_string(),
        });
    }

    let normalized = path.replace('\\', "/");
    let mut components = Vec::new();
    for component in normalized.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return Err(SyncCompileError::InvalidRelativePath {
                path: path.to_string(),
            });
        }
        components.push(component);
    }
    Ok(components.join("/"))
}

fn validate_tree_shape(diff: &WorkspaceDiff) -> Result<(), SyncCompileError> {
    validate_side_tree(diff, WorkspaceSide::Left)?;
    validate_side_tree(diff, WorkspaceSide::Right)
}

fn validate_side_tree(diff: &WorkspaceDiff, side: WorkspaceSide) -> Result<(), SyncCompileError> {
    let mut non_directories = BTreeSet::new();
    let mut paths = Vec::new();
    for entry in &diff.entries {
        if let Some(fingerprint) = fingerprint_for(entry, side) {
            let path = normalize_execution_path(&entry.relative_path)?;
            if fingerprint.kind != EntryKind::Directory {
                non_directories.insert(path.clone());
            }
            paths.push(path);
        }
    }

    for path in paths {
        let components = path.split('/').collect::<Vec<_>>();
        let mut ancestor = String::new();
        for component in components.iter().take(components.len().saturating_sub(1)) {
            if !ancestor.is_empty() {
                ancestor.push('/');
            }
            ancestor.push_str(component);
            if non_directories.contains(&ancestor) {
                return Err(SyncCompileError::AncestorTypeConflict { path: ancestor });
            }
        }
    }
    Ok(())
}

fn index_entries(diff: &WorkspaceDiff) -> BTreeMap<&str, &WorkspaceDiffEntry> {
    diff.entries
        .iter()
        .map(|entry| (entry.relative_path.as_str(), entry))
        .collect()
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

fn root_for_side(plan: &FrozenWorkspaceSyncPlan, side: WorkspaceSide) -> &Location {
    match side {
        WorkspaceSide::Left => plan.left_root(),
        WorkspaceSide::Right => plan.right_root(),
    }
}

fn resolve_relative(root: &Location, path: &str) -> Location {
    path.split('/').fold(root.clone(), |location, component| {
        location.child(component)
    })
}

fn resolve_parent(root: &Location, parent: &str) -> Location {
    if parent.is_empty() {
        root.clone()
    } else {
        resolve_relative(root, parent)
    }
}

fn split_parent_name(path: &str) -> Result<(String, String), SyncCompileError> {
    let normalized = normalize_execution_path(path)?;
    Ok(match normalized.rsplit_once('/') {
        Some((parent, name)) => (parent.to_string(), name.to_string()),
        None => (String::new(), normalized),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp() -> WorkspaceFingerprint {
        WorkspaceFingerprint {
            kind: EntryKind::File,
            size: Some(1),
            modified_unix_ms: None,
            content_hash: None,
        }
    }

    #[test]
    fn rejects_path_escape_and_absolute_paths() {
        assert!(matches!(
            normalize_execution_path("../etc/passwd"),
            Err(SyncCompileError::InvalidRelativePath { .. })
        ));
        assert!(matches!(
            normalize_execution_path("/etc/passwd"),
            Err(SyncCompileError::InvalidRelativePath { .. })
        ));
    }

    #[test]
    fn rejects_normalized_duplicate_paths() {
        let operations = vec![
            FrozenSyncOperation::Copy {
                relative_path: "a\\b".into(),
                from: WorkspaceSide::Left,
                to: WorkspaceSide::Right,
                expected_source: fp(),
                expected_destination: None,
            },
            FrozenSyncOperation::Copy {
                relative_path: "a/b".into(),
                from: WorkspaceSide::Left,
                to: WorkspaceSide::Right,
                expected_source: fp(),
                expected_destination: None,
            },
        ];
        assert!(matches!(
            validate_operation_paths(&operations),
            Err(SyncCompileError::DuplicateNormalizedPath { .. })
        ));
    }

    #[test]
    fn remote_to_remote_is_explicitly_gated() {
        let source = Location::Sftp {
            host: "a".into(),
            path: "/src".into(),
        };
        let destination = Location::Sftp {
            host: "b".into(),
            path: "/dst".into(),
        };
        assert!(matches!(
            SyncExecutorMatrix::default().for_transfer(&source, &destination),
            Err(SyncCompileError::RemoteToRemoteUnsupported { .. })
        ));
    }

    // ── REMOTE-09: SFTP delete never reaches sync execution ──

    #[test]
    fn sftp_delete_never_reaches_sync_execution() {
        // require_local_file_mutation() rejects any non-Local provider.
        // This is the exact gate that prevents SFTP file mutations from
        // being compiled into executable sync steps. The validator (in
        // workspace_sync_execution.rs) now passes SFTP deletes because
        // SFTP_CAPABILITIES includes Delete — the compiler is the
        // definitive blocker.
        let sftp_target = Location::Sftp {
            host: "prod".into(),
            path: "/srv/data.txt".into(),
        };
        let result = require_local_file_mutation(&sftp_target, "data.txt");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Sftp") || msg.contains("cannot safely delete"));
    }
}
