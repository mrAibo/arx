use crate::vfs::{EntryIdentity, EntryKind, ListedEntry, Location};

use super::{WebDavTransferSpec, webdav_write_child_target};

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
}
