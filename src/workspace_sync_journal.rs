use std::io;

use serde::{Deserialize, Serialize};

use crate::journal::{
    OperationId, OperationJournal, OperationKind, OperationRecord, OperationState,
};
use crate::workspace_sync::{SyncDirection, SyncMode};
use crate::workspace_sync_execution::{ExecutableSyncPlan, FrozenWorkspaceSyncPlan};

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
/// Creating a session records `Started`. The executor must call
/// `mark_running` immediately before its first mutation, then exactly one
/// terminal transition. All rows keep the same `OperationId` and frozen plan
/// metadata so a crash never rewrites or erases earlier facts.
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
        let plan = executable.plan();
        let metadata = SyncJournalMetadata::from_plan(plan);
        let (source, destination) = source_destination(plan);
        let mut record = OperationRecord::new(OperationKind::Synchronize);
        record.source = Some(source);
        record.destination = Some(destination);
        record.item_count = Some(plan.operations().len());
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
        self.append_transition(OperationState::Running, None)
    }

    pub fn complete(&mut self, completed_operations: usize) -> Result<(), SyncJournalError> {
        let total = self.current.item_count.unwrap_or(0);
        self.append_transition(
            OperationState::Completed,
            Some(format!(
                "completed {completed_operations} of {total} sync operation(s)"
            )),
        )
    }

    pub fn cancel(&mut self, completed_operations: usize) -> Result<(), SyncJournalError> {
        let total = self.current.item_count.unwrap_or(0);
        self.append_transition(
            OperationState::Cancelled,
            Some(format!(
                "cancelled after {completed_operations} of {total} sync operation(s)"
            )),
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
        )
    }

    fn append_transition(
        &mut self,
        state: OperationState,
        message: Option<String>,
    ) -> Result<(), SyncJournalError> {
        if !valid_transition(self.current.state, state) {
            return Err(SyncJournalError::InvalidTransition {
                from: self.current.state,
                to: state,
            });
        }

        let next = self.current.transition(state, message);
        self.journal.append(&next)?;
        self.current = next;
        Ok(())
    }
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
