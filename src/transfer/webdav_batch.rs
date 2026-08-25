use std::collections::HashSet;
use std::io;

use crate::vfs::{ListedEntry, Location};

use super::{WebDavTransferSpec, build_webdav_copy_spec};

fn resolve_webdav_copy_sources<'a>(
    selected_names: &[String],
    focused_source: Option<&'a ListedEntry>,
    current_active_listed: &[&'a ListedEntry],
) -> Result<Vec<&'a ListedEntry>, String> {
    if selected_names.is_empty() {
        return focused_source
            .map(|source| vec![source])
            .ok_or_else(|| "Focus a real file or directory to copy".to_string());
    }

    let selected: HashSet<&str> = selected_names.iter().map(String::as_str).collect();
    if selected.len() != selected_names.len() {
        return Err("WebDAV copy selection contains duplicate names".into());
    }

    for name in selected_names {
        let count = current_active_listed
            .iter()
            .filter(|listed| listed.entry.name == *name)
            .count();
        match count {
            0 => return Err(format!("Selected item '{name}' is no longer listed")),
            1 => {}
            _ => {
                return Err(format!(
                    "Selected item '{name}' is ambiguous in the current listing"
                ));
            }
        }
    }

    let sources = current_active_listed
        .iter()
        .copied()
        .filter(|listed| selected.contains(listed.entry.name.as_str()))
        .collect::<Vec<_>>();
    if sources.len() != selected_names.len() {
        return Err("WebDAV copy selection no longer matches the active listing".into());
    }
    Ok(sources)
}

fn reject_known_local_collision(spec: &WebDavTransferSpec) -> Result<(), String> {
    let path = match spec {
        WebDavTransferSpec::DownloadOne {
            local_destination, ..
        }
        | WebDavTransferSpec::DownloadTree {
            local_destination, ..
        } => local_destination,
        _ => return Ok(()),
    };

    match std::fs::symlink_metadata(path) {
        Ok(_) => Err(format!(
            "Local destination already exists: {}",
            path.display()
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "Cannot inspect Local destination {}: {error}",
            path.display()
        )),
    }
}

pub fn build_webdav_batch_spec(
    items: Vec<WebDavTransferSpec>,
) -> Result<WebDavTransferSpec, String> {
    if items.len() < 2 {
        return Err("WebDAV batch copy requires at least two roots".into());
    }
    if items
        .iter()
        .any(|item| matches!(item, WebDavTransferSpec::Batch { .. }))
    {
        return Err("nested WebDAV transfer batches are not supported".into());
    }

    let target = items[0].target().to_string();
    if items.iter().any(|item| item.target() != target) {
        return Err("all WebDAV batch roots must use the same target".into());
    }

    Ok(WebDavTransferSpec::Batch { target, items })
}

