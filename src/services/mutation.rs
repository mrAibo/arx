use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::vfs::local::LocalFs;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MutationProgress {
    pub completed: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrashOutcome {
    pub completed: usize,
    pub total: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum MutationError {
    #[error("mutation cancelled after {completed} item(s)")]
    Cancelled { completed: usize },
    #[error("mutation worker failed: {0}")]
    Worker(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub struct MutationService;

impl MutationService {
    /// Move local items to Trash without blocking the TUI.
    ///
    /// Cancellation is checked between items. A single recursive directory
    /// move/cross-device copy is still atomic from this service's perspective;
    /// fine-grained directory cancellation belongs in LocalFs v2.
    pub async fn trash_local(
        dir: PathBuf,
        names: Vec<String>,
        cancel: Arc<AtomicBool>,
        mut on_progress: impl FnMut(MutationProgress),
    ) -> Result<TrashOutcome, MutationError> {
        let total = names.len();
        let mut completed = 0usize;

        for name in names {
            if cancel.load(Ordering::Relaxed) {
                return Err(MutationError::Cancelled { completed });
            }
            let dir = dir.clone();
            tokio::task::spawn_blocking(move || LocalFs::delete_files(&dir, &[name]))
                .await
                .map_err(|error| MutationError::Worker(error.to_string()))??;
            completed += 1;
            on_progress(MutationProgress { completed, total });
        }

        Ok(TrashOutcome { completed, total })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pre_cancelled_trash_preserves_source() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("a.txt"), b"a")
            .await
            .unwrap();
        let cancel = Arc::new(AtomicBool::new(true));

        let result = MutationService::trash_local(
            dir.path().to_path_buf(),
            vec!["a.txt".into()],
            cancel,
            |_| {},
        )
        .await;

        assert!(matches!(
            result,
            Err(MutationError::Cancelled { completed: 0 })
        ));
        assert!(dir.path().join("a.txt").exists());
    }
}
