use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::services::mutation::{
    MutationService, WebDavDeleteError, WebDavDeleteIdentity, WebDavDeleteManifest,
};
use crate::transfer_queue::{PauseGate, TypedTransferProgress};
use crate::vfs::webdav::WebDavProvider;
use crate::vfs::{EntryIdentity, EntryKind, ListedEntry, Location, WebDavCollectionRef};

use super::webdav_transfer::{
    WebDavTreeManifest, build_tree_manifest, copy_tree, revalidate_tree_manifest,
};
use super::{WebDavTransferSpec, WebDavWriteTarget, webdav_write_child_target};

#[derive(Debug, thiserror::Error)]
pub(crate) enum MoveTreeFailure {
    #[error(
        "WebDAV Move destination changed after verification at {target}:{logical_path}: {reason}"
    )]
    DestinationChanged {
        target: String,
        logical_path: String,
        reason: String,
    },
    #[error(
        "WebDAV Move could not clean verified attempt-owned destination {target}:{logical_path}: original={original}; cleanup={cleanup}"
    )]
    CleanupFailure {
        target: String,
        logical_path: String,
        original: String,
        cleanup: String,
    },
    #[error("WebDAV Move cancelled before source commit after {completed} of {total} transaction items")]
    CancelledBeforeSourceDelete { completed: usize, total: usize },
    #[error("WebDAV Move source is partially deleted after {completed} of {total}: {reason}")]
    PartialSourceDelete {
        completed: usize,
        total: usize,
        reason: String,
    },
    #[error("WebDAV Move requires recovery after {completed} of {total} source deletes: {reason}")]
    RecoveryRequired {
        completed: usize,
        total: usize,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VerifiedDestination {
    root: WebDavCollectionRef,
    manifest: WebDavTreeManifest,
}

fn resolve_move_source<'a>(
    selected_names: &[String],
    focused_source: Option<&'a ListedEntry>,
    current_active_listed: &[&'a ListedEntry],
) -> Result<&'a ListedEntry, String> {
    match selected_names {
        [] => focused_source.ok_or_else(|| "Focus a real WebDAV collection to move".to_string()),
        [selected] => {
            let mut matches = current_active_listed
                .iter()
                .copied()
                .filter(|listed| listed.entry.name == *selected);
            let source = matches
                .next()
                .ok_or_else(|| format!("Selected item '{selected}' is no longer listed"))?;
            if matches.next().is_some() {
                return Err(format!(
                    "Selected item '{selected}' is ambiguous in the current listing"
                ));
            }
            Ok(source)
        }
        _ => Err("WebDAV to WebDAV Move currently supports exactly one collection root".into()),
    }
}

/// Freeze the exact one-root WebDAV -> WebDAV Move payload from the ACTIVE
/// current listing. The passive pane contributes only the destination Location;
/// it can never supply source identity.
pub fn prepare_webdav_move_tree(
    src_loc: &Location,
    dst_loc: &Location,
    selected_names: &[String],
    focused_source: Option<&ListedEntry>,
    current_active_listed: &[&ListedEntry],
) -> Result<(WebDavTransferSpec, String), String> {
    let (
        Location::WebDav {
            target: source_target,
            ..
        },
        Location::WebDav {
            target: destination_target,
            path: destination_path,
        },
    ) = (src_loc, dst_loc)
    else {
        return Err("WebDAV Move requires WebDAV source and destination panes".into());
    };

    let source = resolve_move_source(selected_names, focused_source, current_active_listed)?;
    let (EntryKind::Directory, EntryIdentity::WebDavCollection(collection)) =
        (&source.entry.kind, &source.identity)
    else {
        return Err("WebDAV Move requires an exact WebDAV collection identity".into());
    };
    if collection.target != *source_target {
        return Err(format!(
            "WebDAV collection target '{}' does not match source pane target '{}'",
            collection.target, source_target
        ));
    }

    let destination_root = webdav_write_child_target(
        destination_target,
        destination_path,
        &source.entry.name,
    )
    .map_err(|error| error.to_string())?;

    Ok((
        WebDavTransferSpec::MoveTree {
            source: collection.clone(),
            destination_root,
        },
        source.entry.name.clone(),
    ))
}

