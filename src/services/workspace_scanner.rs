use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use tokio::sync::mpsc;

use crate::vfs::{EntryKind, Location, ProviderRegistry};
use crate::workspace_sync::{WorkspaceEntry, WorkspaceFingerprint, WorkspaceSide};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkspaceScanId(pub u64);

#[derive(Debug, Clone, Copy)]
pub struct WorkspaceScanOptions {
    pub max_depth: usize,
    pub max_entries: usize,
}

impl Default for WorkspaceScanOptions {
    fn default() -> Self {
        Self {
            max_depth: 32,
            max_entries: 100_000,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceScanError {
    #[error("workspace scan cancelled")]
    Cancelled,
    #[error("workspace scan exceeded the {limit} entry safety limit")]
    EntryLimit { limit: usize },
    #[error("{location}: {source}")]
    Vfs {
        location: Location,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug)]
pub struct WorkspaceScanResponse {
    pub id: WorkspaceScanId,
    pub side: WorkspaceSide,
    pub root: Location,
    pub result: Result<Vec<WorkspaceEntry>, WorkspaceScanError>,
}

#[derive(Clone)]
pub struct WorkspaceScanner {
    registry: ProviderRegistry,
    next_id: Arc<AtomicU64>,
    tx: mpsc::UnboundedSender<WorkspaceScanResponse>,
}

impl WorkspaceScanner {
    pub fn channel(
        registry: ProviderRegistry,
    ) -> (Self, mpsc::UnboundedReceiver<WorkspaceScanResponse>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Self {
                registry,
                next_id: Arc::new(AtomicU64::new(1)),
                tx,
            },
            rx,
        )
    }

    pub fn scan(
        &self,
        side: WorkspaceSide,
        root: Location,
        options: WorkspaceScanOptions,
        cancel: Arc<AtomicBool>,
    ) -> WorkspaceScanId {
        let id = WorkspaceScanId(self.next_id.fetch_add(1, Ordering::Relaxed));
        let registry = self.registry.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = scan_workspace(&registry, &root, options, &cancel).await;
            let _ = tx.send(WorkspaceScanResponse {
                id,
                side,
                root,
                result,
            });
        });
        id
    }
}

pub async fn scan_workspace(
    registry: &ProviderRegistry,
    root: &Location,
    options: WorkspaceScanOptions,
    cancel: &AtomicBool,
) -> Result<Vec<WorkspaceEntry>, WorkspaceScanError> {
    let mut queue = VecDeque::new();
    queue.push_back((root.clone(), String::new(), 0usize));

    let mut result = Vec::new();
    while let Some((location, relative_dir, depth)) = queue.pop_front() {
        if cancel.load(Ordering::Relaxed) {
            return Err(WorkspaceScanError::Cancelled);
        }

        let entries = registry
            .list_location_async(&location)
            .await
            .map_err(|source| WorkspaceScanError::Vfs {
                location: location.clone(),
                source,
            })?;

        for entry in entries {
            if cancel.load(Ordering::Relaxed) {
                return Err(WorkspaceScanError::Cancelled);
            }
            if result.len() >= options.max_entries {
                return Err(WorkspaceScanError::EntryLimit {
                    limit: options.max_entries,
                });
            }

            let relative_path = if relative_dir.is_empty() {
                entry.name.clone()
            } else {
                format!("{relative_dir}/{}", entry.name)
            };

            result.push(WorkspaceEntry {
                relative_path: relative_path.clone(),
                fingerprint: WorkspaceFingerprint {
                    kind: entry.kind,
                    size: entry.size,
                    // Providers may supply canonical mtime evidence. Hashes remain
                    // optional; missing evidence stays conservative rather than guessing equality.
                    modified_unix_ms: entry.modified_unix_ms,
                    content_hash: None,
                },
            });

            if entry.kind == EntryKind::Directory && depth < options.max_depth {
                queue.push_back((location.child(&entry.name), relative_path, depth + 1));
            }
        }
    }

    result.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn recursively_scans_local_tree() {
        let root = tempfile::tempdir().unwrap();
        tokio::fs::create_dir(root.path().join("src"))
            .await
            .unwrap();
        tokio::fs::write(root.path().join("README.md"), b"readme")
            .await
            .unwrap();
        tokio::fs::write(root.path().join("src/main.rs"), b"fn main() {}")
            .await
            .unwrap();

        let cancel = AtomicBool::new(false);
        let entries = scan_workspace(
            &crate::vfs::default_registry(),
            &Location::Local(PathBuf::from(root.path())),
            WorkspaceScanOptions::default(),
            &cancel,
        )
        .await
        .unwrap();

        let paths: Vec<&str> = entries
            .iter()
            .map(|entry| entry.relative_path.as_str())
            .collect();
        assert_eq!(paths, vec!["README.md", "src", "src/main.rs"]);

        let direct_mtime = crate::vfs::local::LocalFs::list(root.path())
            .unwrap()
            .into_iter()
            .find(|entry| entry.name == "README.md")
            .and_then(|entry| entry.modified_unix_ms);
        let scanned_mtime = entries
            .iter()
            .find(|entry| entry.relative_path == "README.md")
            .and_then(|entry| entry.fingerprint.modified_unix_ms);
        assert_eq!(scanned_mtime, direct_mtime);
        assert!(scanned_mtime.is_some());
    }

    #[tokio::test]
    async fn respects_entry_limit() {
        let root = tempfile::tempdir().unwrap();
        tokio::fs::write(root.path().join("a"), b"a").await.unwrap();
        tokio::fs::write(root.path().join("b"), b"b").await.unwrap();

        let cancel = AtomicBool::new(false);
        let error = scan_workspace(
            &crate::vfs::default_registry(),
            &Location::Local(root.path().to_path_buf()),
            WorkspaceScanOptions {
                max_depth: 1,
                max_entries: 1,
            },
            &cancel,
        )
        .await
        .unwrap_err();

        assert!(matches!(error, WorkspaceScanError::EntryLimit { limit: 1 }));
    }

    #[tokio::test]
    async fn pre_cancelled_scan_does_no_work() {
        let root = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(true);
        let result = scan_workspace(
            &crate::vfs::default_registry(),
            &Location::Local(root.path().to_path_buf()),
            WorkspaceScanOptions::default(),
            &cancel,
        )
        .await;
        assert!(matches!(result, Err(WorkspaceScanError::Cancelled)));
    }
}