/// Canonical F5 preparation seam for one or many current ACTIVE roots.
///
/// Zero selection preserves focused single-root behavior. One selected root
/// stays a single-root spec, so its existing progress/retry semantics are
/// unchanged. Two or more selected roots become one sequential frozen Batch.
pub fn prepare_webdav_copy_batch(
    src_loc: &Location,
    dst_loc: &Location,
    selected_names: &[String],
    focused_source: Option<&ListedEntry>,
    current_active_listed: &[&ListedEntry],
) -> Result<(WebDavTransferSpec, Vec<String>), String> {
    let sources = resolve_webdav_copy_sources(
        selected_names,
        focused_source,
        current_active_listed,
    )?;

    let mut specs = Vec::with_capacity(sources.len());
    let mut names = Vec::with_capacity(sources.len());
    for source in sources {
        specs.push(build_webdav_copy_spec(src_loc, dst_loc, source)?);
        names.push(source.entry.name.clone());
    }

    match specs.len() {
        0 => Err("Select a real file or directory to copy".into()),
        1 => Ok((specs.pop().expect("one WebDAV copy spec"), names)),
        _ => {
            // Multi-root WebDAV -> Local can prove obvious collisions before a
            // job is queued. Runtime noclobber still remains authoritative for
            // races that happen after preparation.
            for spec in &specs {
                reject_known_local_collision(spec)?;
            }
            Ok((build_webdav_batch_spec(specs)?, names))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::{
        Entry, EntryIdentity, EntryKind, WebDavCollectionRef, WebDavObjectRef,
    };
    use std::path::PathBuf;

    fn local_file(name: &str) -> ListedEntry {
        ListedEntry {
            entry: Entry {
                name: name.into(),
                kind: EntryKind::File,
                size: Some(1),
                modified_unix_ms: None,
            },
            identity: EntryIdentity::Other,
        }
    }

    fn local_dir(name: &str) -> ListedEntry {
        ListedEntry {
            entry: Entry {
                name: name.into(),
                kind: EntryKind::Directory,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::Other,
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

    fn dav_location(target: &str, path: &str) -> Location {
        Location::WebDav {
            target: target.into(),
            path: path.into(),
        }
    }

    #[test]
    fn no_selection_preserves_focused_single_root() {
        let file = local_file("a.txt");
        let (spec, names) = prepare_webdav_copy_batch(
            &Location::Local(PathBuf::from("/src")),
            &dav_location("dav", "/dst/"),
            &[],
            Some(&file),
            &[&file],
        )
        .unwrap();
        assert_eq!(names, vec!["a.txt"]);
        assert!(matches!(spec, WebDavTransferSpec::UploadOne { .. }));
    }

    #[test]
    fn multi_upload_uses_active_listing_order_and_mixed_kinds() {
        let file = local_file("b.txt");
        let dir = local_dir("a-dir");
        let ignored = local_file("z.txt");
        let (spec, names) = prepare_webdav_copy_batch(
            &Location::Local(PathBuf::from("/src")),
            &dav_location("dav", "/dst/"),
            &["a-dir".into(), "b.txt".into()],
            Some(&ignored),
            &[&file, &ignored, &dir],
        )
        .unwrap();
        assert_eq!(names, vec!["b.txt", "a-dir"]);
        let WebDavTransferSpec::Batch { target, items } = spec else {
            panic!("expected batch");
        };
        assert_eq!(target, "dav");
        assert!(matches!(items[0], WebDavTransferSpec::UploadOne { .. }));
        assert!(matches!(items[1], WebDavTransferSpec::UploadTree { .. }));
    }

    #[test]
    fn multi_download_preserves_exact_native_refs() {
        let object = dav_file("file.txt", "dav", "/native/f%20x?rev=1");
        let collection = dav_dir("tree", "dav", "/native/tree%2Fraw/?rev=2");
        let destination = tempfile::tempdir().unwrap();
        let (spec, names) = prepare_webdav_copy_batch(
            &dav_location("dav", "/presentation/ignored"),
            &Location::Local(destination.path().to_path_buf()),
            &["tree".into(), "file.txt".into()],
            None,
            &[&object, &collection],
        )
        .unwrap();
        assert_eq!(names, vec!["file.txt", "tree"]);
        let WebDavTransferSpec::Batch { items, .. } = spec else {
            panic!("expected batch");
        };
        assert!(matches!(
            &items[0],
            WebDavTransferSpec::DownloadOne { source, .. }
                if source.href == "/native/f%20x?rev=1"
        ));
        assert!(matches!(
            &items[1],
            WebDavTransferSpec::DownloadTree { source, .. }
                if source.href == "/native/tree%2Fraw/?rev=2"
        ));
    }

    #[test]
    fn stale_and_ambiguous_selection_fail_closed() {
        let current = local_file("current");
        assert!(
            prepare_webdav_copy_batch(
                &Location::Local(PathBuf::from("/src")),
                &dav_location("dav", "/dst"),
                &["missing".into()],
                Some(&current),
                &[&current],
            )
            .unwrap_err()
            .contains("no longer listed")
        );

        let one = dav_file("dup", "dav", "/one");
        let two = dav_file("dup", "dav", "/two");
        assert!(
            prepare_webdav_copy_batch(
                &dav_location("dav", "/"),
                &Location::Local(PathBuf::from("/dst")),
                &["dup".into()],
                None,
                &[&one, &two],
            )
            .unwrap_err()
            .contains("ambiguous")
        );
    }

    #[test]
    fn target_mismatch_and_nested_or_mixed_batch_fail() {
        let wrong = dav_file("a", "other", "/a");
        assert!(
            prepare_webdav_copy_batch(
                &dav_location("dav", "/"),
                &Location::Local(PathBuf::from("/dst")),
                &[],
                Some(&wrong),
                &[&wrong],
            )
            .unwrap_err()
            .contains("does not match")
        );

        let one = WebDavTransferSpec::DownloadOne {
            source: WebDavObjectRef {
                target: "a".into(),
                href: "/one".into(),
            },
            local_destination: PathBuf::from("/dst/one"),
        };
        let two = WebDavTransferSpec::DownloadOne {
            source: WebDavObjectRef {
                target: "b".into(),
                href: "/two".into(),
            },
            local_destination: PathBuf::from("/dst/two"),
        };
        assert!(build_webdav_batch_spec(vec![]).is_err());
        assert!(build_webdav_batch_spec(vec![one.clone()]).is_err());
        assert!(build_webdav_batch_spec(vec![one.clone(), two]).is_err());

        let valid = build_webdav_batch_spec(vec![one.clone(), one.clone()]).unwrap();
        assert!(build_webdav_batch_spec(vec![one, valid]).is_err());
    }

    #[test]
    fn known_local_collision_fails_before_enqueue() {
        let first = dav_file("a", "dav", "/a");
        let second = dav_dir("b", "dav", "/b/");
        let destination = tempfile::tempdir().unwrap();
        std::fs::write(destination.path().join("a"), b"existing").unwrap();
        let error = prepare_webdav_copy_batch(
            &dav_location("dav", "/"),
            &Location::Local(destination.path().to_path_buf()),
            &["a".into(), "b".into()],
            None,
            &[&first, &second],
        )
        .unwrap_err();
        assert!(error.contains("already exists"));
    }
}