fn verify_manifest_shape(
    source: &WebDavTreeManifest,
    destination: &WebDavTreeManifest,
) -> io::Result<()> {
    let source_dirs = source
        .directories
        .iter()
        .map(|entry| entry.relative.clone())
        .collect::<BTreeSet<_>>();
    let destination_dirs = destination
        .directories
        .iter()
        .map(|entry| entry.relative.clone())
        .collect::<BTreeSet<_>>();
    if source_dirs != destination_dirs {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "WebDAV Move verification failed: directory manifest differs",
        ));
    }

    let source_files = source
        .files
        .iter()
        .map(|entry| (entry.relative.clone(), entry.advertised_size))
        .collect::<BTreeMap<_, _>>();
    let destination_files = destination
        .files
        .iter()
        .map(|entry| (entry.relative.clone(), entry.advertised_size))
        .collect::<BTreeMap<_, _>>();
    if source_files.len() != destination_files.len()
        || source_files.keys().ne(destination_files.keys())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "WebDAV Move verification failed: file manifest differs",
        ));
    }
    for (relative, source_size) in source_files {
        if let Some(expected) = source_size
            && destination_files.get(&relative).copied().flatten() != Some(expected)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "WebDAV Move verification failed: size differs for {}",
                    relative.display()
                ),
            ));
        }
    }
    Ok(())
}

fn delete_snapshot_matches_tree(
    provider: &WebDavProvider,
    source: &WebDavCollectionRef,
    tree: &WebDavTreeManifest,
    delete: &WebDavDeleteManifest,
) -> io::Result<()> {
    if &delete.root != source {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "WebDAV Move delete snapshot root differs from copied source root",
        ));
    }
    let mut expected = BTreeMap::new();
    for directory in &tree.directories {
        let canonical = provider.canonical_exact_href_identity(
            &directory.source.target,
            &directory.source.href,
        )?;
        expected.insert(canonical, directory.relative.components().count());
    }
    for file in &tree.files {
        let canonical =
            provider.canonical_exact_href_identity(&file.source.target, &file.source.href)?;
        expected.insert(canonical, file.relative.components().count());
    }

    let actual = delete
        .nodes
        .iter()
        .map(|node| {
            let expected_identity = match &node.identity {
                WebDavDeleteIdentity::Object(object) => provider
                    .canonical_exact_href_identity(&object.target, &object.href),
                WebDavDeleteIdentity::Collection(collection) => provider
                    .canonical_exact_href_identity(&collection.target, &collection.href),
            }?;
            if expected_identity != node.canonical_identity {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "WebDAV Move delete snapshot canonical identity mismatch",
                ));
            }
            Ok((node.canonical_identity.clone(), node.depth))
        })
        .collect::<io::Result<BTreeMap<_, _>>>()?;

    if expected != actual {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "WebDAV Move copy/delete snapshots differ before destination mutation",
        ));
    }
    Ok(())
}

async fn destination_snapshot_uninterruptible(
    provider: &WebDavProvider,
    destination_root: &WebDavWriteTarget,
) -> io::Result<VerifiedDestination> {
    let root = provider
        .resolve_logical_collection_exact(&destination_root.logical_path)
        .await?;
    let never_cancel = AtomicBool::new(false);
    let pause = PauseGate::disabled();
    let manifest = build_tree_manifest(provider, &root, &never_cancel, &pause).await?;
    Ok(VerifiedDestination { root, manifest })
}

async fn cleanup_verified_destination(
    provider: &WebDavProvider,
    destination_root: &WebDavWriteTarget,
    verified: &VerifiedDestination,
    original: io::Error,
) -> io::Error {
    let current = match destination_snapshot_uninterruptible(provider, destination_root).await {
        Ok(current) => current,
        Err(error) => {
            return io::Error::other(MoveTreeFailure::DestinationChanged {
                target: destination_root.target.clone(),
                logical_path: destination_root.logical_path.clone(),
                reason: format!(
                    "cannot prove destination is still attempt-owned before cleanup: {error}; original={original}"
                ),
            });
        }
    };
    if &current != verified {
        return io::Error::other(MoveTreeFailure::DestinationChanged {
            target: destination_root.target.clone(),
            logical_path: destination_root.logical_path.clone(),
            reason: format!("destination no longer matches verified snapshot; original={original}"),
        });
    }
    match provider
        .delete_logical_collection(&destination_root.logical_path)
        .await
    {
        Ok(()) => original,
        Err(cleanup) => io::Error::other(MoveTreeFailure::CleanupFailure {
            target: destination_root.target.clone(),
            logical_path: destination_root.logical_path.clone(),
            original: original.to_string(),
            cleanup: cleanup.to_string(),
        }),
    }
}

