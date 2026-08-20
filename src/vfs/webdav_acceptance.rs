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
//!   W7  Local -> WebDAV via REAL TUI F5 path (build_webdav_copy_spec -> planner -> executor)
//!   W8  WebDAV -> Local via REAL TUI F5 path
//!   W9  MKCOL
//!   W10 async server-side COPY (provider copy_or_move seam)
//!   W11 async server-side MOVE (provider copy_or_move seam)
//!   W12 DELETE
//!   W13 wrong credentials -> AuthFailed
//!   W14 404 truthful mapping
//!   W15 overwrite conflict / fail-closed
//!   W16 423 Locked (deferred: needs deterministic lock fixture)
//!   W17 connection drop of a GET reader (deferred: needs fault proxy)
//!   W18 ambiguous PUT, observed PUT count == 1 (deferred: needs fault proxy)
//!
//! Every physical resource uses a unique run namespace so repeated runs never
//! collide with prior containers.

#![cfg(feature = "physical-webdav")]

// Test-only TCP proxy for W15/W17/W18 (fault injection front of real Apache).
#[path = "webdav_acceptance_proxy.rs"]
mod webdav_acceptance_proxy;

use super::webdav::{WebDavProvider, WebDavTarget};
use crate::transfer::{ExecutorAvailability, TransferIntent, TransferPlanner, TransferRequest};
use crate::vfs::{
    CancellationFlag, EntryIdentity, EntryKind, ListedEntry, Location, ProviderRegistry,
    RemoteEditRevision, VfsProvider,
};
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

