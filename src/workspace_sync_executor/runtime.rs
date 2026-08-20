use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::mpsc;

use crate::journal::OperationJournal;
use crate::services::MutationService;
use crate::transfer::executor::{TransferExecutionError, execute_transfer};
use crate::vfs::{Location, ProviderRegistry};
use crate::workspace_sync::WorkspaceFingerprint;
use crate::workspace_sync_execution::SyncPlanId;
use crate::workspace_sync_journal::{
    SyncJournalError, SyncJournalExecutionMetadata, SyncJournalSession,
};

use super::{CompiledSyncPlan, CompiledSyncStep, PhysicalStepId, PhysicalSyncStep};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedSyncStep {
    pub id: PhysicalStepId,
    pub relative_path: String,
    pub transferred_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncJournalFinalization {
    Recorded,
    Failed { error: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncExecutionError {
    StaleStep {
        path: String,
        expected: Option<Box<WorkspaceFingerprint>>,
        actual: Option<Box<WorkspaceFingerprint>>,
    },
    Transfer {
        path: String,
        error: String,
    },
    Mutation {
        path: String,
        error: String,
    },
}

impl std::fmt::Display for SyncExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StaleStep { path, .. } => write!(f, "sync step became stale: {path}"),
            Self::Transfer { path, error } => write!(f, "transfer failed for {path}: {error}"),
            Self::Mutation { path, error } => write!(f, "mutation failed for {path}: {error}"),
        }
    }
}

