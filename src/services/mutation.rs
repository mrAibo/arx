use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::vfs::local::LocalFs;
use crate::vfs::webdav::{ExactDeleteError, WebDavProvider};
use crate::vfs::{
    EntryIdentity, MAX_WEBDAV_TREE_DEPTH, MAX_WEBDAV_TREE_DESCENDANTS, WebDavCollectionRef,
    WebDavObjectRef,
};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebDavDeleteIdentity {
    Object(WebDavObjectRef),
    Collection(WebDavCollectionRef),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebDavDeleteNode {
    pub identity: WebDavDeleteIdentity,
    pub depth: usize,
    pub canonical_identity: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebDavDeleteManifest {
    pub root: WebDavCollectionRef,
    pub nodes: Vec<WebDavDeleteNode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebDavDeleteOutcome {
    pub completed: usize,
    pub total: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum WebDavDeleteError {
    #[error("cancelled after {completed} of {total} deleted")]
    Cancelled { completed: usize, total: usize },
    #[error("pre-mutation failure: {reason}")]
    PreMutation { reason: String },
    #[error("partial deletion after {completed} of {total}: {reason}")]
    Partial {
        completed: usize,
        total: usize,
        reason: String,
    },
    #[error("Recovery required after {completed} of {total}: {reason}")]
    RecoveryRequired {
        completed: usize,
        total: usize,
        reason: String,
    },
}

pub struct MutationService;

impl MutationService {
    pub async fn build_webdav_delete_manifest(
        provider: &WebDavProvider,
        root: &WebDavCollectionRef,
    ) -> Result<WebDavDeleteManifest, WebDavDeleteError> {
        if provider
            .is_target_root_collection(root)
            .map_err(|e| WebDavDeleteError::PreMutation {
                reason: e.to_string(),
            })?
        {
            return Err(WebDavDeleteError::PreMutation {
                reason: "configured WebDAV target root cannot be deleted".into(),
            });
        }
        let root_id = provider
            .canonical_exact_href_identity(&root.target, &root.href)
            .map_err(|e| WebDavDeleteError::PreMutation {
                reason: e.to_string(),
            })?;
        let mut seen = HashSet::from([root_id]);
        let mut queue = VecDeque::from([(root.clone(), 0usize)]);
        let mut nodes = Vec::new();
        while let Some((collection, depth)) = queue.pop_front() {
            let children = provider
                .list_collection_exact(&collection)
                .await
                .map_err(|e| WebDavDeleteError::PreMutation {
                    reason: e.to_string(),
                })?;
            for child in children {
                let child_depth = depth + 1;
                if child_depth > MAX_WEBDAV_TREE_DEPTH {
                    return Err(WebDavDeleteError::PreMutation {
                        reason: "WebDAV delete tree depth exceeds 128".into(),
                    });
                }
                if nodes.len() == MAX_WEBDAV_TREE_DESCENDANTS {
                    return Err(WebDavDeleteError::PreMutation {
                        reason: "WebDAV delete tree exceeds 50000 descendants".into(),
                    });
                }
                let (identity, target, href, collection_child) = match child.identity {
                    EntryIdentity::WebDavObject(object) => {
                        let t = object.target.clone();
                        let h = object.href.clone();
                        (WebDavDeleteIdentity::Object(object), t, h, None)
                    }
                    EntryIdentity::WebDavCollection(collection) => {
                        let t = collection.target.clone();
                        let h = collection.href.clone();
                        (
                            WebDavDeleteIdentity::Collection(collection.clone()),
                            t,
                            h,
                            Some(collection),
                        )
                    }
                    _ => {
                        return Err(WebDavDeleteError::PreMutation {
                            reason: "WebDAV listing returned non-WebDAV identity".into(),
                        });
                    }
                };
                let canonical = provider
                    .canonical_exact_href_identity(&target, &href)
                    .map_err(|e| WebDavDeleteError::PreMutation {
                        reason: e.to_string(),
                    })?;
                if !seen.insert(canonical.clone()) {
                    return Err(WebDavDeleteError::PreMutation {
                        reason: "duplicate/cyclic WebDAV identity".into(),
                    });
                }
                nodes.push(WebDavDeleteNode {
                    identity,
                    depth: child_depth,
                    canonical_identity: canonical,
                });
                if let Some(collection) = collection_child {
                    queue.push_back((collection, child_depth));
                }
            }
        }
        nodes.sort_by(|a, b| {
            b.depth
                .cmp(&a.depth)
                .then_with(|| {
                    matches!(a.identity, WebDavDeleteIdentity::Collection(_))
                        .cmp(&matches!(b.identity, WebDavDeleteIdentity::Collection(_)))
                })
                .then_with(|| a.canonical_identity.cmp(&b.canonical_identity))
        });
        Ok(WebDavDeleteManifest {
            root: root.clone(),
            nodes,
        })
    }

    /// Delete exactly a previously frozen WebDAV tree snapshot.
    ///
    /// The current tree is rebuilt immediately before the first DELETE and must
    /// still equal `frozen`. This seam is intentionally public so a higher-level
    /// transaction (for example copy -> verify -> delete-source Move) can commit
    /// the exact snapshot it copied instead of silently freezing newer content.
    pub async fn delete_webdav_tree_from_frozen_manifest(
        provider: Arc<WebDavProvider>,
        frozen: WebDavDeleteManifest,
        cancel: Arc<AtomicBool>,
        mut on_progress: impl FnMut(MutationProgress),
    ) -> Result<WebDavDeleteOutcome, WebDavDeleteError> {
        let total = 1 + frozen.nodes.len();
        if cancel.load(Ordering::Acquire) {
            return Err(WebDavDeleteError::Cancelled {
                completed: 0,
                total,
            });
        }

        let root = frozen.root.clone();
        let fresh = Self::build_webdav_delete_manifest(&provider, &root).await?;
        if fresh != frozen {
            return Err(WebDavDeleteError::PreMutation {
                reason: "WebDAV tree changed after manifest freeze".into(),
            });
        }
        if cancel.load(Ordering::Acquire) {
            return Err(WebDavDeleteError::Cancelled {
                completed: 0,
                total,
            });
        }

        let mut completed = 0usize;
        let mut ordered = frozen.nodes;
        ordered.push(WebDavDeleteNode {
            canonical_identity: provider
                .canonical_exact_href_identity(&root.target, &root.href)
                .map_err(|e| WebDavDeleteError::PreMutation {
                    reason: e.to_string(),
                })?,
            identity: WebDavDeleteIdentity::Collection(root),
            depth: 0,
        });
        for node in ordered {
            if cancel.load(Ordering::Acquire) {
                return Err(WebDavDeleteError::Cancelled { completed, total });
            }
            let result = match &node.identity {
                WebDavDeleteIdentity::Object(object) => provider.delete_object_exact(object).await,
                WebDavDeleteIdentity::Collection(collection) => {
                    let children =
                        provider
                            .list_collection_exact(collection)
                            .await
                            .map_err(|e| {
                                if completed == 0 {
                                    WebDavDeleteError::PreMutation {
                                        reason: e.to_string(),
                                    }
                                } else {
                                    WebDavDeleteError::Partial {
                                        completed,
                                        total,
                                        reason: e.to_string(),
                                    }
                                }
                            })?;
                    if !children.is_empty() {
                        return Err(if completed == 0 {
                            WebDavDeleteError::PreMutation {
                                reason: "collection is not empty at delete boundary".into(),
                            }
                        } else {
                            WebDavDeleteError::Partial {
                                completed,
                                total,
                                reason: "collection is not empty at delete boundary".into(),
                            }
                        });
                    }
                    if cancel.load(Ordering::Acquire) {
                        return Err(WebDavDeleteError::Cancelled { completed, total });
                    }
                    provider.delete_collection_exact(collection).await
                }
            };
            match result {
                Ok(()) => {
                    completed += 1;
                    on_progress(MutationProgress { completed, total });
                }
                Err(ExactDeleteError::Definitive(error)) => {
                    return Err(if completed == 0 {
                        WebDavDeleteError::PreMutation {
                            reason: error.to_string(),
                        }
                    } else {
                        WebDavDeleteError::Partial {
                            completed,
                            total,
                            reason: error.to_string(),
                        }
                    });
                }
                Err(ExactDeleteError::Ambiguous(error)) => {
                    return Err(WebDavDeleteError::RecoveryRequired {
                        completed,
                        total,
                        reason: error.to_string(),
                    });
                }
            }
        }
        Ok(WebDavDeleteOutcome { completed, total })
    }

    pub async fn delete_webdav_tree(
        provider: Arc<WebDavProvider>,
        root: WebDavCollectionRef,
        cancel: Arc<AtomicBool>,
        on_progress: impl FnMut(MutationProgress),
    ) -> Result<WebDavDeleteOutcome, WebDavDeleteError> {
        let frozen = Self::build_webdav_delete_manifest(&provider, &root).await?;
        Self::delete_webdav_tree_from_frozen_manifest(provider, frozen, cancel, on_progress).await
    }

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
