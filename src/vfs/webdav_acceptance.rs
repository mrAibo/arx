//! Physical WebDAV acceptance (W1–W18) against a real Apache mod_dav server.
//!
//! Run only when `ARX_WEBDAV_SMOKE_HOST` is set (see
//! `scripts/setup_webdav_acceptance.sh`). Skipped otherwise so CI without the
//! fixture stays green. Mirrors the S3/SFTP physical-acceptance convention:
//! real server, ephemeral localhost creds, never claims support without a
//! green physical run.
//!
//! Canonical frozen matrix:
//!   W1  auth / connect
//!   W2  list root
//!   W3  nested navigation
//!   W4  unicode / spaces / raw href identity
//!   W5  small preview (bounded read)
//!   W6  oversized preview bounded
//!   W7  Local -> WebDAV via canonical source preparation -> planner -> executor
//!   W8  WebDAV -> Local via canonical source preparation -> planner -> executor
//!   W9  MKCOL
//!   W10 async server-side COPY (provider copy_or_move seam)
//!   W11 async server-side MOVE (provider copy_or_move seam)
//!   W12 DELETE
//!   W13 wrong credentials -> AuthFailed
//!   W14 404 truthful mapping
//!   W15 overwrite conflict / fail-closed
//!   W16 423 Locked (real Apache LOCK)
//!   W17 connection drop of a GET reader (real Apache response, truncated)
//!   W18 ambiguous PUT, observed PUT count == 1 (proxy confirms backend commit)
//!
//! Every physical resource uses a unique run namespace so repeated runs never
//! collide with prior containers.

#![cfg(feature = "physical-webdav")]

// Test-only TCP proxy for W15/W17/W18 is shared from the parent vfs test module.
use super::webdav_acceptance_proxy;

use super::webdav::{WebDavProvider, WebDavTarget};
use crate::transfer::{ExecutorAvailability, TransferIntent, TransferPlanner, TransferRequest};
use crate::vfs::{
    CancellationFlag, EntryIdentity, EntryKind, ListedEntry, Location, ProviderRegistry,
    RemoteEditRevision, VfsProvider,
};
use std::io;
use std::sync::Arc;

fn physical_run_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    format!("arx-accept-{}-{}", std::process::id(), nanos)
}

fn target() -> Option<WebDavProvider> {
    let host = std::env::var("ARX_WEBDAV_SMOKE_HOST").ok()?;
    let user = std::env::var("ARX_WEBDAV_SMOKE_USER").ok()?;
    let pass = std::env::var("ARX_WEBDAV_SMOKE_PASS").ok()?;
    Some(
        WebDavProvider::new(
            WebDavTarget {
                id: "accept".into(),
                name: "accept".into(),
                url: host,
                username: user,
                auth: "basic".into(),
            },
            pass,
        )
        .unwrap(),
    )
}