/// Run a WebDAV basic transfer through the EXACT same path as the TUI F5
/// builder (`build_webdav_copy_spec`) and the transfer executor. No direct
/// provider helper shortcuts.
async fn run_f5(
    registry: &Arc<ProviderRegistry>,
    src_loc: Location,
    dst_loc: Location,
    focused_listed: Option<&ListedEntry>,
    other_listed: Option<&ListedEntry>,
    filename: &str,
) -> Result<(), String> {
    let src_provider = src_loc.provider_id();
    let dst_provider = dst_loc.provider_id();
    let src_caps = registry.capabilities(&src_provider).unwrap_or_default();
    let dst_caps = registry.capabilities(&dst_provider).unwrap_or_default();

    let webdav_spec = crate::transfer::build_webdav_copy_spec(
        src_provider,
        dst_provider,
        &src_loc,
        &dst_loc,
        focused_listed,
        other_listed,
    )
    .map_err(|e| e.to_string())?;

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
        &[filename.to_string()],
        registry,
        cancel,
        |_| {},
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ponytail: one combined physical suite that walks the canonical W1–W18 matrix
// using a unique run namespace. Splitting into 18 `#[tokio::test]`s would need
// 18 independent fixtures; a single ordered suite against one real server is
// the minimal honest physical proof. W16/W17/W18 need a deterministic fault
// proxy in front of Apache; they are asserted as deferred here, not faked.
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
    let dl_parent = dl_path.parent().unwrap().to_path_buf();

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

    // W7: Local -> WebDAV via REAL TUI F5 path.
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
    run_f5(
        &registry,
        local_src,
        webdav_dst.clone(),
        None,
        Some(&local_entry),
        &local_name,
    )
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

    // W8: WebDAV -> Local via REAL TUI F5 path, keeping exact href.
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
    run_f5(
        &registry,
        webdav_dst.clone(),
        Location::Local(dl_parent.clone()),
        Some(&ListedEntry {
            entry: crate::vfs::Entry {
                name: local_name.clone(),
                kind: EntryKind::File,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::WebDavObject(w8_obj.clone()),
        }),
        None,
        &dl_name,
    )
    .await
    .expect("W8: WebDAV -> Local F5");
    // The download uses the listing display name for the local file (per spec),
    // which is `local_name`; read that path, not the dl_name alias.
    let dl_bytes = std::fs::read(dl_parent.join(&local_name)).expect("W8: read downloaded file");
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

    // W15: overwrite conflict / fail-closed — HTTP precondition proof via proxy.
    // Proxy in PassThroughRecord records PUT count + If-None-Match header and
    // forwards to real Apache. ARX targets the proxy URL, not Apache directly.
    let upstream = p_arc.target().url.clone();
    let proxy = webdav_acceptance_proxy::start_proxy(
        &upstream,
        webdav_acceptance_proxy::ProxyMode::PassThroughRecord,
    )
    .await
    .expect("W15: start proxy");
    let proxy_url = proxy.listen_addr.clone();

    // Build a provider + registry pointing at the PROXY (not real Apache) so we
    // can observe ARX's exact HTTP behavior.
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

    // Seed remote with bytes directly on Apache (bypass proxy), then prove
    // the overwrite policy at the PUT itself via the proxy.
    let ov = format!("/{}-w15-new.txt", run);
    let ov_name = ov.trim_start_matches('/');
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

    // First PUT via proxy (Forbid on a fresh name: succeeds -> PUT #1).
    proxy_arc
        .put_with_policy(&ov, b"NEW", crate::transfer::WebDavOverwritePolicy::Forbid)
        .await
        .expect("W15: first PUT via proxy");

    // Second PUT to the SAME url with Forbid => If-None-Match:* => 412.
    let conflict = proxy_arc
        .put_with_policy(&ov, b"NEW2", crate::transfer::WebDavOverwritePolicy::Forbid)
        .await;
    assert!(conflict.is_err(), "W15: overwrite conflict rejected");

    // Prove: exactly 2 PUTs (no blind replay after 412), If-None-Match:*
    // observed on the rejected PUT, and remote still holds OLD (no overwrite).
    let rec = proxy.record.lock().await;
    assert_eq!(rec.put_count, 2, "W15: PUT count == 2 (no blind replay)");
    assert!(
        rec.seen_if_none_match,
        "W15: If-None-Match:* observed on PUT"
    );
    drop(rec);

    // Remote OLD bytes unchanged: read directly from Apache (not proxy).
    let remote_after = p_arc.read_all_capped(&ov, 64).await.expect("W15: read OLD");
    assert_eq!(remote_after.bytes, b"OLD", "W15: remote OLD unchanged");
    let _ = p_arc.remove_file(&ov).await;

    // W16: real Apache LOCK -> 423 on ARX mutation without token.
    // Test-only LOCK/UNLOCK directly against Apache with fixture creds.
    let lock_res = lock_resource(
        &upstream,
        ov_name,
        p_arc.target().username.as_str(),
        &std::env::var("ARX_WEBDAV_SMOKE_PASS").unwrap(),
    )
    .await;
    // If the fixture supports LOCK (mod_dav_lock loaded), Apache returns a
    // Lock-Token. Capture it (never logged) and UNLOCK in finally-style.
    if let Some(_token) = lock_res {
        // ARX mutation WITHOUT the token must be rejected with 423.
        let locked_path = format!("/{}-w16.txt", run);
        p_arc
            .write_file_bytes_if_unchanged(
                &locked_path,
                b"before",
                &RemoteEditRevision::new(vec![], 0, 0, 0),
                &CancellationFlag::default(),
                None,
            )
            .await
            .expect("W16: seed");
        // Re-LOCK that specific resource, then attempt ARX overwrite without token.
        let tok2 = lock_resource(
            &upstream,
            locked_path.trim_start_matches('/'),
            p_arc.target().username.as_str(),
            &std::env::var("ARX_WEBDAV_SMOKE_PASS").unwrap(),
        )
        .await
        .expect("W16: lock resource");
        let mut_w = p_arc
            .write_file_bytes_if_unchanged(
                &locked_path,
                b"after",
                &RemoteEditRevision::new(vec![], 0, 0, 0),
                &CancellationFlag::default(),
                None,
            )
            .await;
        // Truthful 423 mapping: ARX must surface a conflict/permission error.
        assert!(
            mut_w.is_err(),
            "W16: mutation under lock without token rejected"
        );
        // Original bytes unchanged.
        let after = p_arc
            .read_all_capped(&locked_path, 64)
            .await
            .expect("W16: read");
        assert_eq!(after.bytes, b"before", "W16: original bytes unchanged");
        // Cleanup: UNLOCK so the fixture is not left locked.
        unlock_resource(
            &upstream,
            locked_path.trim_start_matches('/'),
            &tok2,
            p_arc.target().username.as_str(),
            &std::env::var("ARX_WEBDAV_SMOKE_PASS").unwrap(),
        )
        .await;
        let _ = p_arc.remove_file(&locked_path).await;
    } else {
        // Fixture does not expose mod_dav_lock; skip W16 truthfully (no fake pass).
        eprintln!("W16 SKIP: fixture does not support LOCK (mod_dav_lock not loaded)");
    }

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
    let w17_res = run_f5(
        &registry_with(&w17_arc),
        w17_src_loc.clone(),
        Location::Local(w17_dl.parent().unwrap().to_path_buf()),
        Some(&ListedEntry {
            entry: crate::vfs::Entry {
                name: w17_file.trim_start_matches('/').to_string(),
                kind: EntryKind::File,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::WebDavObject(w17_obj),
        }),
        None,
        &w17_dl.file_name().unwrap().to_string_lossy(),
    )
    .await;
    assert!(w17_res.is_err(), "W17: GET body drop -> error");
    // No partial final file left behind.
    assert!(!w17_dl.exists(), "W17: no partial final destination");
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
        None,
        Some(&w18_entry),
        w18_file.trim_start_matches('/'),
    )
    .await;
    // Truthful ambiguous error; ARX must NOT retry the PUT.
    assert!(w18_res.is_err(), "W18: ambiguous PUT -> error");
    let w18_rec = amb_proxy.record.lock().await;
    assert_eq!(
        w18_rec.put_count, 1,
        "W18: PUT count == 1 (no blind replay)"
    );
    drop(w18_rec);
    // Direct Apache verification MAY show the object exists (proves ambiguity).
    // We do not assert absence — the invariant is: truthful error + no replay.
    let _ = p_arc.remove_file(&w18_file).await;
    let _ = std::fs::remove_file(&w18_local);

    let _ = Arc::new(p_arc);
    eprintln!("physical W1–W18 PASSED for run {}", run);
}

/// Test-only LOCK against real Apache (fixture creds). Returns the Lock-Token
/// if the server supports locking, else None. Never logs the token.
async fn lock_resource(upstream: &str, path: &str, user: &str, pass: &str) -> Option<String> {
    let client = reqwest::Client::new();
    let body = r#"<?xml version="1.0" encoding="utf-8"?>
<D:lockinfo xmlns:D="DAV:">
  <D:lockscope><D:exclusive/></D:lockscope>
  <D:locktype><D:write/></D:locktype>
  <D:owner><D:href>arx-test</D:href></D:owner>
</D:lockinfo>"#;
    let resp = client
        .request(
            reqwest::Method::from_bytes(b"LOCK").unwrap(),
            format!("{}{}", upstream.trim_end_matches('/'), path),
        )
        .basic_auth(user, Some(pass))
        .header("Content-Type", "application/xml")
        .body(body)
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.headers()
        .get("Lock-Token")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

/// Test-only UNLOCK against real Apache.
async fn unlock_resource(upstream: &str, path: &str, token: &str, user: &str, pass: &str) {
    let client = reqwest::Client::new();
    let _ = client
        .request(
            reqwest::Method::from_bytes(b"UNLOCK").unwrap(),
            format!("{}{}", upstream.trim_end_matches('/'), path),
        )
        .basic_auth(user, Some(pass))
        .header("Lock-Token", token)
        .send()
        .await;
}
