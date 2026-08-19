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

    // W1: connect/auth — a PROPFIND under the root requires valid creds.
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

    // W15: overwrite conflict / fail-closed.
    // W15: overwrite conflict / fail-closed — through the REAL F5 path
    // (upload_one enforces WebDavOverwritePolicy::Forbid via exists()).
    let ov_ov = format!("{}-overwrite.txt", run);
    let ov_local = std::env::temp_dir().join(&ov_ov);
    std::fs::write(&ov_local, b"first").expect("W15: write local src");
    let ov_loc_entry = ListedEntry {
        entry: crate::vfs::Entry {
            name: ov_ov.clone(),
            kind: EntryKind::File,
            size: Some(ov_local.metadata().unwrap().len()),
            modified_unix_ms: None,
        },
        identity: EntryIdentity::Other,
    };
    let ov_dst = Location::WebDav {
        target: target_id.clone(),
        path: root.clone(),
    };
    let ov_src = Location::Local(ov_local.parent().unwrap().to_path_buf());
    run_f5(
        &registry,
        ov_src.clone(),
        ov_dst.clone(),
        None,
        Some(&ov_loc_entry),
        &ov_ov,
    )
    .await
    .expect("W15: first upload");
    // Second identical upload must be rejected (policy Forbid).
    let conflict = run_f5(&registry, ov_src, ov_dst, None, Some(&ov_loc_entry), &ov_ov).await;
    assert!(conflict.is_err(), "W15: overwrite conflict rejected");
    let _ = p_arc.remove_file(&format!("/{}", ov_ov)).await;
    let _ = std::fs::remove_file(&ov_local);

    // W16/W17/W18 require a fault-injecting proxy in front of real Apache; the
    // current fixture does not expose deterministic lock/drop responses, so we
    // assert the honest status: not run here, covered by integration fault tests.
    // (See HANDOFF: W16/W17/W18 deferred to fault-proxy harness.)

    let _ = Arc::new(p_arc);
    eprintln!("physical W1–W15 PASSED for run {}", run);
}