/// W1 proof MUST go through the production secret-resolution chain, not
/// `WebDavProvider::new(..., pass)`. Registers the target config, then resolves
/// via `resolve_webdav_provider("accept")` which pulls the password from the
/// keyring / `ARX_WEBDAV_ACCEPT_PASSWORD` env. A successful PROPFIND against
/// real Apache proves the whole chain.
fn target_via_production_resolver() -> Option<Arc<dyn VfsProvider>> {
    let host = std::env::var("ARX_WEBDAV_SMOKE_HOST").ok()?;
    let user = std::env::var("ARX_WEBDAV_SMOKE_USER").ok()?;
    let _ = std::env::var("ARX_WEBDAV_ACCEPT_PASSWORD").ok()?;
    let reg = Arc::new(ProviderRegistry::new());
    reg.register_webdav_targets(&[crate::config::WebDavTargetConfig {
        id: "accept".into(),
        name: "accept".into(),
        url: host,
        username: user,
        auth: "basic".into(),
    }]);
    reg.resolve_webdav_provider("accept").ok()
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

/// Run through the SAME canonical active-source preparation -> planner ->
/// executor seam used by product F5. This harness does not claim to exercise
/// keyboard/mouse dispatch; it has no direct provider PUT shortcut.
async fn run_f5(
    registry: &Arc<ProviderRegistry>,
    src_loc: Location,
    dst_loc: Location,
    source_listed: ListedEntry,
) -> Result<(), String> {
    run_f5_controlled(
        registry,
        src_loc,
        dst_loc,
        source_listed,
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        |_| {},
    )
    .await
}

async fn run_f5_controlled(
    registry: &Arc<ProviderRegistry>,
    src_loc: Location,
    dst_loc: Location,
    source_listed: ListedEntry,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    mut on_progress: impl FnMut(crate::transfer_queue::TypedTransferProgress),
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
    crate::transfer::executor::execute_transfer(
        &plan,
        &[queue_name],
        registry,
        cancel,
        crate::transfer_queue::PauseGate::disabled(),
        &mut on_progress,
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

// ponytail: one combined physical suite that walks the canonical W1–W18 matrix
// using a unique run namespace. Splitting into 18 `#[tokio::test]`s would need
// 18 independent fixtures; a single ordered suite against one real server is
// the minimal honest physical proof. W15/W17/W18 use the deterministic test
// proxy; W16 uses real Apache mod_dav_lock and must execute without a skip path.
#[tokio::test(flavor = "multi_thread")]
async fn physical_w1_through_w18() {
    let Some(p) = target() else {
        eprintln!("skipping physical WebDAV acceptance: set ARX_WEBDAV_SMOKE_* env");
        return;
    };
    let p_arc = Arc::new(p);
    let registry = registry_with(&p_arc);
    let target_id = p_arc.target().id.clone();
    let root = "/".to_string();
    let run = physical_run_id();
    let dir = format!("/{}-dir", run);
    let upload = format!("/{}-upload.txt", run);
    let copy = format!("/{}-copy.txt", run);
    let moved = format!("/{}-moved.txt", run);
    let nested = format!("/{}-nested", run);
    let nested_file = format!("{}/{}", nested, "child.txt");
    let unicode = format!("/{}-unicodé spáces.txt", run);
    let local_name = format!("{}-local-src.txt", run);
    let local_path = std::env::temp_dir().join(&local_name);
    let local_parent = local_path.parent().unwrap().to_path_buf();
    let dl_name = format!("{}-dl.txt", run);
    let dl_path = std::env::temp_dir().join(&dl_name);

    // W1: PRODUCTION resolver chain (ArxConfig/WebDavTargetConfig ->
    // register_webdav_targets -> keyring/env secret resolver ->
    // resolve_webdav_provider("accept") -> authenticated PROPFIND on Apache).
    let prod = target_via_production_resolver()
        .expect("W1: production secret-resolution chain must resolve a provider");
    let prod_page = prod
        .list_page(
            &Location::WebDav {
                target: target_id.clone(),
                path: root.clone(),
            },
            None,
        )
        .await
        .expect("W1: auth + connect via production resolver");
    assert!(
        prod_page.entries.iter().any(|e| e.entry.name == ".keep"),
        "W1: seed visible through production resolver"
    );

    // W1 (continued): direct provider also connects/auths.
    let page = p_arc
        .list_page(
            &Location::WebDav {
                target: target_id.clone(),
                path: root.clone(),
            },
            None,
        )
        .await
        .expect("W1: auth + connect list");
    assert!(
        page.entries.iter().any(|e| e.entry.name == ".keep"),
        "W1/W2: seed visible"
    );

    // W3: nested navigation — create a nested collection and list inside it.
    p_arc.mkdir(&nested).await.expect("W3: mkdir nested");
    p_arc
        .write_file_bytes_if_unchanged(
            &nested_file,
            b"nested-content",
            &RemoteEditRevision::new(vec![], 0, 0, 0),
            &CancellationFlag::default(),
            None,
        )
        .await
        .expect("W3: put nested");
    let nested_page = p_arc
        .list_page(
            &Location::WebDav {
                target: target_id.clone(),
                path: nested.clone(),
            },
            None,
        )
        .await
        .expect("W3: list nested");
    assert!(
        nested_page
            .entries
            .iter()
            .any(|e| e.entry.name == "child.txt" && e.entry.kind == EntryKind::File),
        "W3: nested navigation"
    );

    // W4: unicode/spaces/raw href identity.
    p_arc
        .write_file_bytes_if_unchanged(
            &unicode,
            b"unicode-bytes",
            &RemoteEditRevision::new(vec![], 0, 0, 0),
            &CancellationFlag::default(),
            None,
        )
        .await
        .expect("W4: put unicode");
    let u_page = p_arc
        .list_page(
            &Location::WebDav {
                target: target_id.clone(),
                path: root.clone(),
            },
            None,
        )
        .await
        .expect("W4: list for unicode");
    let u_name = format!("{}-unicodé spáces.txt", run);
    let u_entry = u_page
        .entries
        .iter()
        .find(|e| e.entry.name == u_name)
        .expect("W4: unicode entry present");
    match &u_entry.identity {
        EntryIdentity::WebDavObject(o) => {
            assert!(!o.href.is_empty(), "W4: raw href non-empty");
            assert_eq!(o.target, target_id, "W4: href bound to target");
        }
        other => panic!("W4: expected WebDavObject identity, got {:?}", other),
    }

    // W5: small preview (bounded read of a few bytes).
    let rd = p_arc
        .read_all_capped(&nested_file, 64)
        .await
        .expect("W5: read nested small");
    assert_eq!(rd.bytes, b"nested-content", "W5: small preview content");

    // W6: oversized preview bounded — read far past EOF must be capped.
    let big = p_arc
        .read_all_capped(&nested_file, 10 * 1024 * 1024)
        .await
        .expect("W6: read bounded large");
    assert_eq!(
        big.bytes.len(),
        "nested-content".len(),
        "W6: bounded to real size"
    );

    // W7: Local -> WebDAV through the SAME canonical source-preparation ->
    // planner -> executor seam used by product F5 (not keyboard/mouse dispatch).
    std::fs::write(&local_path, b"local-source-bytes").expect("W7: write local src");
    let local_entry = ListedEntry {
        entry: crate::vfs::Entry {
            name: local_name.clone(),
            kind: EntryKind::File,
            size: Some(local_path.metadata().unwrap().len()),
            modified_unix_ms: None,
        },
        identity: EntryIdentity::Other,
    };
    let webdav_dst = Location::WebDav {
        target: target_id.clone(),
        path: root.clone(),
    };
    let local_src = Location::Local(local_parent.clone());
    run_f5(&registry, local_src, webdav_dst.clone(), local_entry)
        .await
        .expect("W7: Local -> WebDAV F5");
    let w7_page = p_arc
        .list_page(
            &Location::WebDav {
                target: target_id.clone(),
                path: root.clone(),
            },
            None,
        )
        .await
        .expect("W7: list after upload");
    assert!(
        w7_page.entries.iter().any(|e| e.entry.name == local_name),
        "W7: upload landed on WebDAV"
    );

    // W8: WebDAV -> Local through the same canonical seam, keeping exact href.
    // Dedicated download dir so it never collides with W7's source file.
    let w8_dir = tempfile::tempdir().expect("W8: dest dir");
    let w8_page = p_arc
        .list_page(
            &Location::WebDav {
                target: target_id.clone(),
                path: root.clone(),
            },
            None,
        )
        .await
        .expect("W8: list for download");
    let w8_obj = w8_page
        .entries
        .iter()
        .find(|e| e.entry.name == local_name)
        .and_then(|e| match &e.identity {
            EntryIdentity::WebDavObject(o) => Some(o.clone()),
            _ => None,
        })
        .expect("W8: WebDavObject identity for download");
    let w8_source = ListedEntry {
        entry: crate::vfs::Entry {
            name: local_name.clone(),
            kind: EntryKind::File,
            size: None,
            modified_unix_ms: None,
        },
        identity: EntryIdentity::WebDavObject(w8_obj.clone()),
    };
    run_f5(
        &registry,
        webdav_dst.clone(),
        Location::Local(w8_dir.path().to_path_buf()),
        w8_source,
    )
    .await
    .expect("W8: WebDAV -> Local F5");
    // The download uses the listing display name for the local file (per spec),
    // which is `local_name`; read that path, not the dl_name alias.
    let dl_bytes =
        std::fs::read(w8_dir.path().join(&local_name)).expect("W8: read downloaded file");
    assert_eq!(
        dl_bytes, b"local-source-bytes",
        "W8: download content preserved"
    );

    // W9: MKCOL (fresh unique dir — never collides with prior runs).
    p_arc.mkdir(&dir).await.expect("W9: mkdir");
    let w9_page = p_arc
        .list_page(
            &Location::WebDav {
                target: target_id.clone(),
                path: root.clone(),
            },
            None,
        )
        .await
        .expect("W9: list after mkdir");
    assert!(
        w9_page
            .entries
            .iter()
            .any(|e| e.entry.name == dir.trim_start_matches('/')
                && e.entry.kind == EntryKind::Directory),
        "W9: mkdir visible"
    );

    // Seed a file to copy/move.
    p_arc
        .write_file_bytes_if_unchanged(
            &upload,
            b"copy-move-source",
            &RemoteEditRevision::new(vec![], 0, 0, 0),
            &CancellationFlag::default(),
            None,
        )
        .await
        .expect("seed upload");

    // W10: async server-side COPY (provider copy_or_move seam, not sync VfsProvider).
    p_arc
        .copy_or_move(
            reqwest::Method::from_bytes(b"COPY").unwrap(),
            &upload,
            &copy,
            true,
        )
        .await
        .expect("W10: server COPY");
    let w10_page = p_arc
        .list_page(
            &Location::WebDav {
                target: target_id.clone(),
                path: root.clone(),
            },
            None,
        )
        .await
        .expect("W10: list after copy");
    assert!(
        w10_page
            .entries
            .iter()
            .any(|e| e.entry.name == copy.trim_start_matches('/')),
        "W10: copy landed"
    );

    // W11: async server-side MOVE (provider copy_or_move seam).
    p_arc
        .copy_or_move(
            reqwest::Method::from_bytes(b"MOVE").unwrap(),
            &copy,
            &moved,
            true,
        )
        .await
        .expect("W11: server MOVE");
    let w11_page = p_arc
        .list_page(
            &Location::WebDav {
                target: target_id.clone(),
                path: root.clone(),
            },
            None,
        )
        .await
        .expect("W11: list after move");
    assert!(
        w11_page
            .entries
            .iter()
            .any(|e| e.entry.name == moved.trim_start_matches('/')),
        "W11: moved present"
    );
    assert!(
        !w11_page
            .entries
            .iter()
            .any(|e| e.entry.name == copy.trim_start_matches('/')),
        "W11: source gone after move"
    );

    // W12: DELETE.
    p_arc.remove_file(&moved).await.expect("W12: delete moved");
    p_arc
        .remove_file(&upload)
        .await
        .expect("W12: delete upload");
    p_arc
        .remove_file(&unicode)
        .await
        .expect("W12: delete unicode");
    p_arc
        .remove_file(&nested_file)
        .await
        .expect("W12: delete nested file");
    p_arc
        .remove_dir(&nested)
        .await
        .expect("W12: delete nested dir");
    p_arc.remove_dir(&dir).await.expect("W12: delete dir");
    let w12_page = p_arc
        .list_page(
            &Location::WebDav {
                target: target_id.clone(),
                path: root.clone(),
            },
            None,
        )
        .await
        .expect("W12: list after deletes");
    assert!(
        !w12_page.entries.iter().any(|e| {
            let n = e.entry.name.as_str();
            n == moved.trim_start_matches('/')
                || n == upload.trim_start_matches('/')
                || n == dir.trim_start_matches('/')
                || n == nested.trim_start_matches('/')
        }),
        "W12: all deleted"
    );
    // Cleanup F5-uploaded file + local artifacts.
    let _ = p_arc.remove_file(&format!("/{}", local_name)).await;
    let _ = std::fs::remove_file(&local_path);
    let _ = std::fs::remove_file(&dl_path);

    // W13: wrong credentials -> AuthFailed.
    let bad = WebDavProvider::new(
        WebDavTarget {
            id: target_id.clone(),
            name: "accept".into(),
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
    assert!(bad_res.is_err(), "W13: wrong creds rejected");

    // W14: 404 truthful mapping.
    let missing = p_arc.read_all_capped("/does-not-exist-xyz.txt", 64).await;
    assert!(missing.is_err(), "W14: 404 -> err");

    // W15: overwrite fail-closed — ONE product F5 (Local -> WebDAV) through the
    // proxy. The proxy records method counts + If-None-Match, never secrets.
    let upstream = p_arc.target().url.clone();
    let proxy = webdav_acceptance_proxy::start_proxy(
        &upstream,
        webdav_acceptance_proxy::ProxyMode::PassThroughRecord,
    )
    .await
    .expect("W15: start proxy");
    let proxy_url = proxy.listen_addr.clone();

    // ARX targets the proxy (not Apache directly) so we observe exact HTTP.
    let proxy_provider = WebDavProvider::new(
        WebDavTarget {
            id: "accept".into(),
            name: "accept".into(),
            url: proxy_url.clone(),
            username: p_arc.target().username.clone(),
            auth: "basic".into(),
        },
        std::env::var("ARX_WEBDAV_SMOKE_PASS").unwrap(),
    )
    .unwrap();
    let proxy_arc = Arc::new(proxy_provider);
    let proxy_reg = registry_with(&proxy_arc);

    // Seed the SAME target resource directly on Apache (bypass proxy) with OLD.
    let ov = format!("/{}-w15.txt", run);
    p_arc
        .write_file_bytes_if_unchanged(
            &ov,
            b"OLD",
            &RemoteEditRevision::new(vec![], 0, 0, 0),
            &CancellationFlag::default(),
            None,
        )
        .await
        .expect("W15: seed OLD on Apache");

    // Local file with the SAME basename and NEW bytes.
    let ov_local = std::env::temp_dir().join(ov.trim_start_matches('/'));
    std::fs::write(&ov_local, b"NEW").expect("W15: write local NEW");

    // Exactly ONE product F5 upload (Local -> WebDAV) via the proxy.
    // Destination is the root collection; the F5 derives the exact href
    // `/run-w15.txt` from the source file name, matching the seeded resource.
    let conflict = run_f5(
        &proxy_reg,
        Location::Local(ov_local.parent().unwrap().to_path_buf()),
        Location::WebDav {
            target: "accept".into(),
            path: root.clone(),
        },
        ListedEntry {
            entry: crate::vfs::Entry {
                name: ov.trim_start_matches('/').to_string(),
                kind: EntryKind::File,
                size: Some(ov_local.metadata().unwrap().len()),
                modified_unix_ms: None,
            },
            identity: EntryIdentity::Other,
        },
    )
    .await;
    // Product F5 must surface the conflict (Forbid => 412 => AlreadyExists).
    assert!(conflict.is_err(), "W15: one product F5 rejected conflict");

    // Proxy evidence: exactly one PUT with If-None-Match:*, and crucially NO
    // overwrite preflight (HEAD == 0, PROPFIND == 0). The direct Apache seed
    // bypassed the proxy, so these counts are purely the product F5.
    let rec = proxy.record.lock().await;
    assert_eq!(rec.put_count, 1, "W15: PUT count == 1 (single product F5)");
    assert!(
        rec.seen_if_none_match,
        "W15: If-None-Match:* observed on PUT"
    );
    assert_eq!(rec.head_count, 0, "W15: no HEAD preflight");
    assert_eq!(rec.propfind_count, 0, "W15: no PROPFIND preflight");
    drop(rec);

    // Remote still holds OLD (the existing resource was not overwritten).
    let remote_after = p_arc.read_all_capped(&ov, 64).await.expect("W15: read OLD");
    assert_eq!(remote_after.bytes, b"OLD", "W15: remote OLD unchanged");
    let _ = p_arc.remove_file(&ov).await;
    let _ = std::fs::remove_file(&ov_local);

    // W16: real Apache LOCK -> 423 on ARX mutation without token (MUST EXECUTE).
    // Fixture loads mod_dav_lock, so LOCK is expected to succeed. No SKIP path.
    let w16_path = format!("/{}-w16.txt", run);
    p_arc
        .write_file_bytes_if_unchanged(
            &w16_path,
            b"before",
            &RemoteEditRevision::new(vec![], 0, 0, 0),
            &CancellationFlag::default(),
            None,
        )
        .await
        .expect("W16: seed");
    let token = lock_resource(
        &upstream,
        &w16_path,
        p_arc.target().username.as_str(),
        &std::env::var("ARX_WEBDAV_SMOKE_PASS").unwrap(),
    )
    .await
    .expect("W16: LOCK existing resource");
    // ARX mutation WITHOUT the token must be rejected (423 / PermissionDenied).
    let mut_w = p_arc
        .write_file_bytes_if_unchanged(
            &w16_path,
            b"after",
            &RemoteEditRevision::new(vec![], 0, 0, 0),
            &CancellationFlag::default(),
            None,
        )
        .await;
    let error = mut_w.expect_err("W16: locked mutation must fail");
    assert_eq!(
        error.kind(),
        io::ErrorKind::PermissionDenied,
        "W16: 423 Locked must map to PermissionDenied"
    );
    assert!(
        error.to_string().contains("423"),
        "W16: error must preserve the 423 Locked status"
    );
    // Original bytes unchanged.
    let after = p_arc
        .read_all_capped(&w16_path, 64)
        .await
        .expect("W16: read original");
    assert_eq!(after.bytes, b"before", "W16: original bytes unchanged");
    // Cleanup: UNLOCK must succeed before removing the resource.
    unlock_resource(
        &upstream,
        &w16_path,
        &token,
        p_arc.target().username.as_str(),
        &std::env::var("ARX_WEBDAV_SMOKE_PASS").unwrap(),
    )
    .await
    .expect("W16: UNLOCK");
    p_arc
        .remove_file(&w16_path)
        .await
        .expect("W16: cleanup remove_file");

    // W17: GET body drop via proxy -> GET count == 1, error, no retry.
    let drop_proxy = webdav_acceptance_proxy::start_proxy(
        &upstream,
        webdav_acceptance_proxy::ProxyMode::DropGetBody,
    )
    .await
    .expect("W17: start drop proxy");
    let w17_provider = WebDavProvider::new(
        crate::vfs::webdav::WebDavTarget {
            id: "accept".into(),
            name: "accept".into(),
            url: drop_proxy.listen_addr.clone(),
            username: p_arc.target().username.clone(),
            auth: "basic".into(),
        },
        std::env::var("ARX_WEBDAV_SMOKE_PASS").unwrap(),
    )
    .unwrap();
    let w17_arc = Arc::new(w17_provider);
    let w17_file = format!("/{}-w17.txt", run);
    let w17_bytes = b"w17-body-data";
    p_arc
        .write_file_bytes_if_unchanged(
            &w17_file,
            w17_bytes,
            &RemoteEditRevision::new(vec![], 0, 0, 0),
            &CancellationFlag::default(),
            None,
        )
        .await
        .expect("W17: seed");
    let w17_dl = std::env::temp_dir().join(format!("{}-w17-dl.txt", run));
    let w17_src_loc = Location::WebDav {
        target: "accept".into(),
        path: root.clone(),
    };
    let w17_list = w17_arc
        .list_page(&w17_src_loc, None)
        .await
        .expect("W17: list via proxy");
    let w17_obj = w17_list
        .entries
        .iter()
        .find(|e| e.entry.name == w17_file.trim_start_matches('/'))
        .and_then(|e| match &e.identity {
            EntryIdentity::WebDavObject(o) => Some(o.clone()),
            _ => None,
        })
        .expect("W17: object identity");
    // Dedicated destination dir: product F5 derives the local name from the
    // listed display name, so the real final path is dir/{listed name}.
    let w17_dir = tempfile::tempdir().expect("W17: dest dir");
    let expected_final = w17_dir.path().join(w17_file.trim_start_matches('/'));
    let w17_res = run_f5(
        &registry_with(&w17_arc),
        w17_src_loc.clone(),
        Location::Local(w17_dir.path().to_path_buf()),
        ListedEntry {
            entry: crate::vfs::Entry {
                name: w17_file.trim_start_matches('/').to_string(),
                kind: EntryKind::File,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::WebDavObject(w17_obj),
        },
    )
    .await;
    assert!(w17_res.is_err(), "W17: GET body drop -> error");
    // Real final path absent (product F5 names it from the WebDAV display name).
    assert!(!expected_final.exists(), "W17: final absent");
    // No stage artifact (.<stage>- prefix) left in the dedicated dir.
    let stage_left = std::fs::read_dir(w17_dir.path())
        .ok()
        .map(|mut d| {
            d.any(|e| {
                e.ok()
                    .map(|x| {
                        x.file_name()
                            .to_string_lossy()
                            .starts_with(".arx-download-")
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    assert!(!stage_left, "W17: no stage artifact left");
    // Proxy evidence: exactly one GET, no retry.
    let w17_rec = drop_proxy.record.lock().await;
    assert_eq!(w17_rec.get_count, 1, "W17: GET count == 1 (no retry)");
    drop(w17_rec);
    let _ = p_arc.remove_file(&w17_file).await;
    let _ = std::fs::remove_file(&w17_dl);

    // W18: ambiguous PUT via proxy -> PUT count == 1, ambiguous error, no retry.
    let amb_proxy = webdav_acceptance_proxy::start_proxy(
        &upstream,
        webdav_acceptance_proxy::ProxyMode::AmbiguousPut,
    )
    .await
    .expect("W18: start ambiguous proxy");
    let w18_provider = WebDavProvider::new(
        crate::vfs::webdav::WebDavTarget {
            id: "accept".into(),
            name: "accept".into(),
            url: amb_proxy.listen_addr.clone(),
            username: p_arc.target().username.clone(),
            auth: "basic".into(),
        },
        std::env::var("ARX_WEBDAV_SMOKE_PASS").unwrap(),
    )
    .unwrap();
    let w18_arc = Arc::new(w18_provider);
    let w18_file = format!("/{}-w18.txt", run);
    let w18_local = std::env::temp_dir().join(w18_file.trim_start_matches('/'));
    std::fs::write(&w18_local, b"w18-payload").expect("W18: write local");
    let w18_dst = Location::WebDav {
        target: "accept".into(),
        path: root.clone(),
    };
    let w18_entry = ListedEntry {
        entry: crate::vfs::Entry {
            name: w18_file.trim_start_matches('/').to_string(),
            kind: EntryKind::File,
            size: Some(w18_local.metadata().unwrap().len()),
            modified_unix_ms: None,
        },
        identity: EntryIdentity::Other,
    };
    let w18_res = run_f5(
        &registry_with(&w18_arc),
        Location::Local(w18_local.parent().unwrap().to_path_buf()),
        w18_dst.clone(),
        w18_entry,
    )
    .await;
    // Truthful ambiguous error; ARX must NOT retry the PUT.
    assert!(w18_res.is_err(), "W18: ambiguous PUT -> error");
    let w18_rec = amb_proxy.record.lock().await;
    assert_eq!(
        w18_rec.put_count, 1,
        "W18: PUT count == 1 (no blind replay)"
    );
    // Proxy observed Apache's full response -> backend committed the mutation,
    // yet ARX never received the outcome (ambiguous).
    assert!(
        w18_rec.apache_response_seen,
        "W18: proxy observed Apache response (backend committed)"
    );
    drop(w18_rec);
    // Direct Apache verification MAY show the object exists (proves ambiguity).
    // We do not assert absence — the invariant is: truthful error + no replay.
    let _ = p_arc.remove_file(&w18_file).await;
    let _ = std::fs::remove_file(&w18_local);

    let _ = Arc::new(p_arc);
    eprintln!("physical W1–W18 PASSED for run {}", run);
}

#[tokio::test(flavor = "multi_thread")]
async fn physical_webdav_recursive_download_tree() {
    let Some(provider) = target() else {
        eprintln!("skipping recursive WebDAV acceptance: no fixture env");
        return;
    };
    let provider = Arc::new(provider);
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
    let selected = provider
        .list_page(
            &Location::WebDav {
                target: target_id.clone(),
                path: "/".into(),
            },
            None,
        )
        .await
        .expect("real root listing")
        .entries
        .into_iter()
        .find(|entry| entry.entry.name == root_name)
        .expect("exact selected collection");
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
    .expect("product recursive F5");
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
async fn physical_webdav_recursive_upload_tree() {
    let Some(provider) = target() else {
        eprintln!("skipping recursive WebDAV upload acceptance: no fixture env");
        return;
    };
    let provider = Arc::new(provider);
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
        source.clone(),
    )
    .await
    .expect("recursive upload product F5");
    for (path, expected) in [
        ("normal.txt", b"normal".as_slice()),
        ("zero.bin", b"".as_slice()),
        ("nested/child.txt", b"nested".as_slice()),
        ("unicodé spáces/file name.txt", b"unicode".as_slice()),
    ] {
        assert_eq!(
            provider
                .read_all_capped(&format!("/{root_name}/{path}"), 64)
                .await
                .expect("uploaded file readable")
                .bytes,
            expected
        );
    }
    let empty = provider
        .list_page(
            &Location::WebDav {
                target: target_id.clone(),
                path: format!("/{root_name}/empty"),
            },
            None,
        )
        .await
        .expect("empty directory exists");
    assert!(
        empty
            .entries
            .iter()
            .all(|entry| entry.entry.name == "empty")
    );

    // Root collision: MKCOL is the authority; existing marker survives.
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
    let collision_source = ListedEntry {
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
                target: target_id.clone(),
                path: "/".into()
            },
            collision_source,
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

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let bad_name = format!("{}-symlink", physical_run_id());
        let bad_root = local_parent.path().join(&bad_name);
        std::fs::create_dir(&bad_root).unwrap();
        std::fs::write(bad_root.join("real"), b"x").unwrap();
        symlink(bad_root.join("real"), bad_root.join("link")).unwrap();
        let bad_source = ListedEntry {
            entry: crate::vfs::Entry {
                name: bad_name.clone(),
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
                    target: target_id.clone(),
                    path: "/".into()
                },
                bad_source
            )
            .await
            .is_err()
        );
        assert!(
            !provider
                .list_page(
                    &Location::WebDav {
                        target: target_id.clone(),
                        path: "/".into()
                    },
                    None
                )
                .await
                .unwrap()
                .entries
                .iter()
                .any(|entry| entry.entry.name == bad_name)
        );

        use std::os::unix::net::UnixListener;
        let socket_name = format!("{}-socket", physical_run_id());
        let socket_root = local_parent.path().join(&socket_name);
        std::fs::create_dir(&socket_root).unwrap();
        let listener = UnixListener::bind(socket_root.join("special.sock")).unwrap();
        let socket_source = ListedEntry {
            entry: crate::vfs::Entry {
                name: socket_name.clone(),
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
                    target: target_id.clone(),
                    path: "/".into()
                },
                socket_source,
            )
            .await
            .is_err()
        );
        drop(listener);
        assert!(
            !provider
                .list_page(
                    &Location::WebDav {
                        target: target_id.clone(),
                        path: "/".into()
                    },
                    None
                )
                .await
                .unwrap()
                .entries
                .iter()
                .any(|entry| entry.entry.name == socket_name)
        );
    }

    let cancel_name = format!("{}-cancel", physical_run_id());
    let cancel_root = local_parent.path().join(&cancel_name);
    std::fs::create_dir(&cancel_root).unwrap();
    std::fs::write(cancel_root.join("a.txt"), b"first").unwrap();
    std::fs::write(cancel_root.join("b.txt"), b"second").unwrap();
    let cancel_source = ListedEntry {
        entry: crate::vfs::Entry {
            name: cancel_name.clone(),
            kind: EntryKind::Directory,
            size: None,
            modified_unix_ms: None,
        },
        identity: EntryIdentity::Other,
    };
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let set_cancel = cancel.clone();
    let result = run_f5_controlled(
        &registry_with(&provider),
        Location::Local(local_parent.path().to_path_buf()),
        Location::WebDav {
            target: target_id.clone(),
            path: "/".into(),
        },
        cancel_source,
        cancel,
        move |progress| {
            if matches!(
                progress,
                crate::transfer_queue::TypedTransferProgress::Bytes { completed, .. }
                    if completed > 0
            ) {
                set_cancel.store(true, std::sync::atomic::Ordering::Release);
            }
        },
    )
    .await;
    assert!(result.unwrap_err().contains("cancelled"));
    assert!(
        !provider
            .list_page(
                &Location::WebDav {
                    target: target_id,
                    path: "/".into(),
                },
                None,
            )
            .await
            .unwrap()
            .entries
            .iter()
            .any(|entry| entry.entry.name == cancel_name)
    );
}

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
    provider
        .mkdir(&upload_parent)
        .await
        .expect("multi upload parent");
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
        local
            .path()
            .join(&tree_name)
            .join("unicodé child")
            .join("file name.txt"),
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
    provider
        .mkdir(&download_parent)
        .await
        .expect("multi download parent");
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
        .mkdir(&format!(
            "{download_parent}/{collection_name}/unicodé child"
        ))
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
    let download_selected = vec![
        empty_remote.clone(),
        object_name.clone(),
        collection_name.clone(),
    ];
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
                .join("unicodé child")
                .join("file name.txt")
        )
        .unwrap(),
        b"remote-nested"
    );
    assert!(destination.path().join(&empty_remote).is_dir());
    assert_eq!(
        std::fs::read(destination.path().join("sentinel-local")).unwrap(),
        b"keep-local"
    );

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

/// Join the fixture DAV root and a resource path with exactly one slash.
fn fixture_resource_url(upstream: &str, path: &str) -> String {
    format!(
        "{}/{}",
        upstream.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

/// Test-only LOCK against a seeded resource on real Apache. The fixture loads
/// mod_dav_lock, so failure is a physical-test failure, not a skip condition.
async fn lock_resource(upstream: &str, path: &str, user: &str, pass: &str) -> io::Result<String> {
    let client = reqwest::Client::new();
    let url = fixture_resource_url(upstream, path);
    let body = r#"<?xml version="1.0" encoding="utf-8"?>
<D:lockinfo xmlns:D="DAV:">
  <D:lockscope><D:exclusive/></D:lockscope>
  <D:locktype><D:write/></D:locktype>
  <D:owner><D:href>arx-test</D:href></D:owner>
</D:lockinfo>"#;
    let resp = client
        .request(reqwest::Method::from_bytes(b"LOCK").unwrap(), &url)
        .basic_auth(user, Some(pass))
        .header("Content-Type", "application/xml")
        .header("Depth", "0")
        .header("Timeout", "Second-3600")
        .body(body)
        .send()
        .await
        .map_err(|e| io::Error::other(format!("W16 LOCK transport error: {e}")))?;
    let status = resp.status();
    if status != reqwest::StatusCode::OK {
        let body = resp.text().await.unwrap_or_default();
        return Err(io::Error::other(format!(
            "W16 LOCK expected 200 for existing resource, got {status}: {}",
            body.trim()
        )));
    }
    resp.headers()
        .get("Lock-Token")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .ok_or_else(|| io::Error::other("W16 LOCK response missing Lock-Token"))
}

/// Test-only UNLOCK against real Apache; the response must be 204 No Content.
async fn unlock_resource(
    upstream: &str,
    path: &str,
    token: &str,
    user: &str,
    pass: &str,
) -> io::Result<()> {
    let client = reqwest::Client::new();
    let url = fixture_resource_url(upstream, path);
    let resp = client
        .request(reqwest::Method::from_bytes(b"UNLOCK").unwrap(), &url)
        .basic_auth(user, Some(pass))
        .header("Lock-Token", token)
        .send()
        .await
        .map_err(|e| io::Error::other(format!("W16 UNLOCK transport error: {e}")))?;
    let status = resp.status();
    if status != reqwest::StatusCode::NO_CONTENT {
        let body = resp.text().await.unwrap_or_default();
        return Err(io::Error::other(format!(
            "W16 UNLOCK expected 204, got {status}: {}",
            body.trim()
        )));
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn physical_webdav_recursive_delete_tree() {
    let Some(provider) = target() else {
        return;
    };
    let provider = Arc::new(provider);
    let target_id = provider.target().id.clone();
    let root_name = format!("{}-delete-tree", physical_run_id());
    let root_path = format!("/{root_name}");
    provider.mkdir(&root_path).await.unwrap();
    provider.mkdir(&format!("{root_path}/empty")).await.unwrap();
    provider
        .mkdir(&format!("{root_path}/nested"))
        .await
        .unwrap();
    for (path, bytes) in [
        ("normal.txt", b"normal".as_slice()),
        ("zero.bin", b"".as_slice()),
        ("nested/child.txt", b"child".as_slice()),
    ] {
        provider
            .write_file_bytes_if_unchanged(
                &format!("{root_path}/{path}"),
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
    let registry = registry_with(&provider);
    let same = registry.webdav_provider_for_mutation(&target_id).unwrap();
    assert_eq!(same.target().id, target_id);
    let outcome = crate::services::MutationService::delete_webdav_tree(
        same,
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

#[test]
fn fixture_resource_url_joins_with_exactly_one_slash() {
    assert_eq!(
        fixture_resource_url("http://127.0.0.1:1234/dav/", "/run-w16.txt"),
        "http://127.0.0.1:1234/dav/run-w16.txt"
    );
    assert_eq!(
        fixture_resource_url("http://127.0.0.1:1234/dav", "run-w16.txt"),
        "http://127.0.0.1:1234/dav/run-w16.txt"
    );
}
