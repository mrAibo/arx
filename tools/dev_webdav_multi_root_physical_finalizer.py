from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one replacement, found {count}")
    p.write_text(text.replace(old, new, 1))


runtime_helpers = r'''
async fn run_f5_batch_runtime(
    registry: &Arc<ProviderRegistry>,
    src_loc: Location,
    dst_loc: Location,
    selected_names: &[String],
    current_active_listed: &[ListedEntry],
) -> Result<Vec<crate::jobs::JobEvent>, String> {
    let src_provider = src_loc.provider_id();
    let dst_provider = dst_loc.provider_id();
    let src_caps = registry
        .capabilities_for_location(&src_loc)
        .unwrap_or_default();
    let dst_caps = registry
        .capabilities_for_location(&dst_loc)
        .unwrap_or_default();
    let current_refs = current_active_listed.iter().collect::<Vec<_>>();
    let (webdav_spec, names) = crate::transfer::webdav_batch::prepare_webdav_copy_batch(
        &src_loc,
        &dst_loc,
        selected_names,
        None,
        &current_refs,
    )?;
    let mut executors = ExecutorAvailability::local();
    executors.webdav = true;
    let plan = TransferPlanner::plan(TransferRequest {
        source: src_loc,
        destination: dst_loc,
        source_provider: src_provider,
        destination_provider: dst_provider,
        source_capabilities: src_caps,
        destination_capabilities: dst_caps,
        intent: TransferIntent::Copy,
        executors,
        delete_extraneous: false,
        s3_spec: None,
        webdav_spec: Some(webdav_spec),
    })
    .map_err(|e| e.to_string())?;

    let manager = crate::jobs::JobManager::new();
    let (events, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let runtime = crate::transfer_queue_runtime::TransferQueueRuntime::new(
        manager,
        events,
        registry.as_ref().clone(),
        crate::transfer_queue::TransferQueueConfig::default(),
    );
    let job_id = runtime.enqueue(plan, names).map_err(|e| e.to_string())?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(90);
    let mut observed = Vec::new();
    loop {
        let event = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .map_err(|_| format!("timed out waiting for multi-root job {job_id}"))?
            .ok_or_else(|| format!("event stream closed for multi-root job {job_id}"))?;
        if event.id() != job_id {
            continue;
        }
        let terminal = event.is_terminal();
        observed.push(event);
        if terminal {
            break;
        }
    }
    Ok(observed)
}

fn assert_batch_completed(events: &[crate::jobs::JobEvent], total: usize) {
    let progress = events
        .iter()
        .filter_map(|event| match event {
            crate::jobs::JobEvent::Progress {
                progress:
                    crate::jobs::JobProgress::Generic(crate::jobs::Progress::Items { done, total }),
                ..
            } => Some((*done, *total)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let expected = (1..=total).map(|done| (done, total)).collect::<Vec<_>>();
    assert_eq!(progress, expected, "root progress must be stable and monotonic");
    match events.last() {
        Some(crate::jobs::JobEvent::Completed {
            result:
                crate::jobs::JobResult::Generic {
                    completed_items: Some(completed),
                    ..
                },
            ..
        }) => assert_eq!(*completed, total),
        other => panic!("expected completed multi-root job, got {other:?}"),
    }
}
'''

apache_anchor = "    Ok(())\n}\n\n// ponytail: one combined physical suite that walks the canonical W1–W18 matrix\n"
replace_once(
    "src/vfs/webdav_acceptance.rs",
    apache_anchor,
    "    Ok(())\n}\n" + runtime_helpers + "\n// ponytail: one combined physical suite that walks the canonical W1–W18 matrix\n",
)

