//! Persistent operation journal for filesystem and transfer activity.
//!
//! The journal records what ARX changed without coupling history to a
//! particular VFS backend. Undo execution stays in the application/service
//! layer; this module only persists structured facts and undo metadata.

use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const SCHEMA_VERSION: u32 = 2;
const JOURNAL_FILE: &str = "operations.jsonl";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    Copy,
    Move,
    Trash,
    Restore,
    Rename,
    Synchronize,
    RemoteWatch,
    Custom(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    Started,
    Running,
    Completed,
    Failed,
    Cancelled,
    Undone,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UndoRecord {
    /// Stable application-level action name, e.g. `restore_trash`.
    pub action: String,
    /// Opaque data consumed by the service that owns the undo action.
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationRecord {
    pub schema_version: u32,
    pub id: OperationId,
    pub timestamp_unix_ms: u128,
    pub kind: OperationKind,
    pub state: OperationState,
    pub source: Option<String>,
    pub destination: Option<String>,
    pub item_count: Option<usize>,
    pub message: Option<String>,
    pub undo: Option<UndoRecord>,
    /// Operation-specific structured facts. Older schema-v1 records omit this
    /// field and continue to deserialize as `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

impl OperationRecord {
    pub fn new(kind: OperationKind) -> Self {
        let timestamp_unix_ms = now_ms();
        Self {
            schema_version: SCHEMA_VERSION,
            id: OperationId(format!("op-{timestamp_unix_ms}")),
            timestamp_unix_ms,
            kind,
            state: OperationState::Started,
            source: None,
            destination: None,
            item_count: None,
            message: None,
            undo: None,
            metadata: None,
        }
    }

    /// Create the next append-only lifecycle record for the same logical
    /// operation. The operation identity and structured facts stay frozen;
    /// only state, timestamp, and the transition message change.
    pub fn transition(&self, state: OperationState, message: Option<String>) -> Self {
        let mut next = self.clone();
        next.schema_version = SCHEMA_VERSION;
        next.timestamp_unix_ms = now_ms();
        next.state = state;
        next.message = message;
        next
    }
}

#[derive(Debug, Clone)]
pub struct OperationJournal {
    path: PathBuf,
}

impl OperationJournal {
    pub fn open_default() -> io::Result<Self> {
        let dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("arx");
        Self::open(dir.join(JOURNAL_FILE))
    }

    pub fn open(path: PathBuf) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one complete JSON object per line. A partial final line can be
    /// ignored safely after a crash; earlier records remain readable.
    pub fn append(&self, record: &OperationRecord) -> io::Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        serde_json::to_writer(&mut file, record).map_err(io::Error::other)?;
        file.write_all(b"\n")?;
        file.flush()
    }

    pub fn read_all(&self) -> io::Result<Vec<OperationRecord>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }

        let file = fs::File::open(&self.path)?;
        let mut records = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<OperationRecord>(&line) {
                Ok(record) => records.push(record),
                Err(_) => {
                    // A crash can leave a truncated tail record. Keep earlier
                    // valid history instead of making the whole journal unusable.
                    break;
                }
            }
        }
        Ok(records)
    }

    pub fn recent(&self, limit: usize) -> io::Result<Vec<OperationRecord>> {
        let records = self.read_all()?;
        let start = records.len().saturating_sub(limit);
        Ok(records[start..].to_vec())
    }

    /// Return the newest lifecycle snapshot for one logical operation.
    pub fn latest_for(&self, id: &OperationId) -> io::Result<Option<OperationRecord>> {
        Ok(self
            .read_all()?
            .into_iter()
            .rev()
            .find(|record| record.id == *id))
    }

    pub fn latest_undoable(&self) -> io::Result<Option<OperationRecord>> {
        Ok(self
            .read_all()?
            .into_iter()
            .rev()
            .find(|record| record.state == OperationState::Completed && record.undo.is_some()))
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_and_reads_records() {
        let dir = tempfile::tempdir().unwrap();
        let journal = OperationJournal::open(dir.path().join("ops.jsonl")).unwrap();
        let mut record = OperationRecord::new(OperationKind::Copy);
        record.state = OperationState::Completed;
        record.source = Some("file:///tmp/a".into());
        record.destination = Some("sftp://host/tmp/a".into());
        record.item_count = Some(1);

        journal.append(&record).unwrap();

        assert_eq!(journal.read_all().unwrap(), vec![record]);
    }

    #[test]
    fn transition_preserves_operation_identity_and_metadata() {
        let mut started = OperationRecord::new(OperationKind::Synchronize);
        started.metadata = Some(serde_json::json!({"plan_id": 7}));
        started.source = Some("file:///left".into());
        started.destination = Some("file:///right".into());

        let running = started.transition(OperationState::Running, Some("running".into()));

        assert_eq!(running.id, started.id);
        assert_eq!(running.kind, started.kind);
        assert_eq!(running.metadata, started.metadata);
        assert_eq!(running.source, started.source);
        assert_eq!(running.destination, started.destination);
        assert_eq!(running.state, OperationState::Running);
        assert_eq!(running.message.as_deref(), Some("running"));
        assert_eq!(running.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn reads_schema_v1_records_without_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ops.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"schema_version\":1,\"id\":\"op-old\",\"timestamp_unix_ms\":1,",
                "\"kind\":\"copy\",\"state\":\"completed\",\"source\":null,",
                "\"destination\":null,\"item_count\":null,\"message\":null,\"undo\":null}\n"
            ),
        )
        .unwrap();
        let journal = OperationJournal::open(path).unwrap();

        let records = journal.read_all().unwrap();

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].schema_version, 1);
        assert_eq!(records[0].metadata, None);
    }

    #[test]
    fn latest_for_returns_latest_lifecycle_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let journal = OperationJournal::open(dir.path().join("ops.jsonl")).unwrap();
        let started = OperationRecord::new(OperationKind::Synchronize);
        let running = started.transition(OperationState::Running, None);
        journal.append(&started).unwrap();
        journal.append(&running).unwrap();

        assert_eq!(journal.latest_for(&started.id).unwrap(), Some(running));
    }

    #[test]
    fn finds_latest_undoable_operation() {
        let dir = tempfile::tempdir().unwrap();
        let journal = OperationJournal::open(dir.path().join("ops.jsonl")).unwrap();

        let mut copy = OperationRecord::new(OperationKind::Copy);
        copy.state = OperationState::Completed;
        journal.append(&copy).unwrap();

        let mut trash = OperationRecord::new(OperationKind::Trash);
        trash.state = OperationState::Completed;
        trash.undo = Some(UndoRecord {
            action: "restore_trash".into(),
            payload: serde_json::json!({"trash_id": "abc"}),
        });
        journal.append(&trash).unwrap();

        assert_eq!(journal.latest_undoable().unwrap(), Some(trash));
    }

    #[test]
    fn ignores_truncated_tail_after_valid_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ops.jsonl");
        let journal = OperationJournal::open(path.clone()).unwrap();
        let record = OperationRecord::new(OperationKind::Move);
        journal.append(&record).unwrap();

        let mut file = OpenOptions::new().append(true).open(path).unwrap();
        file.write_all(b"{\"schema_version\":2").unwrap();

        assert_eq!(journal.read_all().unwrap(), vec![record]);
    }
}
