//! #241 — WebDAV interoperability certification against REAL pinned
//! Nextcloud and ownCloud servers.
//!
//! One physical test (`physical_webdav_interop_core_matrix`) executes the
//! portable I1–I12 matrix against whatever fixture `setup_webdav_interop.sh`
//! provisioned (kind recorded in `ARX_WEBDAV_INTEROP_KIND`). The SAME logic
//! runs for both servers — no per-server fake behavior.
//!
//! Contract:
//! - With `ARX_WEBDAV_INTEROP_REQUIRED=1`, missing fixture env is a FAILURE,
//!   never a silent skip.
//! - W1-style production resolver evidence is mandatory (WebDavTargetConfig →
//!   register_webdav_targets → keyring/env secret resolution →
//!   resolve_webdav_provider → authenticated PROPFIND).
//! - F5 items go through the real product path
//!   (`prepare_webdav_copy` → planner → executor), never direct PUT.
//! - Apache-specific LOCK/proxy/fault behaviors stay in webdav_acceptance.rs.
use super::webdav::{WebDavProvider, WebDavTarget};
use crate::transfer::{ExecutorAvailability, TransferIntent, TransferPlanner, TransferRequest};
use crate::vfs::{
    CancellationFlag, EntryIdentity, EntryKind, ListedEntry, Location, ProviderRegistry,
    RemoteEditRevision, VfsProvider,
};
use std::sync::Arc;

fn required_env(name: &str) -> String {
    match std::env::var(name) {
        Ok(v) if !v.is_empty() => v,
        _ => panic!(
            "#241: required interop fixture env {name} missing — \
             run scripts/setup_webdav_interop.sh <nextcloud|owncloud>; \
             with ARX_WEBDAV_INTEROP_REQUIRED=1 a missing fixture is a FAILURE"
        ),
    }
}

fn physical_run_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    format!("interop-{}-{}", std::process::id(), nanos)
}

fn target_direct() -> WebDavProvider {
    WebDavProvider::new(
        WebDavTarget {
            id: "accept".into(),
            name: "interop".into(),
            url: required_env("ARX_WEBDAV_SMOKE_HOST"),
            username: required_env("ARX_WEBDAV_SMOKE_USER"),
            auth: "basic".into(),
        },
        required_env("ARX_WEBDAV_SMOKE_PASS"),
    )
    .expect("direct provider construction")
}

/// I1 production-chain evidence: config → registration → env secret
/// resolution → resolve → authenticated PROPFIND. No interop-only adapter.
/// Target id `accept` maps the resolver to `ARX_WEBDAV_ACCEPT_PASSWORD`.
fn target_via_production_resolver() -> Arc<dyn VfsProvider> {
    let host = required_env("ARX_WEBDAV_SMOKE_HOST");
    let user = required_env("ARX_WEBDAV_SMOKE_USER");
    let _ = required_env("ARX_WEBDAV_ACCEPT_PASSWORD"); // resolver convention
    let reg = Arc::new(ProviderRegistry::new());
    reg.register_webdav_targets(&[crate::config::WebDavTargetConfig {
        id: "accept".into(),
        name: "interop".into(),
        url: host,
        username: user,
        auth: "basic".into(),
    }]);
    reg.resolve_webdav_provider("accept")
        .expect("production secret-resolution chain must resolve the interop provider")
}

fn registry_with(p: &WebDavProvider) -> Arc<ProviderRegistry> {
    let reg = Arc::new(ProviderRegistry::new());
    let t = p.target();
    reg.register_webdav_targets(&[crate::config::WebDavTargetConfig {
        id: t.id.clone(),
        name: t.name.clone(),
        url: t.url.clone(),
        username: t.username.clone(),
        auth: t.auth.clone(),
    }]);
    reg
}