fn delete_preflight_error(error: WebDavDeleteError) -> io::Error {
    match error {
        WebDavDeleteError::PreMutation { reason } => io::Error::new(
            io::ErrorKind::InvalidData,
            format!("WebDAV Move source delete preflight failed: {reason}"),
        ),
        other => io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unexpected WebDAV Move source delete preflight state: {other}"),
        ),
    }
}

/// Execute one WebDAV -> WebDAV Move as the explicit transaction
/// copy -> independent destination verification -> frozen-source delete.
///
/// Progress uses one stable item unit for the whole attempt: `2 * N` total,
/// where the first N means copied+verified destination items and the second N
/// means definitive source deletes. Byte callbacks from the inner CopyTree are
/// intentionally not published to the queue because the queue correctly rejects
/// unit changes within one attempt.
///
/// The destination stays attempt-owned until the source delete commit boundary.
/// Once any source DELETE succeeds it is never rolled back.
pub(crate) async fn move_tree(
    source_provider: Arc<WebDavProvider>,
    destination_provider: Arc<WebDavProvider>,
    spec: &WebDavTransferSpec,
    cancel: Arc<AtomicBool>,
    pause: PauseGate,
    on_progress: &mut impl FnMut(TypedTransferProgress),
) -> io::Result<usize> {
    let (source, destination_root) = match spec {
        WebDavTransferSpec::MoveTree {
            source,
            destination_root,
        } => (source, destination_root),
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "move_tree requires WebDavTransferSpec::MoveTree",
            ));
        }
    };

    // Freeze both the copy-visible tree and the exact destructive delete plan
    // before any destination mutation. They must describe the same exact source
    // identity set and depths before the copy can start.
    let source_manifest = build_tree_manifest(&source_provider, source, &cancel, &pause).await?;
    if cancel.load(Ordering::Acquire) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "WebDAV Move cancelled before copy",
        ));
    }
    let frozen_delete = MutationService::build_webdav_delete_manifest(&source_provider, source)
        .await
        .map_err(delete_preflight_error)?;
    delete_snapshot_matches_tree(&source_provider, source, &source_manifest, &frozen_delete)?;
    let source_items = 1usize
        .checked_add(source_manifest.descendant_count)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "WebDAV Move item overflow"))?;
    let transaction_total = source_items
        .checked_mul(2)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "WebDAV Move item overflow"))?;

    pause.checkpoint().await;
    if cancel.load(Ordering::Acquire) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "WebDAV Move cancelled before copy",
        ));
    }
    revalidate_tree_manifest(
        &source_provider,
        source,
        &source_manifest,
        &cancel,
        &pause,
    )
    .await?;

    let copy_spec = WebDavTransferSpec::CopyTree {
        source: source.clone(),
        destination_root: destination_root.clone(),
    };
    let copied_items = copy_tree(
        &source_provider,
        &destination_provider,
        &copy_spec,
        cancel.clone(),
        pause.clone(),
        &mut |_| {},
    )
    .await?;

    // copy_tree already performs its accepted independent destination verify.
    // Re-read the destination without honoring user cancellation so Move owns a
    // concrete verified snapshot that can later gate cleanup and source commit.
    let verified_destination =
        destination_snapshot_uninterruptible(&destination_provider, destination_root)
            .await
            .map_err(|error| {
                io::Error::other(MoveTreeFailure::DestinationChanged {
                    target: destination_root.target.clone(),
                    logical_path: destination_root.logical_path.clone(),
                    reason: format!(
                        "cannot establish post-copy destination snapshot for Move: {error}"
                    ),
                })
            })?;
    if copied_items != source_items {
        return Err(io::Error::other(MoveTreeFailure::DestinationChanged {
            target: destination_root.target.clone(),
            logical_path: destination_root.logical_path.clone(),
            reason: format!(
                "copy completed {copied_items} items but frozen Move snapshot contains {source_items}"
            ),
        }));
    }
    if let Err(error) = verify_manifest_shape(&source_manifest, &verified_destination.manifest) {
        return Err(io::Error::other(MoveTreeFailure::DestinationChanged {
            target: destination_root.target.clone(),
            logical_path: destination_root.logical_path.clone(),
            reason: error.to_string(),
        }));
    }

    on_progress(TypedTransferProgress::Items {
        completed: source_items as u64,
        total: Some(transaction_total as u64),
    });

    let cancelled_before_commit = || {
        io::Error::other(MoveTreeFailure::CancelledBeforeSourceDelete {
            completed: source_items,
            total: transaction_total,
        })
    };
    let cleanup_on_predelete = |reason: io::Error| async {
        cleanup_verified_destination(
            &destination_provider,
            destination_root,
            &verified_destination,
            reason,
        )
        .await
    };

    if cancel.load(Ordering::Acquire) {
        return Err(cleanup_on_predelete(cancelled_before_commit()).await);
    }
    pause.checkpoint().await;
    if cancel.load(Ordering::Acquire) {
        return Err(cleanup_on_predelete(cancelled_before_commit()).await);
    }

    // The copied source snapshot must still be current before destructive commit.
    if let Err(error) = revalidate_tree_manifest(
        &source_provider,
        source,
        &source_manifest,
        &cancel,
        &pause,
    )
    .await
    {
        return Err(cleanup_on_predelete(error).await);
    }
    if cancel.load(Ordering::Acquire) {
        return Err(cleanup_on_predelete(cancelled_before_commit()).await);
    }

    // Final destination proof immediately before handing control to the exact
    // frozen-source delete authority. Any destination drift is RecoveryRequired;
    // never remove an unproven tree and never touch source in that state.
    let current_destination =
        match destination_snapshot_uninterruptible(&destination_provider, destination_root).await {
            Ok(current) => current,
            Err(error) => {
                return Err(io::Error::other(MoveTreeFailure::DestinationChanged {
                    target: destination_root.target.clone(),
                    logical_path: destination_root.logical_path.clone(),
                    reason: format!("cannot revalidate destination before source delete: {error}"),
                }));
            }
        };
    if current_destination != verified_destination {
        return Err(io::Error::other(MoveTreeFailure::DestinationChanged {
            target: destination_root.target.clone(),
            logical_path: destination_root.logical_path.clone(),
            reason: "destination changed after verification".into(),
        }));
    }
    if cancel.load(Ordering::Acquire) {
        return Err(cleanup_on_predelete(cancelled_before_commit()).await);
    }

    let delete_result = MutationService::delete_webdav_tree_from_frozen_manifest(
        source_provider,
        frozen_delete,
        cancel,
        |progress| {
            on_progress(TypedTransferProgress::Items {
                completed: (source_items + progress.completed) as u64,
                total: Some(transaction_total as u64),
            });
        },
    )
    .await;

    match delete_result {
        Ok(outcome) => {
            debug_assert_eq!(outcome.total, source_items);
            Ok(transaction_total)
        }
        Err(WebDavDeleteError::PreMutation { reason }) => {
            Err(cleanup_on_predelete(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("WebDAV Move source changed before first DELETE: {reason}"),
            ))
            .await)
        }
        Err(WebDavDeleteError::Cancelled {
            completed: 0,
            total: _,
        }) => Err(cleanup_on_predelete(cancelled_before_commit()).await),
        Err(WebDavDeleteError::Cancelled { completed, total }) => {
            Err(io::Error::other(MoveTreeFailure::PartialSourceDelete {
                completed,
                total,
                reason: "cancelled after source deletion started".into(),
            }))
        }
        Err(WebDavDeleteError::Partial {
            completed,
            total,
            reason,
        }) => Err(io::Error::other(MoveTreeFailure::PartialSourceDelete {
            completed,
            total,
            reason,
        })),
        Err(WebDavDeleteError::RecoveryRequired {
            completed,
            total,
            reason,
        }) => Err(io::Error::other(MoveTreeFailure::RecoveryRequired {
            completed,
            total,
            reason,
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::{Entry, WebDavCollectionRef, WebDavObjectRef};

    fn dav_location(target: &str, path: &str) -> Location {
        Location::WebDav {
            target: target.into(),
            path: path.into(),
        }
    }

    fn dav_dir(name: &str, target: &str, href: &str) -> ListedEntry {
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

    fn dav_file(name: &str, target: &str, href: &str) -> ListedEntry {
        ListedEntry {
            entry: Entry {
                name: name.into(),
                kind: EntryKind::File,
                size: Some(1),
                modified_unix_ms: None,
            },
            identity: EntryIdentity::WebDavObject(WebDavObjectRef {
                target: target.into(),
                href: href.into(),
            }),
        }
    }

    #[test]
    fn focus_freezes_exact_raw_href_and_cross_target_destination() {
        let source = dav_dir("unicodé root", "src", "/dav/raw%20root/?rev=7");
        let (spec, name) = prepare_webdav_move_tree(
            &dav_location("src", "/presentation/ignored"),
            &dav_location("dst", "/archive"),
            &[],
            Some(&source),
            &[&source],
        )
        .unwrap();
        assert_eq!(name, "unicodé root");
        assert!(matches!(
            spec,
            WebDavTransferSpec::MoveTree { source, destination_root }
                if source.target == "src"
                    && source.href == "/dav/raw%20root/?rev=7"
                    && destination_root.target == "dst"
                    && destination_root.logical_path == "/archive/unicodé root"
        ));
    }

    #[test]
    fn one_selection_wins_over_focus_and_same_target_is_supported() {
        let selected = dav_dir("selected", "dav", "/native/selected/");
        let focused = dav_dir("focused", "dav", "/native/focused/");
        let (spec, name) = prepare_webdav_move_tree(
            &dav_location("dav", "/src"),
            &dav_location("dav", "/dst"),
            &["selected".into()],
            Some(&focused),
            &[&focused, &selected],
        )
        .unwrap();
        assert_eq!(name, "selected");
        assert!(matches!(
            spec,
            WebDavTransferSpec::MoveTree { source, destination_root }
                if source.href == "/native/selected/"
                    && destination_root.target == "dav"
                    && destination_root.logical_path == "/dst/selected"
        ));
    }

    #[test]
    fn stale_ambiguous_multi_file_and_target_mismatch_fail_closed() {
        let focused = dav_dir("focused", "src", "/focused/");
        assert!(
            prepare_webdav_move_tree(
                &dav_location("src", "/"),
                &dav_location("dst", "/"),
                &["missing".into()],
                Some(&focused),
                &[&focused],
            )
            .unwrap_err()
            .contains("no longer listed")
        );

        let dup_a = dav_dir("dup", "src", "/a/");
        let dup_b = dav_dir("dup", "src", "/b/");
        assert!(
            prepare_webdav_move_tree(
                &dav_location("src", "/"),
                &dav_location("dst", "/"),
                &["dup".into()],
                None,
                &[&dup_a, &dup_b],
            )
            .unwrap_err()
            .contains("ambiguous")
        );

        assert!(
            prepare_webdav_move_tree(
                &dav_location("src", "/"),
                &dav_location("dst", "/"),
                &["a".into(), "b".into()],
                None,
                &[&dup_a, &dup_b],
            )
            .unwrap_err()
            .contains("exactly one collection root")
        );

        let file = dav_file("file", "src", "/file");
        assert!(
            prepare_webdav_move_tree(
                &dav_location("src", "/"),
                &dav_location("dst", "/"),
                &[],
                Some(&file),
                &[&file],
            )
            .unwrap_err()
            .contains("exact WebDAV collection")
        );

        let wrong = dav_dir("wrong", "other", "/wrong/");
        assert!(
            prepare_webdav_move_tree(
                &dav_location("src", "/"),
                &dav_location("dst", "/"),
                &[],
                Some(&wrong),
                &[&wrong],
            )
            .unwrap_err()
            .contains("does not match")
        );
    }

    #[test]
    fn manifest_shape_matches_copy_verification_semantics() {
        let source = WebDavTreeManifest {
            directories: vec![],
            files: vec![],
            descendant_count: 0,
            total_bytes: Some(0),
        };
        assert!(verify_manifest_shape(&source, &source).is_ok());
    }
}
