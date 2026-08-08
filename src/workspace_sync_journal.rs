use std::io;

use serde::{Deserialize, Serialize};

use crate::journal::{
    OperationId, OperationJournal, OperationKind, OperationRecord, OperationState,
};
use crate::workspace_sync::{SyncDirection, SyncMode};
use crate::workspace_sync_execution::{ExecutableSyncPlan, FrozenWorkspaceSyncPlan};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SyncJournalExecutionMetadata {
    pub completed_steps: usize,
    pub failed_step: Option<usize>,
    pub remaining_steps: usize,
    pub transferred_bytes: u64,
    pub rollback_attempted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncJournalMetadata {
    pub plan_id: u64,
    pub plan_digest: String,
    pub left_root: String,
    pub right_root: String,
    pub direction: String,
    pub mode: String,
    pub operations: usize,
    pub bytes: u64,
    pub destructive_operations: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub physical_steps: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<SyncJournalExecutionMetadata>,
}

impl SyncJournalMetadata {
    pub fn from_plan(plan: &FrozenWorkspaceSyncPlan) -> Self {
        Self {
            plan_id: plan.id().get(),
            plan_digest: plan.digest().as_hex(),
            left_root: plan.left_root().to_string(),
            right_root: plan.right_root().to_string(),
            direction: direction_name(plan.direction()).into(),
            mode: mode_name(plan.mode()).into(),
            operations: plan.operations().len(),
            bytes: plan.total_bytes(),
            destructive_operations: plan.destructive_operations(),
            physical_steps: None,
            execution: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SyncJournalError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("failed to serialize sync journal metadata: {0}")]
    Metadata(#[from] serde_json::Error),
    #[error("invalid sync journal transition: {from:?} -> {to:?}")]
    InvalidTransition {
        from: OperationState,
        to: OperationState,
    },
}

/// Append-only lifecycle writer for one executable sync plan.
///
/// Creating a session records `Started`. The executor records `Running` when
/// execution enters the physical-step loop, then exactly one terminal state.
/// All rows keep the same `OperationId` and frozen plan facts. Terminal rows
/// may additionally attach structured physical execution facts.
#[derive(Debug)]
pub struct SyncJournalSession {
    journal: OperationJournal,
    current: OperationRecord,
}

impl SyncJournalSession {
    pub fn begin(
        journal: OperationJournal,
        executable: &ExecutableSyncPlan,
    ) -> Result<Self, SyncJournalError> {
        Self::begin_with_step_count(journal, executable, executable.plan().operations().len())
    }

    pub fn begin_with_step_count(
        journal: OperationJournal,
        executable: &ExecutableSyncPlan,
        physical_steps: usize,
    ) -> Result<Self, SyncJournalError> {
        let plan = executable.plan();
        let mut metadata = SyncJournalMetadata::from_plan(plan);
        metadata.physical_steps = Some(physical_steps);
        let (source, destination) = source_destination(plan);
        let mut record = OperationRecord::new(OperationKind::Synchronize);
        record.source = Some(source);
        record.destination = Some(destination);
        record.item_count = Some(physical_steps);
        record.metadata = Some(serde_json::to_value(metadata)?);
        journal.append(&record)?;

        Ok(Self {
            journal,
            current: record,
        })
    }

    pub fn operation_id(&self) -> &OperationId {
        &self.current.id
    }

    pub fn state(&self) -> OperationState {
        self.current.state
    }

    pub fn current_record(&self) -> &OperationRecord {
        &self.current
    }

    pub fn mark_running(&mut self) -> Result<(), SyncJournalError> {
        self.append_transition(OperationState::Running, None, None)
    }

    pub fn complete(&mut self, completed_operations: usize) -> Result<(), SyncJournalError> {
        let total = self.current.item_count.unwrap_or(0);
        self.append_transition(
            OperationState::Completed,
            Some(format!(
                "completed {completed_operations} of {total} sync operation(s)"
            )),
            None,
        )
    }

    pub fn complete_with_execution(
        &mut self,
        completed_steps: usize,
        execution: SyncJournalExecutionMetadata,
    ) -> Result<(), SyncJournalError> {
        let total = self.current.item_count.unwrap_or(0);
        self.append_transition(
            OperationState::Completed,
            Some(format!(
                "completed {completed_steps} of {total} sync step(s)"
            )),
            Some(execution),
        )
    }

    pub fn cancel(&mut self, completed_operations: usize) -> Result<(), SyncJournalError> {
        let total = self.current.item_count.unwrap_or(0);
        self.append_transition(
            OperationState::Cancelled,
            Some(format!(
                "cancelled after {completed_operations} of {total} sync operation(s)"
            )),
            None,
        )
    }

    pub fn cancel_with_execution(
        &mut self,
        completed_steps: usize,
        execution: SyncJournalExecutionMetadata,
    ) -> Result<(), SyncJournalError> {
        let total = self.current.item_count.unwrap_or(0);
        self.append_transition(
            OperationState::Cancelled,
            Some(format!(
                "cancelled after {completed_steps} of {total} sync step(s)"
            )),
            Some(execution),
        )
    }

    pub fn fail(
        &mut self,
        completed_operations: usize,
        error: impl Into<String>,
    ) -> Result<(), SyncJournalError> {
        let total = self.current.item_count.unwrap_or(0);
        self.append_transition(
            OperationState::Failed,
            Some(format!(
                "failed after {completed_operations} of {total} sync operation(s): {}",
                error.into()
            )),
            None,
        )
    }

    pub fn fail_with_execution(
        &mut self,
        completed_steps: usize,
        error: impl Into<String>,
        execution: SyncJournalExecutionMetadata,
    ) -> Result<(), SyncJournalError> {
        let total = self.current.item_count.unwrap_or(0);
        self.append_transition(
            OperationState::Failed,
            Some(format!(
                "failed after {completed_steps} of {total} sync step(s): {}",
                error.into()
            )),
            Some(execution),
        )
    }

    fn append_transition(
        &mut self,
        state: OperationState,
        message: Option<String>,
        execution: Option<SyncJournalExecutionMetadata>,
    ) -> Result<(), SyncJournalError> {
        if !valid_transition(self.current.state, state) {
            return Err(SyncJournalError::InvalidTransition {
                from: self.current.state,
                to: state,
            });
        }

        let mut next = self.current.transition(state, message);
        if let Some(execution) = execution {
            attach_execution_metadata(&mut next, execution)?;
        }
        self.journal.append(&next)?;
        self.current = next;
        Ok(())
    }
}

fn attach_execution_metadata(
    record: &mut OperationRecord,
    execution: SyncJournalExecutionMetadata,
) -> Result<(), SyncJournalError> {
    let mut metadata: SyncJournalMetadata = serde_json::from_value(
        record
            .metadata
            .clone()
            .unwrap_or_else(|| serde_json::json!({})),
    )?;
    metadata.execution = Some(execution);
    record.metadata = Some(serde_json::to_value(metadata)?);
    Ok(())
}

fn valid_transition(from: OperationState, to: OperationState) -> bool {
    matches!(
        (from, to),
        (OperationState::Started, OperationState::Running)
            | (OperationState::Started, OperationState::Failed)
            | (OperationState::Started, OperationState::Cancelled)
            | (OperationState::Running, OperationState::Completed)
            | (OperationState::Running, OperationState::Failed)
            | (OperationState::Running, OperationState::Cancelled)
    )
}

fn source_destination(plan: &FrozenWorkspaceSyncPlan) -> (String, String) {
    match plan.direction() {
        SyncDirection::LeftToRight => (plan.left_root().to_string(), plan.right_root().to_string()),
        SyncDirection::RightToLeft => (plan.right_root().to_string(), plan.left_root().to_string()),
    }
}

fn direction_name(direction: SyncDirection) -> &'static str {
    match direction {
        SyncDirection::LeftToRight => "left_to_right",
        SyncDirection::RightToLeft => "right_to_left",
    }
}

fn mode_name(mode: SyncMode) -> &'static str {
    match mode {
        SyncMode::Update => "update",
        SyncMode::Mirror => "mirror",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::{EntryKind, Location, default_registry};
    use crate::workspace_sync::{
        SyncPolicy, WorkspaceDiff, WorkspaceEntry, WorkspaceFingerprint, WorkspaceSyncPlan,
    };
    use crate::workspace_sync_execution::{ExecutableSyncPlan, SyncPlanValidator};
    use std::path::PathBuf;

    fn local(path: &str) -> Location {
        Location::Local(PathBuf::from(path))
    }

    fn entry(path: &str, size: u64) -> WorkspaceEntry {
        WorkspaceEntry {
            relative_path: path.into(),
            fingerprint: WorkspaceFingerprint {
                kind: EntryKind::File,
                size: Some(size),
                modified_unix_ms: Some(1),
                content_hash: None,
            },
        }
    }

    fn executable(direction: SyncDirection) -> ExecutableSyncPlan {
        let (left, right) = match direction {
            SyncDirection::LeftToRight => (vec![entry("a.txt", 7)], Vec::new()),
            SyncDirection::RightToLeft => (Vec::new(), vec![entry("a.txt", 7)]),
        };
        let diff = WorkspaceDiff::compare(local("/left"), local("/right"), left, right);
        let plan = WorkspaceSyncPlan::build(
            &diff,
            SyncPolicy {
                direction,
                ..SyncPolicy::default()
            },
        );
        let frozen = SyncPlanValidator::freeze(&plan, &diff, &default_registry()).unwrap();
        ExecutableSyncPlan::new(frozen, None).unwrap()
    }

    #[test]
    fn begin_records_frozen_plan_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let journal = OperationJournal::open(dir.path().join("ops.jsonl")).unwrap();
        let executable = executable(SyncDirection::LeftToRight);
        let expected_id = executable.plan().id().get();
        let expected_digest = executable.plan().digest().as_hex();

        let session = SyncJournalSession::begin(journal.clone(), &executable).unwrap();
        let records = journal.read_all().unwrap();
        let metadata: SyncJournalMetadata =
            serde_json::from_value(records[0].metadata.clone().unwrap()).unwrap();

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].kind, OperationKind::Synchronize);
        assert_eq!(records[0].state, OperationState::Started);
        assert_eq!(records[0].source.as_deref(), Some("file:///left"));
        assert_eq!(records[0].destination.as_deref(), Some("file:///right"));
        assert_eq!(records[0].item_count, Some(1));
        assert_eq!(metadata.plan_id, expected_id);
        assert_eq!(metadata.plan_digest, expected_digest);
        assert_eq!(metadata.direction, "left_to_right");
        assert_eq!(metadata.mode, "update");
        assert_eq!(metadata.operations, 1);
        assert_eq!(metadata.physical_steps, Some(1));
        assert_eq!(metadata.bytes, 7);
        assert_eq!(session.state(), OperationState::Started);
    }

    #[test]
    fn lifecycle_appends_same_operation_id() {
        let dir = tempfile::tempdir().unwrap();
        let journal = OperationJournal::open(dir.path().join("ops.jsonl")).unwrap();
        let executable = executable(SyncDirection::LeftToRight);
        let mut session = SyncJournalSession::begin(journal.clone(), &executable).unwrap();
        let id = session.operation_id().clone();

        session.mark_running().unwrap();
        session.complete(1).unwrap();

        let records = journal.read_all().unwrap();
        assert_eq!(records.len(), 3);
        assert!(records.iter().all(|record| record.id == id));
        assert_eq!(
            records
                .iter()
                .map(|record| record.state)
                .collect::<Vec<_>>(),
            vec![
                OperationState::Started,
                OperationState::Running,
                OperationState::Completed,
            ]
        );
        assert_eq!(journal.latest_for(&id).unwrap(), records.last().cloned());
    }

    #[test]
    fn terminal_execution_metadata_is_structured() {
        let dir = tempfile::tempdir().unwrap();
        let journal = OperationJournal::open(dir.path().join("ops.jsonl")).unwrap();
        let executable = executable(SyncDirection::LeftToRight);
        let mut session =
            SyncJournalSession::begin_with_step_count(journal.clone(), &executable, 3).unwrap();
        session.mark_running().unwrap();
        session
            .fail_with_execution(
                1,
                "boom",
                SyncJournalExecutionMetadata {
                    completed_steps: 1,
                    failed_step: Some(2),
                    remaining_steps: 1,
                    transferred_bytes: 7,
                    rollback_attempted: false,
                },
            )
            .unwrap();

        let record = journal.read_all().unwrap().pop().unwrap();
        let metadata: SyncJournalMetadata =
            serde_json::from_value(record.metadata.unwrap()).unwrap();
        let execution = metadata.execution.unwrap();
        assert_eq!(metadata.physical_steps, Some(3));
        assert_eq!(execution.completed_steps, 1);
        assert_eq!(execution.failed_step, Some(2));
        assert_eq!(execution.remaining_steps, 1);
        assert!(!execution.rollback_attempted);
    }

    #[test]
    fn cancel_before_first_mutation_is_recorded_without_running() {
        let dir = tempfile::tempdir().unwrap();
        let journal = OperationJournal::open(dir.path().join("ops.jsonl")).unwrap();
        let executable = executable(SyncDirection::LeftToRight);
        let mut session = SyncJournalSession::begin(journal.clone(), &executable).unwrap();

        session.cancel(0).unwrap();

        assert_eq!(
            journal
                .read_all()
                .unwrap()
                .iter()
                .map(|record| record.state)
                .collect::<Vec<_>>(),
            vec![OperationState::Started, OperationState::Cancelled]
        );
    }

    #[test]
    fn invalid_terminal_transition_is_rejected_without_append() {
        let dir = tempfile::tempdir().unwrap();
        let journal = OperationJournal::open(dir.path().join("ops.jsonl")).unwrap();
        let executable = executable(SyncDirection::LeftToRight);
        let mut session = SyncJournalSession::begin(journal.clone(), &executable).unwrap();

        assert!(matches!(
            session.complete(1),
            Err(SyncJournalError::InvalidTransition {
                from: OperationState::Started,
                to: OperationState::Completed,
            })
        ));
        assert_eq!(journal.read_all().unwrap().len(), 1);
    }

    #[test]
    fn reverse_direction_journals_actual_source_and_destination() {
        let dir = tempfile::tempdir().unwrap();
        let journal = OperationJournal::open(dir.path().join("ops.jsonl")).unwrap();
        let executable = executable(SyncDirection::RightToLeft);

        SyncJournalSession::begin(journal.clone(), &executable).unwrap();

        let record = journal.read_all().unwrap().remove(0);
        assert_eq!(record.source.as_deref(), Some("file:///right"));
        assert_eq!(record.destination.as_deref(), Some("file:///left"));
    }
}
