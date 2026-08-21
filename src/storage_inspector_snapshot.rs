//! Immutable completed-scan snapshots for the Storage Inspector UI.
//!
//! Large `UsageScanResult` record vectors do not belong in generic `JobResult`
//! strings or repeated TUI clones. This store keeps one immutable `Arc` per
//! storage-scan JobId. JobManager remains the lifecycle/cancellation source of
//! truth; this store owns only completed drill-down data.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::storage_inspector::UsageScanResult;

#[derive(Debug, Clone, Default)]
pub struct StorageScanSnapshotStore {
    inner: Arc<Mutex<BTreeMap<String, Arc<UsageScanResult>>>>,
}

impl StorageScanSnapshotStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace the immutable snapshot for one storage-scan JobId.
    ///
    /// Returning the same `Arc` lets the caller hand the result to another
    /// read-only consumer without cloning the potentially large record vector.
    pub fn insert(
        &self,
        job_id: impl Into<String>,
        result: UsageScanResult,
    ) -> Arc<UsageScanResult> {
        let snapshot = Arc::new(result);
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.insert(job_id.into(), Arc::clone(&snapshot));
        snapshot
    }

    pub fn get(&self, job_id: &str) -> Option<Arc<UsageScanResult>> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(job_id)
            .cloned()
    }

    pub fn remove(&self, job_id: &str) -> Option<Arc<UsageScanResult>> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(job_id)
    }

    pub fn contains(&self, job_id: &str) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains_key(job_id)
    }

    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage_inspector::{UsageScanOutcome, UsageTotals};
    use std::path::PathBuf;

    fn result(root: &str) -> UsageScanResult {
        UsageScanResult {
            root: PathBuf::from(root),
            outcome: UsageScanOutcome::Complete,
            totals: UsageTotals::default(),
            records: Vec::new(),
            top_files: Vec::new(),
        }
    }

    #[test]
    fn snapshots_are_keyed_by_job_id_without_copying_on_get() {
        let store = StorageScanSnapshotStore::new();
        let inserted = store.insert("storage-1", result("/tmp/a"));
        let fetched = store.get("storage-1").expect("snapshot");

        assert!(Arc::ptr_eq(&inserted, &fetched));
        assert_eq!(fetched.root, PathBuf::from("/tmp/a"));
    }

    #[test]
    fn replacing_one_job_does_not_touch_other_snapshots() {
        let store = StorageScanSnapshotStore::new();
        store.insert("storage-1", result("/tmp/old"));
        store.insert("storage-2", result("/tmp/other"));
        store.insert("storage-1", result("/tmp/new"));

        assert_eq!(store.len(), 2);
        assert_eq!(store.get("storage-1").unwrap().root, PathBuf::from("/tmp/new"));
        assert_eq!(
            store.get("storage-2").unwrap().root,
            PathBuf::from("/tmp/other")
        );
    }

    #[test]
    fn remove_is_explicit_and_idempotent() {
        let store = StorageScanSnapshotStore::new();
        store.insert("storage-1", result("/tmp/a"));

        assert!(store.contains("storage-1"));
        assert!(store.remove("storage-1").is_some());
        assert!(store.remove("storage-1").is_none());
        assert!(store.is_empty());
    }
}
