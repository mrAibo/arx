use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::vfs::webdav::{ExactDeleteError, WebDavProvider};
use crate::vfs::{
    EntryIdentity, EntryKind, ListedEntry, Location, MAX_WEBDAV_TREE_DESCENDANTS,
    WebDavCollectionRef,
};

use super::mutation::{
    MutationProgress, MutationService, WebDavDeleteError, WebDavDeleteIdentity,
    WebDavDeleteManifest, WebDavDeleteNode, WebDavDeleteOutcome,
};

/// Frozen provider-native WebDAV delete selection.
///
/// `source` / `presentation_name` preserve the accepted single-root surface while
/// `sources` / `presentation_names` carry the complete deterministic batch for
/// the multi-root slice. Execution identity is always taken from `sources`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebDavRecursiveDeletePlan {
    pub source: WebDavCollectionRef,
    pub presentation_name: String,
    pub sources: Vec<WebDavCollectionRef>,
    pub presentation_names: Vec<String>,
    pub created_at: Instant,
}

/// Compatibility entry point for the already-shipped single-root F8 path.
///
/// Keep rejecting multi-selection until the TUI is switched atomically to the
/// batch executor; this prevents an intermediate branch state from accepting a
/// multi-selection and then deleting only the first root.
pub fn prepare_webdav_recursive_delete(
    location: &Location,
    selected_names: &[String],
    focused: Option<&ListedEntry>,
    active: &[&ListedEntry],
) -> Result<WebDavRecursiveDeletePlan, String> {
    if selected_names.len() > 1 {
        return Err("WebDAV recursive delete supports exactly one collection".into());
    }
    prepare_webdav_recursive_delete_batch(location, selected_names, focused, active)
}

/// Freeze one or more exact WebDAV collection roots from the active listing.
///
/// Selection wins over focus. Every selected presentation name must resolve to
/// exactly one current real listing row; display names never become remote
/// addressing authority.
pub fn prepare_webdav_recursive_delete_batch(
    location: &Location,
    selected_names: &[String],
    focused: Option<&ListedEntry>,
    active: &[&ListedEntry],
) -> Result<WebDavRecursiveDeletePlan, String> {
    let Location::WebDav { target, .. } = location else {
        return Err("WebDAV recursive delete requires an active WebDAV location".into());
    };

    let mut roots: Vec<(WebDavCollectionRef, String)> = if selected_names.is_empty() {
        let listed = focused.ok_or_else(|| "Focus a WebDAV collection to delete".to_string())?;
        vec![resolve_collection_row(listed, target)?]
    } else {
        let mut resolved = Vec::with_capacity(selected_names.len());
        for name in selected_names {
            let matches: Vec<_> = active
                .iter()
                .filter(|entry| entry.entry.name == *name)
                .copied()
                .collect();
            if matches.len() != 1 {
                return Err("Selection is stale or ambiguous".into());
            }
            resolved.push(resolve_collection_row(matches[0], target)?);
        }
        resolved
    };

    if roots.is_empty() {
        return Err("Select a WebDAV collection to delete".into());
    }

    let mut exact_seen = HashSet::new();
    for (source, _) in &roots {
        if !exact_seen.insert((source.target.clone(), source.href.clone())) {
            return Err("Duplicate exact WebDAV collection identity in selection".into());
        }
    }

    // Selection storage order is not execution authority. Freeze a deterministic
    // exact-identity order so root ordering is stable across UI/hash iteration.
    roots.sort_by(|(left, _), (right, _)| {
        left.target
            .cmp(&right.target)
            .then_with(|| left.href.cmp(&right.href))
    });

    let sources: Vec<_> = roots.iter().map(|(source, _)| source.clone()).collect();
    let presentation_names: Vec<_> = roots.iter().map(|(_, name)| name.clone()).collect();
    let source = sources[0].clone();
    let presentation_name = presentation_names[0].clone();

    Ok(WebDavRecursiveDeletePlan {
        source,
        presentation_name,
        sources,
        presentation_names,
        created_at: Instant::now(),
    })
}