/// Same canonical active-source preparation -> planner -> executor seam used
/// by product F5. Identical to the Apache suite helper; no direct PUT shortcut.
#[allow(clippy::too_many_arguments)]
async fn run_f5(
    registry: &Arc<ProviderRegistry>,
    src_loc: Location,
    dst_loc: Location,
    source_listed: ListedEntry,
) -> Result<(), String> {
    let src_provider = src_loc.provider_id();
    let dst_provider = dst_loc.provider_id();
    let src_caps = registry
        .capabilities_for_location(&src_loc)
        .unwrap_or_default();
    let dst_caps = registry
        .capabilities_for_location(&dst_loc)
        .unwrap_or_default();

    let (webdav_spec, queue_name) = crate::transfer::prepare_webdav_copy(
        &src_loc,
        &dst_loc,
        &[],
        Some(&source_listed),
        &[&source_listed],
    )?;

    let mut executors = ExecutorAvailability::local();
    executors.webdav = true;

    let request = TransferRequest {
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
    };
    let plan = TransferPlanner::plan(request).map_err(|e| e.to_string())?;
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    crate::transfer::executor::execute_transfer(
        &plan,
        &[queue_name],
        registry,
        cancel,
        crate::transfer_queue::PauseGate::disabled(),
        |_| {},
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

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
    assert_eq!(
        progress, expected,
        "root progress must be stable and monotonic"
    );
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

async fn list_at(p: &WebDavProvider, target_id: &str, path: &str) -> Vec<ListedEntry> {
    p.list_page(
        &Location::WebDav {
            target: target_id.to_string(),
            path: path.to_string(),
        },
        None,
    )
    .await
    .unwrap_or_else(|e| panic!("list {path} failed: {e}"))
    .entries
}

/// The complete portable I1–I12 matrix against the provisioned REAL server.
#[tokio::test(flavor = "multi_thread")]
async fn physical_webdav_interop_core_matrix() {
    let host_ok = std::env::var("ARX_WEBDAV_SMOKE_HOST").is_ok();
    let required = std::env::var("ARX_WEBDAV_INTEROP_REQUIRED").ok().as_deref() == Some("1");
    if !host_ok && !required {
        eprintln!(
            "skipping #241 interop matrix: no fixture env (set ARX_WEBDAV_SMOKE_*; \
with ARX_WEBDAV_INTEROP_REQUIRED=1 absence is a FAILURE)"
        );
        return;
    }
    let kind = std::env::var("ARX_WEBDAV_INTEROP_KIND").unwrap_or_default();
    assert!(
        matches!(kind.as_str(), "nextcloud" | "owncloud"),
        "#241: ARX_WEBDAV_INTEROP_KIND must be nextcloud|owncloud (got `{kind}`)"
    );

    // ── I1 AUTH / ROOT LIST via the PRODUCTION resolver chain ──
    let prod = target_via_production_resolver();
    let target_id = "accept".to_string();
    let root = "/".to_string();
    let seed_entries = prod
        .list_page(
            &Location::WebDav {
                target: target_id.clone(),
                path: root.clone(),
            },
            None,
        )
        .await
        .expect("I1: authenticated root PROPFIND via production resolver");
    assert!(
        seed_entries
            .entries
            .iter()
            .any(|e| e.entry.name == "interop-seed" && e.entry.kind == EntryKind::Directory),
        "I1: seeded fixture collection visible through production resolver"
    );
    println!("I1 PASS ({kind}): production resolver + auth + seeded listing");

    let p_arc = Arc::new(target_direct());
    let run = physical_run_id();

    // ── I2 NESTED MKCOL / NAVIGATION ──
    let nested = format!("/{}-nested", run);
    let nested_file = format!("{}/child.txt", nested);
    p_arc.mkdir(&nested).await.expect("I2: mkdir");
    p_arc
        .write_file_bytes_if_unchanged(
            &nested_file,
            b"nested-content",
            &RemoteEditRevision::new(vec![], 0, 0, 0),
            &CancellationFlag::default(),
            None,
        )
        .await
        .expect("I2: put child");
    let child = list_at(&p_arc, &target_id, &nested)
        .await
        .into_iter()
        .find(|e| e.entry.name == "child.txt")
        .expect("I2: child listed inside collection");
    assert_eq!(child.entry.kind, EntryKind::File);
    println!("I2 PASS ({kind})");

    // ── I3 UNICODE / SPACE / RAW HREF IDENTITY ──
    let unicode = format!("/{}-unicodé spáces.txt", run);
    p_arc
        .write_file_bytes_if_unchanged(
            &unicode,
            b"unicode-bytes",
            &RemoteEditRevision::new(vec![], 0, 0, 0),
            &CancellationFlag::default(),
            None,
        )
        .await
        .expect("I3: put unicode/space name");
    let u_entry = list_at(&p_arc, &target_id, &root)
        .await
        .into_iter()
        .find(|e| e.entry.name == unicode.trim_start_matches('/'))
        .expect("I3: unicode entry listed by display name");
    match &u_entry.identity {
        EntryIdentity::WebDavObject(o) => {
            assert!(!o.href.is_empty(), "I3: raw href non-empty");
            assert_eq!(o.target, target_id, "I3: href bound to target");
            assert!(
                o.href.contains("%20") || o.href.contains(' '),
                "I3: href preserves the encoded/raw space form: {}",
                o.href
            );
        }
        other => panic!("I3: expected WebDavObject identity, got {other:?}"),
    }
    println!("I3 PASS ({kind})");

    // ── I4 BOUNDED GET / PREVIEW ──
    let rd = p_arc
        .read_all_capped(&nested_file, 64)
        .await
        .expect("I4: bounded read small");
    assert_eq!(rd.bytes, b"nested-content", "I4: exact bytes");
    let big = p_arc
        .read_all_capped(&nested_file, 10 * 1024 * 1024)
        .await
        .expect("I4: bounded read past EOF");
    assert_eq!(
        big.bytes.len(),
        "nested-content".len(),
        "I4: bounded to real size"
    );
    println!("I4 PASS ({kind})");

    // ── I5 LOCAL -> WEBDAV via REAL F5 path ──
    let local_name = format!("{}-upload.txt", run);
    let local_path = std::env::temp_dir().join(&local_name);
    const UPLOAD_BYTES: &[u8] = b"f5-upload-bytes";
    std::fs::write(&local_path, UPLOAD_BYTES).expect("I5: local source");
    run_f5(
        &registry_with(&p_arc),
        Location::Local(local_path.parent().unwrap().to_path_buf()),
        Location::WebDav {
            target: target_id.clone(),
            path: root.clone(),
        },
        ListedEntry {
            entry: crate::vfs::Entry {
                name: local_name.clone(),
                kind: EntryKind::File,
                size: Some(UPLOAD_BYTES.len() as u64),
                modified_unix_ms: None,
            },
            identity: EntryIdentity::Other,
        },
    )
    .await
    .expect("I5: Local -> WebDAV F5");
    let uploaded = list_at(&p_arc, &target_id, &root)
        .await
        .into_iter()
        .find(|e| e.entry.name == local_name)
        .expect("I5: upload visible on server");
    let got = p_arc
        .read_all_capped(&format!("/{local_name}"), 64)
        .await
        .expect("I5: read back upload");
    assert_eq!(got.bytes, UPLOAD_BYTES, "I5: upload bytes exact");
    let _ = uploaded;
    println!("I5 PASS ({kind})");

    // ── I6 WEBDAV -> LOCAL via REAL F5 path using real WebDavObject identity ──
    let dl_dir = tempfile::tempdir().expect("I6: dest dir");
    let w_obj = list_at(&p_arc, &target_id, &root)
        .await
        .into_iter()
        .find(|e| e.entry.name == local_name)
        .and_then(|e| match e.identity {
            EntryIdentity::WebDavObject(o) => Some(o),
            _ => None,
        })
        .expect("I6: WebDavObject identity from listing");
    run_f5(
        &registry_with(&p_arc),
        Location::WebDav {
            target: target_id.clone(),
            path: root.clone(),
        },
        Location::Local(dl_dir.path().to_path_buf()),
        ListedEntry {
            entry: crate::vfs::Entry {
                name: local_name.clone(),
                kind: EntryKind::File,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::WebDavObject(w_obj),
        },
    )
    .await
    .expect("I6: WebDAV -> Local F5");
    let dl = std::fs::read(dl_dir.path().join(&local_name)).expect("I6: downloaded file");
    assert_eq!(dl, UPLOAD_BYTES, "I6: download bytes exact");
    println!("I6 PASS ({kind})");

    // ── I7 SAME-TARGET SERVER-SIDE COPY ──
    let copy = format!("/{}-copy.txt", run);
    p_arc
        .copy_or_move(
            reqwest::Method::from_bytes(b"COPY").unwrap(),
            &format!("/{local_name}"),
            &copy,
            true,
        )
        .await
        .expect("I7: server-side COPY");
    assert!(
        list_at(&p_arc, &target_id, &root)
            .await
            .into_iter()
            .any(|e| e.entry.name == copy.trim_start_matches('/')),
        "I7: copy present"
    );
    println!("I7 PASS ({kind})");

    // ── I8 SAME-TARGET SERVER-SIDE MOVE (post-#242: no Depth on MOVE) ──
    let moved = format!("/{}-moved.txt", run);
    p_arc
        .copy_or_move(
            reqwest::Method::from_bytes(b"MOVE").unwrap(),
            &copy,
            &moved,
            true,
        )
        .await
        .expect("I8: server-side MOVE");
    let after_move = list_at(&p_arc, &target_id, &root).await;
    assert!(
        after_move
            .iter()
            .any(|e| e.entry.name == moved.trim_start_matches('/')),
        "I8: destination present"
    );
    assert!(
        !after_move
            .iter()
            .any(|e| e.entry.name == copy.trim_start_matches('/')),
        "I8: source gone after move"
    );
    println!("I8 PASS ({kind})");

    // ── I9 DELETE ──
    p_arc.remove_file(&moved).await.expect("I9: delete moved");
    p_arc
        .remove_file(&format!("/{local_name}"))
        .await
        .expect("I9: delete upload");
    p_arc
        .remove_file(&unicode)
        .await
        .expect("I9: delete unicode");
    p_arc
        .remove_file(&nested_file)
        .await
        .expect("I9: delete child");
    p_arc.remove_dir(&nested).await.expect("I9: delete dir");
    let after_delete = list_at(&p_arc, &target_id, &root).await;
    assert!(
        !after_delete.iter().any(|e| {
            let n = e.entry.name.as_str();
            n == moved.trim_start_matches('/')
                || n == local_name
                || n == unicode.trim_start_matches('/')
                || n == nested.trim_start_matches('/')
        }),
        "I9: all deleted resources gone"
    );
    println!("I9 PASS ({kind})");

    // ── I10 WRONG CREDENTIALS: factual failure, not an empty listing ──
    let bad = WebDavProvider::new(
        WebDavTarget {
            id: target_id.clone(),
            name: "interop".into(),
            url: p_arc.target().url.clone(),
            username: p_arc.target().username.clone(),
            auth: "basic".into(),
        },
        "definitely-wrong-password".into(),
    )
    .unwrap();
    let bad_res = bad
        .list_page(
            &Location::WebDav {
                target: target_id.clone(),
                path: root.clone(),
            },
            None,
        )
        .await;
    assert!(bad_res.is_err(), "I10: wrong credentials rejected");
    println!("I10 PASS ({kind})");

    // ── I11 MISSING RESOURCE: truthful 404/NotFound-equivalent ──
    let missing = p_arc.read_all_capped("/does-not-exist-xyz.txt", 64).await;
    assert!(missing.is_err(), "I11: 404 -> factual error");
    println!("I11 PASS ({kind})");

    // ── I12 OVERWRITE / NOCLOBBER SAFETY through the REAL one-file F5 path ──
    let ov = format!("/{}-noclobber.txt", run);
    p_arc
        .write_file_bytes_if_unchanged(
            &ov,
            b"OLD",
            &RemoteEditRevision::new(vec![], 0, 0, 0),
            &CancellationFlag::default(),
            None,
        )
        .await
        .expect("I12: seed OLD");
    let ov_local = std::env::temp_dir().join(ov.trim_start_matches('/'));
    std::fs::write(&ov_local, b"NEW").expect("I12: local NEW");
    let conflict = run_f5(
        &registry_with(&p_arc),
        Location::Local(ov_local.parent().unwrap().to_path_buf()),
        Location::WebDav {
            target: target_id.clone(),
            path: root.clone(),
        },
        ListedEntry {
            entry: crate::vfs::Entry {
                name: ov.trim_start_matches('/').to_string(),
                kind: EntryKind::File,
                size: Some(std::fs::metadata(&ov_local).unwrap().len()),
                modified_unix_ms: None,
            },
            identity: EntryIdentity::Other,
        },
    )
    .await;
    assert!(conflict.is_err(), "I12: forbid/noclobber must fail closed");
    let preserved = p_arc
        .read_all_capped(&ov, 64)
        .await
        .expect("I12: read existing remote object");
    assert_eq!(preserved.bytes, b"OLD", "I12: original bytes preserved");
    let _ = p_arc.remove_file(&ov).await;
    let _ = std::fs::remove_file(&ov_local);
    println!("I12 PASS ({kind})");

    println!("#241 interop core matrix PASSED for kind={kind}");
}

#[tokio::test(flavor = "multi_thread")]
async fn physical_webdav_interop_recursive_download_tree() {
    let host_ok = std::env::var("ARX_WEBDAV_SMOKE_HOST").is_ok();
    let required = std::env::var("ARX_WEBDAV_INTEROP_REQUIRED").ok().as_deref() == Some("1");
    if !host_ok && !required {
        eprintln!("skipping recursive interop: no fixture env");
        return;
    }
    let provider = Arc::new(target_direct());
    let target_id = provider.target().id.clone();
    let root_name = format!("{}-tree", physical_run_id());
    let remote_root = format!("/{root_name}");
    provider.mkdir(&remote_root).await.expect("tree root MKCOL");
    for dir in ["empty", "nested", "unicodé spáces"] {
        provider
            .mkdir(&format!("{remote_root}/{dir}"))
            .await
            .expect("child MKCOL");
    }
    for (path, bytes) in [
        ("normal.txt", b"normal".as_slice()),
        ("nested/child.txt", b"nested".as_slice()),
        ("unicodé spáces/zero.bin", b"".as_slice()),
    ] {
        provider
            .write_file_bytes_if_unchanged(
                &format!("{remote_root}/{path}"),
                bytes,
                &RemoteEditRevision::new(vec![], 0, 0, 0),
                &CancellationFlag::default(),
                None,
            )
            .await
            .expect("seed tree file");
    }
    let selected = list_at(&provider, &target_id, "/")
        .await
        .into_iter()
        .find(|entry| entry.entry.name == root_name)
        .expect("real exact collection listing");
    assert!(matches!(
        selected.identity,
        EntryIdentity::WebDavCollection(_)
    ));
    let local = tempfile::tempdir().unwrap();
    run_f5(
        &registry_with(&provider),
        Location::WebDav {
            target: target_id.clone(),
            path: "/".into(),
        },
        Location::Local(local.path().to_path_buf()),
        selected.clone(),
    )
    .await
    .expect("recursive product path");
    let downloaded = local.path().join(&root_name);
    assert_eq!(
        std::fs::read(downloaded.join("normal.txt")).unwrap(),
        b"normal"
    );
    assert_eq!(
        std::fs::read(downloaded.join("nested/child.txt")).unwrap(),
        b"nested"
    );
    assert_eq!(
        std::fs::metadata(downloaded.join("unicodé spáces/zero.bin"))
            .unwrap()
            .len(),
        0
    );
    assert!(downloaded.join("empty").is_dir());
    let collision = tempfile::tempdir().unwrap();
    let existing = collision.path().join(&root_name);
    std::fs::create_dir(&existing).unwrap();
    std::fs::write(existing.join("sentinel"), b"keep").unwrap();
    assert!(
        run_f5(
            &registry_with(&provider),
            Location::WebDav {
                target: target_id,
                path: "/".into()
            },
            Location::Local(collision.path().to_path_buf()),
            selected
        )
        .await
        .is_err()
    );
    assert_eq!(std::fs::read(existing.join("sentinel")).unwrap(), b"keep");
}

#[tokio::test(flavor = "multi_thread")]
async fn physical_webdav_interop_recursive_upload_tree() {
    let host_ok = std::env::var("ARX_WEBDAV_SMOKE_HOST").is_ok();
    let required = std::env::var("ARX_WEBDAV_INTEROP_REQUIRED").ok().as_deref() == Some("1");
    if !host_ok && !required {
        eprintln!("skipping recursive upload interop: no fixture env");
        return;
    }
    let provider = Arc::new(target_direct());
    let target_id = provider.target().id.clone();
    let local_parent = tempfile::tempdir().unwrap();
    let root_name = format!("{}-upload-tree", physical_run_id());
    let local_root = local_parent.path().join(&root_name);
    std::fs::create_dir(&local_root).unwrap();
    for dir in ["empty", "nested", "unicodé spáces"] {
        std::fs::create_dir(local_root.join(dir)).unwrap();
    }
    std::fs::write(local_root.join("normal.txt"), b"normal").unwrap();
    std::fs::write(local_root.join("zero.bin"), b"").unwrap();
    std::fs::write(local_root.join("nested/child.txt"), b"nested").unwrap();
    std::fs::write(local_root.join("unicodé spáces/file name.txt"), b"unicode").unwrap();
    let source = ListedEntry {
        entry: crate::vfs::Entry {
            name: root_name.clone(),
            kind: EntryKind::Directory,
            size: None,
            modified_unix_ms: None,
        },
        identity: EntryIdentity::Other,
    };
    run_f5(
        &registry_with(&provider),
        Location::Local(local_parent.path().to_path_buf()),
        Location::WebDav {
            target: target_id.clone(),
            path: "/".into(),
        },
        source,
    )
    .await
    .expect("recursive upload product path");
    for (path, bytes) in [
        ("normal.txt", b"normal".as_slice()),
        ("zero.bin", b"".as_slice()),
        ("nested/child.txt", b"nested".as_slice()),
        ("unicodé spáces/file name.txt", b"unicode".as_slice()),
    ] {
        assert_eq!(
            provider
                .read_all_capped(&format!("/{root_name}/{path}"), 64)
                .await
                .unwrap()
                .bytes,
            bytes
        );
    }
    assert!(
        list_at(&provider, &target_id, &format!("/{root_name}/empty"))
            .await
            .iter()
            .all(|entry| entry.entry.name == "empty")
    );
    let collision_name = format!("{}-collision", physical_run_id());
    provider.mkdir(&format!("/{collision_name}")).await.unwrap();
    provider
        .write_file_bytes_if_unchanged(
            &format!("/{collision_name}/marker"),
            b"keep",
            &RemoteEditRevision::new(vec![], 0, 0, 0),
            &CancellationFlag::default(),
            None,
        )
        .await
        .unwrap();
    let collision_root = local_parent.path().join(&collision_name);
    std::fs::create_dir(&collision_root).unwrap();
    std::fs::write(collision_root.join("new"), b"new").unwrap();
    let collision = ListedEntry {
        entry: crate::vfs::Entry {
            name: collision_name.clone(),
            kind: EntryKind::Directory,
            size: None,
            modified_unix_ms: None,
        },
        identity: EntryIdentity::Other,
    };
    assert!(
        run_f5(
            &registry_with(&provider),
            Location::Local(local_parent.path().to_path_buf()),
            Location::WebDav {
                target: target_id,
                path: "/".into()
            },
            collision
        )
        .await
        .is_err()
    );
    assert_eq!(
        provider
            .read_all_capped(&format!("/{collision_name}/marker"), 64)
            .await
            .unwrap()
            .bytes,
        b"keep"
    );
}

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
    std::fs::write(
        local
            .path()
            .join(&tree_name)
            .join("nested space/uñicode.txt"),
        b"portable",
    )
    .unwrap();
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
            .read_all_capped(
                &format!("{upload_parent}/{tree_name}/nested space/uñicode.txt"),
                64
            )
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
    provider
        .mkdir(&format!("{download_parent}/{collection_name}"))
        .await
        .unwrap();
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
    provider
        .mkdir(&format!("{download_parent}/{empty_remote}"))
        .await
        .unwrap();
    let remote_location = Location::WebDav {
        target: target_id,
        path: download_parent,
    };
    let rows = provider
        .list_page(&remote_location, None)
        .await
        .unwrap()
        .entries;
    let download_selected = vec![
        collection_name.clone(),
        empty_remote.clone(),
        object_name.clone(),
    ];
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
    assert_eq!(
        std::fs::metadata(destination.path().join(&object_name))
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        std::fs::read(
            destination
                .path()
                .join(&collection_name)
                .join("file name.txt")
        )
        .unwrap(),
        b"portable-down"
    );
    assert!(destination.path().join(&empty_remote).is_dir());
    println!("multi-root F5 PASS ({kind})");
}

#[tokio::test(flavor = "multi_thread")]
async fn physical_webdav_interop_recursive_delete_tree() {
    let host_ok = std::env::var("ARX_WEBDAV_SMOKE_HOST").is_ok();
    let required = std::env::var("ARX_WEBDAV_INTEROP_REQUIRED").ok().as_deref() == Some("1");
    if !host_ok && !required {
        eprintln!("skipping recursive delete interop: no fixture env");
        return;
    }
    let provider = Arc::new(target_direct());
    let target_id = provider.target().id.clone();
    let root_name = format!("{}-delete", physical_run_id());
    let root = format!("/{root_name}");
    provider.mkdir(&root).await.unwrap();
    provider.mkdir(&format!("{root}/empty")).await.unwrap();
    provider.mkdir(&format!("{root}/nested")).await.unwrap();
    for (path, bytes) in [
        ("zero.bin", b"".as_slice()),
        ("unicodé spáces.txt", b"u".as_slice()),
        ("nested/file.txt", b"n".as_slice()),
    ] {
        provider
            .write_file_bytes_if_unchanged(
                &format!("{root}/{path}"),
                bytes,
                &RemoteEditRevision::new(vec![], 0, 0, 0),
                &CancellationFlag::default(),
                None,
            )
            .await
            .unwrap();
    }
    let location = Location::WebDav {
        target: target_id.clone(),
        path: "/".into(),
    };
    let rows = provider.list_page(&location, None).await.unwrap().entries;
    let selected = rows.iter().find(|row| row.entry.name == root_name).unwrap();
    let plan = crate::services::prepare_webdav_recursive_delete(
        &location,
        &[],
        Some(selected),
        &rows.iter().collect::<Vec<_>>(),
    )
    .unwrap();
    let outcome = crate::services::MutationService::delete_webdav_tree(
        provider.clone(),
        plan.source,
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        |_| {},
    )
    .await
    .unwrap();
    assert_eq!(outcome.completed, outcome.total);
    assert!(
        provider
            .list_page(&location, None)
            .await
            .unwrap()
            .entries
            .iter()
            .all(|row| row.entry.name != root_name)
    );
}