apache_test = r'''
#[tokio::test(flavor = "multi_thread")]
async fn physical_webdav_multi_root_copy() {
    let Some(provider) = target() else {
        eprintln!("skipping multi-root WebDAV acceptance: no fixture env");
        return;
    };
    let provider = Arc::new(provider);
    let registry = registry_with(&provider);
    let target_id = provider.target().id.clone();
    let run = physical_run_id();

    let upload_parent = format!("/{run}-multi-upload");
    provider.mkdir(&upload_parent).await.expect("multi upload parent");
    provider
        .write_file_bytes_if_unchanged(
            &format!("{upload_parent}/sentinel.txt"),
            b"keep",
            &RemoteEditRevision::new(vec![], 0, 0, 0),
            &CancellationFlag::default(),
            None,
        )
        .await
        .expect("multi upload sentinel");
    let local = tempfile::tempdir().unwrap();
    let file_name = format!("{run}-zéro file.txt");
    let tree_name = format!("{run}-nested root");
    let empty_name = format!("{run}-empty dir");
    std::fs::write(local.path().join(&file_name), b"").unwrap();
    std::fs::create_dir(local.path().join(&tree_name)).unwrap();
    std::fs::create_dir(local.path().join(&tree_name).join("unicodé child")).unwrap();
    std::fs::write(
        local.path().join(&tree_name).join("unicodé child").join("file name.txt"),
        b"nested-bytes",
    )
    .unwrap();
    std::fs::create_dir(local.path().join(&empty_name)).unwrap();
    let local_rows = vec![
        ListedEntry {
            entry: crate::vfs::Entry {
                name: tree_name.clone(),
                kind: EntryKind::Directory,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::Other,
        },
        ListedEntry {
            entry: crate::vfs::Entry {
                name: file_name.clone(),
                kind: EntryKind::File,
                size: Some(0),
                modified_unix_ms: None,
            },
            identity: EntryIdentity::Other,
        },
        ListedEntry {
            entry: crate::vfs::Entry {
                name: empty_name.clone(),
                kind: EntryKind::Directory,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::Other,
        },
    ];
    let selected = vec![file_name.clone(), empty_name.clone(), tree_name.clone()];
    let upload_events = run_f5_batch_runtime(
        &registry,
        Location::Local(local.path().to_path_buf()),
        Location::WebDav {
            target: target_id.clone(),
            path: upload_parent.clone(),
        },
        &selected,
        &local_rows,
    )
    .await
    .expect("multi-root Local -> WebDAV product path");
    assert_batch_completed(&upload_events, 3);
    assert_eq!(
        provider
            .read_all_capped(&format!("{upload_parent}/{file_name}"), 16)
            .await
            .unwrap()
            .bytes,
        b""
    );
    assert_eq!(
        provider
            .read_all_capped(
                &format!("{upload_parent}/{tree_name}/unicodé child/file name.txt"),
                64,
            )
            .await
            .unwrap()
            .bytes,
        b"nested-bytes"
    );
    provider
        .list_page(
            &Location::WebDav {
                target: target_id.clone(),
                path: format!("{upload_parent}/{empty_name}"),
            },
            None,
        )
        .await
        .expect("empty uploaded root exists");
    assert_eq!(
        provider
            .read_all_capped(&format!("{upload_parent}/sentinel.txt"), 16)
            .await
            .unwrap()
            .bytes,
        b"keep"
    );

    let download_parent = format!("/{run}-multi-download");
    provider.mkdir(&download_parent).await.expect("multi download parent");
    let object_name = format!("{run}-remote zero.txt");
    let collection_name = format!("{run}-remote tree");
    let empty_remote = format!("{run}-remote empty");
    provider
        .write_file_bytes_if_unchanged(
            &format!("{download_parent}/{object_name}"),
            b"",
            &RemoteEditRevision::new(vec![], 0, 0, 0),
            &CancellationFlag::default(),
            None,
        )
        .await
        .unwrap();
    provider
        .mkdir(&format!("{download_parent}/{collection_name}"))
        .await
        .unwrap();
    provider
        .mkdir(&format!("{download_parent}/{collection_name}/unicodé child"))
        .await
        .unwrap();
    provider
        .write_file_bytes_if_unchanged(
            &format!("{download_parent}/{collection_name}/unicodé child/file name.txt"),
            b"remote-nested",
            &RemoteEditRevision::new(vec![], 0, 0, 0),
            &CancellationFlag::default(),
            None,
        )
        .await
        .unwrap();
    provider
        .mkdir(&format!("{download_parent}/{empty_remote}"))
        .await
        .unwrap();
    let remote_location = Location::WebDav {
        target: target_id.clone(),
        path: download_parent.clone(),
    };
    let remote_rows = provider
        .list_page(&remote_location, None)
        .await
        .expect("real multi-root source listing")
        .entries;
    let download_selected = vec![empty_remote.clone(), object_name.clone(), collection_name.clone()];
    let destination = tempfile::tempdir().unwrap();
    std::fs::write(destination.path().join("sentinel-local"), b"keep-local").unwrap();
    let download_events = run_f5_batch_runtime(
        &registry,
        remote_location,
        Location::Local(destination.path().to_path_buf()),
        &download_selected,
        &remote_rows,
    )
    .await
    .expect("multi-root WebDAV -> Local product path");
    assert_batch_completed(&download_events, 3);
    assert_eq!(std::fs::metadata(destination.path().join(&object_name)).unwrap().len(), 0);
    assert_eq!(
        std::fs::read(
            destination.path().join(&collection_name).join("unicodé child").join("file name.txt")
        )
        .unwrap(),
        b"remote-nested"
    );
    assert!(destination.path().join(&empty_remote).is_dir());
    assert_eq!(std::fs::read(destination.path().join("sentinel-local")).unwrap(), b"keep-local");

    let cancel_dest = tempfile::tempdir().unwrap();
    let current_refs = remote_rows.iter().collect::<Vec<_>>();
    let (cancel_spec, cancel_names) = crate::transfer::webdav_batch::prepare_webdav_copy_batch(
        &Location::WebDav {
            target: target_id.clone(),
            path: download_parent.clone(),
        },
        &Location::Local(cancel_dest.path().to_path_buf()),
        &download_selected,
        None,
        &current_refs,
    )
    .unwrap();
    let mut executors = ExecutorAvailability::local();
    executors.webdav = true;
    let cancel_source = Location::WebDav {
        target: target_id.clone(),
        path: download_parent,
    };
    let cancel_destination = Location::Local(cancel_dest.path().to_path_buf());
    let cancel_plan = TransferPlanner::plan(TransferRequest {
        source: cancel_source.clone(),
        destination: cancel_destination.clone(),
        source_provider: cancel_source.provider_id(),
        destination_provider: cancel_destination.provider_id(),
        source_capabilities: registry
            .capabilities_for_location(&cancel_source)
            .unwrap_or_default(),
        destination_capabilities: registry
            .capabilities_for_location(&cancel_destination)
            .unwrap_or_default(),
        intent: TransferIntent::Copy,
        executors,
        delete_extraneous: false,
        s3_spec: None,
        webdav_spec: Some(cancel_spec),
    })
    .unwrap();
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let set_cancel = cancel.clone();
    let error = crate::transfer::executor::execute_transfer(
        &cancel_plan,
        &cancel_names,
        &registry,
        cancel,
        crate::transfer_queue::PauseGate::disabled(),
        move |progress| {
            if matches!(
                progress,
                crate::transfer_queue::TypedTransferProgress::Items { completed: 1, .. }
            ) {
                set_cancel.store(true, std::sync::atomic::Ordering::Release);
            }
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        crate::transfer::executor::TransferExecutionError::Cancelled { completed: 1 }
    ));
    assert_eq!(
        std::fs::read_dir(cancel_dest.path()).unwrap().count(),
        1,
        "exactly one completed root must remain after boundary cancellation"
    );
}

'''
replace_once(
    "src/vfs/webdav_acceptance.rs",
    "/// Join the fixture DAV root and a resource path with exactly one slash.\n",
    apache_test + "/// Join the fixture DAV root and a resource path with exactly one slash.\n",
)