impl std::error::Error for SyncExecutionError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncTerminalState {
    Completed,
    Cancelled {
        completed_steps: usize,
    },
    Failed {
        step: PhysicalStepId,
        error: SyncExecutionError,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncExecutionOutcome {
    pub plan_id: SyncPlanId,
    pub completed: Vec<CompletedSyncStep>,
    pub terminal: SyncTerminalState,
    pub remaining: Vec<CompiledSyncStep>,
    pub transferred_bytes: u64,
    /// Conservative physical truth: once a validated step enters its mutation
    /// adapter, the workspace may have changed even if that step does not
    /// reach `CompletedSyncStep`.
    pub workspace_may_have_changed: bool,
    /// Durable audit finalization is separate from physical execution truth.
    pub journal: SyncJournalFinalization,
}

impl SyncExecutionOutcome {
    /// Whether a terminal physical outcome requires a fresh workspace rescan.
    /// Completed always verifies; failed/cancelled runs verify only after a
    /// mutation adapter may have touched the workspace.
    pub fn needs_verification(&self) -> bool {
        match self.terminal {
            SyncTerminalState::Completed => true,
            SyncTerminalState::Cancelled { .. } | SyncTerminalState::Failed { .. } => {
                self.workspace_may_have_changed
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncExecutionEvent {
    Started {
        plan_id: SyncPlanId,
        steps: usize,
    },
    StepStarted {
        id: PhysicalStepId,
        path: String,
    },
    Progress {
        completed_steps: usize,
        total_steps: usize,
        transferred_bytes: u64,
        total_bytes: u64,
    },
    StepCompleted {
        id: PhysicalStepId,
        path: String,
    },
    Cancelled {
        completed_steps: usize,
    },
    Failed {
        step: PhysicalStepId,
        path: String,
        error: String,
    },
    Completed {
        steps: usize,
        transferred_bytes: u64,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum SyncRunError {
    #[error(transparent)]
    Journal(#[from] crate::workspace_sync_journal::SyncJournalError),
}

#[derive(Debug, Clone)]
pub struct WorkspaceSyncExecutor {
    registry: ProviderRegistry,
    journal: OperationJournal,
}

impl WorkspaceSyncExecutor {
    pub fn new(registry: ProviderRegistry, journal: OperationJournal) -> Self {
        Self { registry, journal }
    }

    pub async fn execute(
        &self,
        plan: CompiledSyncPlan,
        cancel: Arc<AtomicBool>,
        events: mpsc::UnboundedSender<SyncExecutionEvent>,
    ) -> Result<SyncExecutionOutcome, SyncRunError> {
        let mut journal = SyncJournalSession::begin_with_step_count(
            self.journal.clone(),
            plan.executable(),
            plan.steps().len(),
        )?;
        let _ = events.send(SyncExecutionEvent::Started {
            plan_id: plan.plan_id(),
            steps: plan.steps().len(),
        });
        journal.mark_running()?;

        let mut completed = Vec::new();
        let mut transferred_bytes = 0u64;
        let mut workspace_may_have_changed = false;

        for (index, compiled) in plan.steps().iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Self::finish_cancelled(
                    &plan,
                    index,
                    completed,
                    transferred_bytes,
                    workspace_may_have_changed,
                    &events,
                    &mut journal,
                );
            }

            let _ = events.send(SyncExecutionEvent::StepStarted {
                id: compiled.id,
                path: compiled.step.relative_path().to_string(),
            });
            tokio::task::yield_now().await;

            if let Err(error) = self.validate_step(&compiled.step).await {
                return self.finish_failed(
                    &plan,
                    compiled,
                    index,
                    completed,
                    transferred_bytes,
                    workspace_may_have_changed,
                    error,
                    &events,
                    &mut journal,
                );
            }

            workspace_may_have_changed = true;
            match self.execute_step(&compiled.step, cancel.clone()).await {
                Ok(()) => {}
                Err(StepExecutionResult::Cancelled) => {
                    return Self::finish_cancelled(
                        &plan,
                        index,
                        completed,
                        transferred_bytes,
                        workspace_may_have_changed,
                        &events,
                        &mut journal,
                    );
                }
                Err(StepExecutionResult::Failed(error)) => {
                    return self.finish_failed(
                        &plan,
                        compiled,
                        index,
                        completed,
                        transferred_bytes,
                        workspace_may_have_changed,
                        error,
                        &events,
                        &mut journal,
                    );
                }
            }

            transferred_bytes = transferred_bytes.saturating_add(compiled.step.bytes());
            completed.push(CompletedSyncStep {
                id: compiled.id,
                relative_path: compiled.step.relative_path().to_string(),
                transferred_bytes: compiled.step.bytes(),
            });
            let _ = events.send(SyncExecutionEvent::StepCompleted {
                id: compiled.id,
                path: compiled.step.relative_path().to_string(),
            });
            let _ = events.send(SyncExecutionEvent::Progress {
                completed_steps: completed.len(),
                total_steps: plan.steps().len(),
                transferred_bytes,
                total_bytes: plan.total_bytes(),
            });
            tokio::task::yield_now().await;
        }

        let journal = finalize_journal(
            journal.complete_with_execution(
                completed.len(),
                execution_metadata(completed.len(), None, 0, transferred_bytes),
            ),
            workspace_may_have_changed,
        )?;
        let _ = events.send(SyncExecutionEvent::Completed {
            steps: completed.len(),
            transferred_bytes,
        });
        Ok(SyncExecutionOutcome {
            plan_id: plan.plan_id(),
            completed,
            terminal: SyncTerminalState::Completed,
            remaining: Vec::new(),
            transferred_bytes,
            workspace_may_have_changed,
            journal,
        })
    }

    fn finish_cancelled(
        plan: &CompiledSyncPlan,
        index: usize,
        completed: Vec<CompletedSyncStep>,
        transferred_bytes: u64,
        workspace_may_have_changed: bool,
        events: &mpsc::UnboundedSender<SyncExecutionEvent>,
        journal: &mut SyncJournalSession,
    ) -> Result<SyncExecutionOutcome, SyncRunError> {
        let remaining = plan.steps()[index..].to_vec();
        let journal = finalize_journal(
            journal.cancel_with_execution(
                completed.len(),
                execution_metadata(completed.len(), None, remaining.len(), transferred_bytes),
            ),
            workspace_may_have_changed,
        )?;
        let _ = events.send(SyncExecutionEvent::Cancelled {
            completed_steps: completed.len(),
        });
        Ok(SyncExecutionOutcome {
            plan_id: plan.plan_id(),
            terminal: SyncTerminalState::Cancelled {
                completed_steps: completed.len(),
            },
            completed,
            remaining,
            transferred_bytes,
            workspace_may_have_changed,
            journal,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_failed(
        &self,
        plan: &CompiledSyncPlan,
        compiled: &CompiledSyncStep,
        index: usize,
        completed: Vec<CompletedSyncStep>,
        transferred_bytes: u64,
        workspace_may_have_changed: bool,
        error: SyncExecutionError,
        events: &mpsc::UnboundedSender<SyncExecutionEvent>,
        journal: &mut SyncJournalSession,
    ) -> Result<SyncExecutionOutcome, SyncRunError> {
        let remaining = plan.steps()[index + 1..].to_vec();
        let journal = finalize_journal(
            journal.fail_with_execution(
                completed.len(),
                error.to_string(),
                execution_metadata(
                    completed.len(),
                    Some(compiled.id.0),
                    remaining.len(),
                    transferred_bytes,
                ),
            ),
            workspace_may_have_changed,
        )?;
        let _ = events.send(SyncExecutionEvent::Failed {
            step: compiled.id,
            path: compiled.step.relative_path().to_string(),
            error: error.to_string(),
        });
        Ok(SyncExecutionOutcome {
            plan_id: plan.plan_id(),
            completed,
            terminal: SyncTerminalState::Failed {
                step: compiled.id,
                error,
            },
            remaining,
            transferred_bytes,
            workspace_may_have_changed,
            journal,
        })
    }

    async fn validate_step(&self, step: &PhysicalSyncStep) -> Result<(), SyncExecutionError> {
        match step {
            PhysicalSyncStep::EnsureDirectory {
                relative_path,
                target,
                expected_target,
            } => {
                let actual = self.fingerprint(target, relative_path, false).await?;
                stale_if_changed(relative_path, expected_target.as_ref(), actual.as_ref())
            }
            PhysicalSyncStep::TransferFile {
                relative_path,
                source,
                destination,
                expected_source,
                expected_destination,
                ..
            } => {
                let actual_source = self.fingerprint(source, relative_path, true).await?;
                stale_if_changed(relative_path, Some(expected_source), actual_source.as_ref())?;
                let actual_destination = self.fingerprint(destination, relative_path, true).await?;
                stale_if_changed(
                    relative_path,
                    expected_destination.as_ref(),
                    actual_destination.as_ref(),
                )
            }
            PhysicalSyncStep::DeleteFile {
                relative_path,
                target,
                expected_target,
            }
            | PhysicalSyncStep::RemoveDirectory {
                relative_path,
                target,
                expected_target,
            } => {
                let actual = self.fingerprint(target, relative_path, false).await?;
                stale_if_changed(relative_path, Some(expected_target), actual.as_ref())
            }
        }
    }

    async fn fingerprint(
        &self,
        location: &Location,
        relative_path: &str,
        transfer: bool,
    ) -> Result<Option<WorkspaceFingerprint>, SyncExecutionError> {
        fingerprint_location(&self.registry, location)
            .await
            .map_err(|error| {
                if transfer {
                    SyncExecutionError::Transfer {
                        path: relative_path.to_string(),
                        error: error.to_string(),
                    }
                } else {
                    SyncExecutionError::Mutation {
                        path: relative_path.to_string(),
                        error: error.to_string(),
                    }
                }
            })
    }

    async fn execute_step(
        &self,
        step: &PhysicalSyncStep,
        cancel: Arc<AtomicBool>,
    ) -> Result<(), StepExecutionResult> {
        match step {
            PhysicalSyncStep::EnsureDirectory {
                relative_path,
                target,
                ..
            } => {
                let Location::Local(path) = target else {
                    return Err(failed_mutation(
                        relative_path,
                        "compiled directory creation is not local",
                    ));
                };
                tokio::fs::create_dir(path)
                    .await
                    .map_err(|error| failed_mutation(relative_path, error))
            }
            PhysicalSyncStep::TransferFile {
                relative_path,
                transfer,
                name,
                ..
            } => execute_transfer(
                transfer,
                std::slice::from_ref(name),
                &self.registry,
                cancel,
                crate::transfer_queue::PauseGate::disabled(),
                |_| {},
            )
            .await
            .map(|_| ())
            .map_err(|error| match error {
                TransferExecutionError::Cancelled { .. } => StepExecutionResult::Cancelled,
                other => StepExecutionResult::Failed(SyncExecutionError::Transfer {
                    path: relative_path.clone(),
                    error: other.to_string(),
                }),
            }),
            PhysicalSyncStep::DeleteFile {
                relative_path,
                target,
                ..
            } => {
                let (parent, name) = local_parent_name(target).ok_or_else(|| {
                    failed_mutation(relative_path, "compiled file deletion is not local")
                })?;
                MutationService::trash_local(parent, vec![name], cancel, |_| {})
                    .await
                    .map(|_| ())
                    .map_err(|error| match error {
                        crate::services::MutationError::Cancelled { .. } => {
                            StepExecutionResult::Cancelled
                        }
                        other => failed_mutation(relative_path, other),
                    })
            }
            PhysicalSyncStep::RemoveDirectory {
                relative_path,
                target,
                ..
            } => {
                if cancel.load(Ordering::Relaxed) {
                    return Err(StepExecutionResult::Cancelled);
                }
                let Location::Local(path) = target else {
                    return Err(failed_mutation(
                        relative_path,
                        "compiled directory removal is not local",
                    ));
                };
                tokio::fs::remove_dir(path)
                    .await
                    .map_err(|error| failed_mutation(relative_path, error))
            }
        }
    }
}

enum StepExecutionResult {
    Cancelled,
    Failed(SyncExecutionError),
}

fn failed_mutation(path: &str, error: impl std::fmt::Display) -> StepExecutionResult {
    StepExecutionResult::Failed(SyncExecutionError::Mutation {
        path: path.to_string(),
        error: error.to_string(),
    })
}

fn finalize_journal(
    result: Result<(), SyncJournalError>,
    workspace_may_have_changed: bool,
) -> Result<SyncJournalFinalization, SyncRunError> {
    match result {
        Ok(()) => Ok(SyncJournalFinalization::Recorded),
        Err(error) if workspace_may_have_changed => Ok(SyncJournalFinalization::Failed {
            error: error.to_string(),
        }),
        Err(error) => Err(SyncRunError::Journal(error)),
    }
}

fn execution_metadata(
    completed_steps: usize,
    failed_step: Option<usize>,
    remaining_steps: usize,
    transferred_bytes: u64,
) -> SyncJournalExecutionMetadata {
    SyncJournalExecutionMetadata {
        completed_steps,
        failed_step,
        remaining_steps,
        transferred_bytes,
        rollback_attempted: false,
    }
}

fn stale_if_changed(
    path: &str,
    expected: Option<&WorkspaceFingerprint>,
    actual: Option<&WorkspaceFingerprint>,
) -> Result<(), SyncExecutionError> {
    if expected == actual {
        Ok(())
    } else {
        Err(SyncExecutionError::StaleStep {
            path: path.to_string(),
            expected: expected.cloned().map(Box::new),
            actual: actual.cloned().map(Box::new),
        })
    }
}

async fn fingerprint_location(
    registry: &ProviderRegistry,
    location: &Location,
) -> std::io::Result<Option<WorkspaceFingerprint>> {
    let Some((parent, name)) = location_parent_name(location) else {
        return Ok(None);
    };
    let entries = registry.list_location_async(&parent).await?;
    Ok(entries
        .into_iter()
        .find(|entry| entry.name == name)
        .map(|entry| WorkspaceFingerprint {
            kind: entry.kind,
            size: entry.size,
            modified_unix_ms: entry.modified_unix_ms,
            content_hash: None,
        }))
}

fn location_parent_name(location: &Location) -> Option<(Location, String)> {
    match location {
        Location::Local(path) => Some((
            Location::Local(path.parent()?.to_path_buf()),
            path.file_name()?.to_string_lossy().into_owned(),
        )),
        Location::Sftp { host, path } => {
            let trimmed = path.trim_end_matches('/');
            let (parent, name) = trimmed.rsplit_once('/').unwrap_or(("", trimmed));
            let parent = if parent.is_empty() { "/" } else { parent };
            Some((
                Location::Sftp {
                    host: host.clone(),
                    path: parent.to_string(),
                },
                name.to_string(),
            ))
        }
        Location::Archive { .. } => None,
        // ponytail: S3 Workspace Sync parked; legacy parent/name path invalid for S3
        Location::S3 { .. } => None,
        Location::WebDav { target, path } => {
            let trimmed = path.trim_end_matches('/');
            let (parent, name) = trimmed.rsplit_once('/').unwrap_or(("", trimmed));
            let parent = if parent.is_empty() { "/" } else { parent };
            Some((
                Location::WebDav {
                    target: target.clone(),
                    path: parent.to_string(),
                },
                name.to_string(),
            ))
        }
    }
}

fn local_parent_name(location: &Location) -> Option<(PathBuf, String)> {
    let Location::Local(path) = location else {
        return None;
    };
    Some((
        path.parent()?.to_path_buf(),
        path.file_name()?.to_string_lossy().into_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn location_parent_name_rejects_s3() {
        let loc = Location::S3 {
            target: "aws".into(),
            bucket: Some("bucket".into()),
            prefix: "foo/bar".into(),
        };
        assert_eq!(location_parent_name(&loc), None);
    }
}