fn resolve_collection_row(
    listed: &ListedEntry,
    target: &str,
) -> Result<(WebDavCollectionRef, String), String> {
    if listed.entry.kind != EntryKind::Directory {
        return Err("WebDAV recursive delete requires collections only".into());
    }
    let EntryIdentity::WebDavCollection(source) = &listed.identity else {
        return Err("Selected row has no exact WebDAV collection identity".into());
    };
    if source.target != target {
        return Err("WebDAV collection target does not match active target".into());
    }
    Ok((source.clone(), listed.entry.name.clone()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WebDavDeleteBatchManifest {
    manifests: Vec<WebDavDeleteManifest>,
    total: usize,
}

impl MutationService {
    /// Delete multiple exact WebDAV collection roots as one truthful mutation
    /// job. The existing single-root implementation remains authoritative when
    /// exactly one root is supplied, preserving its accepted 50,000-descendant
    /// semantics without silently tightening that shipped contract.
    pub async fn delete_webdav_trees(
        provider: Arc<WebDavProvider>,
        roots: Vec<WebDavCollectionRef>,
        cancel: Arc<AtomicBool>,
        mut on_progress: impl FnMut(MutationProgress),
    ) -> Result<WebDavDeleteOutcome, WebDavDeleteError> {
        if roots.is_empty() {
            return Err(WebDavDeleteError::PreMutation {
                reason: "WebDAV delete batch contains no roots".into(),
            });
        }
        if roots.len() == 1 {
            return Self::delete_webdav_tree(
                provider,
                roots.into_iter().next().expect("one root"),
                cancel,
                on_progress,
            )
            .await;
        }

        // Complete aggregate discovery before the first destructive request.
        let frozen = Self::build_webdav_delete_batch_manifest(&provider, &roots).await?;
        if cancel.load(Ordering::Acquire) {
            return Err(WebDavDeleteError::Cancelled {
                completed: 0,
                total: frozen.total,
            });
        }

        // Rebuild every selected tree immediately before mutation. A change in
        // any root invalidates the entire batch with zero ARX deletion.
        let fresh = Self::build_webdav_delete_batch_manifest(&provider, &roots).await?;
        if fresh != frozen {
            return Err(WebDavDeleteError::PreMutation {
                reason: "WebDAV delete batch changed after confirmation".into(),
            });
        }
        if cancel.load(Ordering::Acquire) {
            return Err(WebDavDeleteError::Cancelled {
                completed: 0,
                total: frozen.total,
            });
        }

        let total = frozen.total;
        let mut completed = 0usize;
        for manifest in frozen.manifests {
            let root = manifest.root;
            let mut ordered = manifest.nodes;
            let root_identity = provider
                .canonical_exact_href_identity(&root.target, &root.href)
                .map_err(|error| WebDavDeleteError::PreMutation {
                    reason: error.to_string(),
                })?;
            ordered.push(WebDavDeleteNode {
                canonical_identity: root_identity,
                identity: WebDavDeleteIdentity::Collection(root),
                depth: 0,
            });

            for node in ordered {
                if cancel.load(Ordering::Acquire) {
                    return Err(WebDavDeleteError::Cancelled { completed, total });
                }

                let exact_identity = node.canonical_identity.clone();
                let result = match &node.identity {
                    WebDavDeleteIdentity::Object(object) => {
                        provider.delete_object_exact(object).await
                    }
                    WebDavDeleteIdentity::Collection(collection) => {
                        let children =
                            provider
                                .list_collection_exact(collection)
                                .await
                                .map_err(|error| {
                                    batch_definitive_failure(
                                        completed,
                                        total,
                                        &exact_identity,
                                        &error.to_string(),
                                    )
                                })?;
                        if !children.is_empty() {
                            return Err(batch_definitive_failure(
                                completed,
                                total,
                                &exact_identity,
                                "collection is not empty at delete boundary",
                            ));
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
                        return Err(batch_definitive_failure(
                            completed,
                            total,
                            &exact_identity,
                            &error.to_string(),
                        ));
                    }
                    Err(ExactDeleteError::Ambiguous(error)) => {
                        return Err(WebDavDeleteError::RecoveryRequired {
                            completed,
                            total,
                            reason: format!("{exact_identity}: {error}"),
                        });
                    }
                }
            }
        }

        Ok(WebDavDeleteOutcome { completed, total })
    }

    async fn build_webdav_delete_batch_manifest(
        provider: &WebDavProvider,
        roots: &[WebDavCollectionRef],
    ) -> Result<WebDavDeleteBatchManifest, WebDavDeleteError> {
        let mut ordered_roots = Vec::with_capacity(roots.len());
        let mut root_seen = HashSet::new();
        for root in roots {
            let canonical = provider
                .canonical_exact_href_identity(&root.target, &root.href)
                .map_err(|error| WebDavDeleteError::PreMutation {
                    reason: error.to_string(),
                })?;
            if !root_seen.insert(canonical.clone()) {
                return Err(WebDavDeleteError::PreMutation {
                    reason: "duplicate exact WebDAV root identity in delete batch".into(),
                });
            }
            ordered_roots.push((canonical, root.clone()));
        }
        ordered_roots.sort_by(|(left, _), (right, _)| left.cmp(right));

        let mut manifests = Vec::with_capacity(ordered_roots.len());
        let mut all_seen = HashSet::new();
        let mut total = 0usize;
        for (root_identity, root) in ordered_roots {
            let manifest = Self::build_webdav_delete_manifest(provider, &root).await?;

            if !all_seen.insert(root_identity) {
                return Err(WebDavDeleteError::PreMutation {
                    reason: "duplicate/cyclic WebDAV identity across delete roots".into(),
                });
            }
            for node in &manifest.nodes {
                if !all_seen.insert(node.canonical_identity.clone()) {
                    return Err(WebDavDeleteError::PreMutation {
                        reason: "duplicate/cyclic WebDAV identity across delete roots".into(),
                    });
                }
            }

            total = total.checked_add(1 + manifest.nodes.len()).ok_or_else(|| {
                WebDavDeleteError::PreMutation {
                    reason: "WebDAV delete batch item count overflow".into(),
                }
            })?;
            if total > MAX_WEBDAV_TREE_DESCENDANTS {
                return Err(WebDavDeleteError::PreMutation {
                    reason: "WebDAV multi-root delete exceeds 50000 planned items".into(),
                });
            }
            manifests.push(manifest);
        }

        Ok(WebDavDeleteBatchManifest { manifests, total })
    }
}

fn batch_definitive_failure(
    completed: usize,
    total: usize,
    exact_identity: &str,
    reason: &str,
) -> WebDavDeleteError {
    let reason = format!("{exact_identity}: {reason}");
    if completed == 0 {
        WebDavDeleteError::PreMutation { reason }
    } else {
        WebDavDeleteError::Partial {
            completed,
            total,
            reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::{Entry, WebDavObjectRef};

    fn collection(name: &str, target: &str, href: &str) -> ListedEntry {
        ListedEntry {
            entry: Entry {
                name: name.into(),
                kind: EntryKind::Directory,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::WebDavCollection(WebDavCollectionRef {
                target: target.into(),
                href: href.into(),
            }),
        }
    }

    #[test]
    fn batch_plan_freezes_all_selected_exact_roots_deterministically() {
        let location = Location::WebDav {
            target: "t".into(),
            path: "/".into(),
        };
        let alpha = collection("alpha", "t", "/dav/a%20raw/?q=1");
        let beta = collection("beta", "t", "/dav/beta/");
        let selected = vec!["beta".to_string(), "alpha".to_string()];

        let plan = prepare_webdav_recursive_delete_batch(
            &location,
            &selected,
            Some(&beta),
            &[&beta, &alpha],
        )
        .unwrap();

        assert_eq!(plan.sources.len(), 2);
        assert_eq!(plan.sources[0].href, "/dav/a%20raw/?q=1");
        assert_eq!(plan.sources[1].href, "/dav/beta/");
        assert_eq!(plan.presentation_names, vec!["alpha", "beta"]);
        assert_eq!(plan.source, plan.sources[0]);
    }

    #[test]
    fn batch_plan_selection_wins_and_fails_closed_as_a_whole() {
        let location = Location::WebDav {
            target: "t".into(),
            path: "/".into(),
        };
        let alpha = collection("alpha", "t", "/dav/alpha/");
        let beta = collection("beta", "t", "/dav/beta/");
        let duplicate_beta = collection("beta", "t", "/dav/other-beta/");
        let wrong_target = collection("wrong", "other", "/dav/wrong/");
        let file = ListedEntry {
            entry: Entry {
                name: "file".into(),
                kind: EntryKind::File,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::WebDavObject(WebDavObjectRef {
                target: "t".into(),
                href: "/dav/file".into(),
            }),
        };

        let selected = vec!["alpha".to_string(), "beta".to_string()];
        let plan = prepare_webdav_recursive_delete_batch(
            &location,
            &selected,
            Some(&wrong_target),
            &[&alpha, &beta],
        )
        .unwrap();
        assert_eq!(plan.sources.len(), 2);

        assert!(
            prepare_webdav_recursive_delete_batch(
                &location,
                &["missing".into()],
                Some(&alpha),
                &[&alpha, &beta],
            )
            .is_err()
        );
        assert!(
            prepare_webdav_recursive_delete_batch(
                &location,
                &["beta".into()],
                Some(&alpha),
                &[&beta, &duplicate_beta],
            )
            .is_err()
        );
        assert!(
            prepare_webdav_recursive_delete_batch(
                &location,
                &["alpha".into(), "file".into()],
                Some(&alpha),
                &[&alpha, &file],
            )
            .is_err()
        );
        assert!(
            prepare_webdav_recursive_delete_batch(
                &location,
                &["alpha".into(), "wrong".into()],
                Some(&alpha),
                &[&alpha, &wrong_target],
            )
            .is_err()
        );
    }

    #[test]
    fn compatibility_planner_still_rejects_multi_selection() {
        let location = Location::WebDav {
            target: "t".into(),
            path: "/".into(),
        };
        let alpha = collection("alpha", "t", "/dav/alpha/");
        let beta = collection("beta", "t", "/dav/beta/");
        assert!(
            prepare_webdav_recursive_delete(
                &location,
                &["alpha".into(), "beta".into()],
                Some(&alpha),
                &[&alpha, &beta],
            )
            .is_err()
        );
    }
}