interop_anchor = "    Ok(())\n}\n\nasync fn list_at(p: &WebDavProvider, target_id: &str, path: &str) -> Vec<ListedEntry> {\n"
replace_once(
    "src/vfs/webdav_interop_acceptance.rs",
    interop_anchor,
    "    Ok(())\n}\n" + runtime_helpers + "\nasync fn list_at(p: &WebDavProvider, target_id: &str, path: &str) -> Vec<ListedEntry> {\n",
)

interop_test = r'''
#[tokio::test(flavor = "multi_thread")]
async fn physical_webdav_interop_multi_root_copy() {
    let host_ok = std::env::var("ARX_WEBDAV_SMOKE_HOST").is_ok();
    let required = std::env::var("ARX_WEBDAV_INTEROP_REQUIRED").ok().as_deref() == Some("1");
    if !host_ok && !required {
        eprintln!("skipping multi-root interop: no fixture env");
        return;
    }
    let kind = std::env::var("ARX_WEBDAV_INTEROP_KIND").unwrap_or_default();
    assert!(matches!(kind.as_str(), "nextcloud" | "owncloud"));
    let provider = Arc::new(target_direct());
    let registry = registry_with(&provider);
    let target_id = provider.target().id.clone();
    let run = physical_run_id();

    let upload_parent = format!("/{run}-multi-upload");
    provider.mkdir(&upload_parent).await.unwrap();
    let local = tempfile::tempdir().unwrap();
    let file_name = format!("{run}-zéro file.txt");
    let tree_name = format!("{run}-tree root");
    let empty_name = format!("{run}-empty root");
    std::fs::write(local.path().join(&file_name), b"").unwrap();
    std::fs::create_dir(local.path().join(&tree_name)).unwrap();
    std::fs::create_dir(local.path().join(&tree_name).join("nested space")).unwrap();
    std::fs::write(local.path().join(&tree_name).join("nested space/uñicode.txt"), b"portable").unwrap();
    std::fs::create_dir(local.path().join(&empty_name)).unwrap();
    let local_rows = vec![
        ListedEntry {
            entry: crate::vfs::Entry {
                name: file_name.clone(),
                kind: EntryKind::File,
                size: Some(0),
                modified_unix_ms: None,
            },
            identity: EntryIdentity::Other,
        },
        ListedEntry {
            entry: crate::vfs::Entry {
                name: tree_name.clone(),
                kind: EntryKind::Directory,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::Other,
        },
        ListedEntry {
            entry: crate::vfs::Entry {
                name: empty_name.clone(),
                kind: EntryKind::Directory,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::Other,
        },
    ];
    let selected = vec![empty_name.clone(), tree_name.clone(), file_name.clone()];
    let events = run_f5_batch_runtime(
        &registry,
        Location::Local(local.path().to_path_buf()),
        Location::WebDav {
            target: target_id.clone(),
            path: upload_parent.clone(),
        },
        &selected,
        &local_rows,
    )
    .await
    .unwrap();
    assert_batch_completed(&events, 3);
    assert_eq!(
        provider
            .read_all_capped(&format!("{upload_parent}/{file_name}"), 16)
            .await
            .unwrap()
            .bytes,
        b""
    );
    assert_eq!(
        provider
            .read_all_capped(&format!("{upload_parent}/{tree_name}/nested space/uñicode.txt"), 64)
            .await
            .unwrap()
            .bytes,
        b"portable"
    );
    provider
        .list_page(
            &Location::WebDav {
                target: target_id.clone(),
                path: format!("{upload_parent}/{empty_name}"),
            },
            None,
        )
        .await
        .unwrap();

    let download_parent = format!("/{run}-multi-download");
    provider.mkdir(&download_parent).await.unwrap();
    let object_name = format!("{run}-remote zero.txt");
    let collection_name = format!("{run}-remote tree");
    let empty_remote = format!("{run}-remote empty");
    provider
        .write_file_bytes_if_unchanged(
            &format!("{download_parent}/{object_name}"),
            b"",
            &RemoteEditRevision::new(vec![], 0, 0, 0),
            &CancellationFlag::default(),
            None,
        )
        .await
        .unwrap();
    provider.mkdir(&format!("{download_parent}/{collection_name}")).await.unwrap();
    provider
        .write_file_bytes_if_unchanged(
            &format!("{download_parent}/{collection_name}/file name.txt"),
            b"portable-down",
            &RemoteEditRevision::new(vec![], 0, 0, 0),
            &CancellationFlag::default(),
            None,
        )
        .await
        .unwrap();
    provider.mkdir(&format!("{download_parent}/{empty_remote}")).await.unwrap();
    let remote_location = Location::WebDav {
        target: target_id,
        path: download_parent,
    };
    let rows = provider.list_page(&remote_location, None).await.unwrap().entries;
    let download_selected = vec![collection_name.clone(), empty_remote.clone(), object_name.clone()];
    let destination = tempfile::tempdir().unwrap();
    let events = run_f5_batch_runtime(
        &registry,
        remote_location,
        Location::Local(destination.path().to_path_buf()),
        &download_selected,
        &rows,
    )
    .await
    .unwrap();
    assert_batch_completed(&events, 3);
    assert_eq!(std::fs::metadata(destination.path().join(&object_name)).unwrap().len(), 0);
    assert_eq!(
        std::fs::read(destination.path().join(&collection_name).join("file name.txt")).unwrap(),
        b"portable-down"
    );
    assert!(destination.path().join(&empty_remote).is_dir());
    println!("multi-root F5 PASS ({kind})");
}

'''
replace_once(
    "src/vfs/webdav_interop_acceptance.rs",
    '#[tokio::test(flavor = "multi_thread")]\nasync fn physical_webdav_interop_recursive_delete_tree() {\n',
    interop_test + '#[tokio::test(flavor = "multi_thread")]\nasync fn physical_webdav_interop_recursive_delete_tree() {\n',
)

replace_once(
    ".github/workflows/ci.yml",
    "          cargo test --locked --lib --features physical-webdav physical_webdav_recursive_delete_tree -- --nocapture\n          cargo test --locked --test webdav_recursive_delete_safety --features physical-webdav -- --nocapture --test-threads=1\n",
    "          cargo test --locked --lib --features physical-webdav physical_webdav_recursive_delete_tree -- --nocapture\n          cargo test --locked --lib --features physical-webdav physical_webdav_multi_root_copy -- --nocapture\n          cargo test --locked --test webdav_recursive_delete_safety --features physical-webdav -- --nocapture --test-threads=1\n",
)
replace_once(
    ".github/workflows/webdav-interop.yml",
    "          cargo test --locked --lib --features physical-webdav \\\n            physical_webdav_interop_recursive_delete_tree -- --nocapture\n",
    "          cargo test --locked --lib --features physical-webdav \\\n            physical_webdav_interop_recursive_delete_tree -- --nocapture\n          cargo test --locked --lib --features physical-webdav \\\n            physical_webdav_interop_multi_root_copy -- --nocapture\n",
)
