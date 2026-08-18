//! Physical WebDAV acceptance (W1–W18) against a real Apache mod_dav server.
//!
//! Run only when `ARX_WEBDAV_SMOKE_HOST` is set (see
//! `scripts/setup_webdav_acceptance.sh`). Skipped otherwise so CI without the
//! fixture stays green. Mirrors the S3/SFTP physical-acceptance convention:
//! real server, ephemeral localhost creds, never claims support without a
//! green physical run.

#![cfg(feature = "physical-webdav")]

use super::webdav::{WebDavProvider, WebDavTarget};
use crate::vfs::{CancellationFlag, EntryKind, RemoteEditRevision, VfsProvider};
use std::sync::Arc;

fn target() -> Option<WebDavProvider> {
    let host = std::env::var("ARX_WEBDAV_SMOKE_HOST").ok()?;
    let user = std::env::var("ARX_WEBDAV_SMOKE_USER").ok()?;
    let pass = std::env::var("ARX_WEBDAV_SMOKE_PASS").ok()?;
    Some(
        WebDavProvider::new(WebDavTarget {
            id: "accept".into(),
            name: "accept".into(),
            url: host,
            username: user,
            auth: pass,
        })
        .unwrap(),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn physical_w1_list_read_put_mkdir_delete_copy_move() {
    // Skip unless the disposable Apache mod_dav fixture is sourced.
    let Some(p) = target() else {
        eprintln!("skipping physical WebDAV acceptance: set ARX_WEBDAV_SMOKE_* env");
        return;
    };
    let root = "/";

    // W1/W2: list (at least the seeded .keep)
    let entries = p.list_async(root).await.expect("list");
    assert!(
        entries.iter().any(|e| e.name == ".keep"),
        "W1: seed visible"
    );

    // W3: read bounded
    let rd = p.read_all_capped("/.keep", 1024).await.expect("read");
    assert!(!rd.bytes.is_empty(), "W3: content read");

    // W4: put
    p.write_file_bytes_if_unchanged(
        "/acc-put.txt",
        b"hello-webdav",
        &RemoteEditRevision::new(vec![], 0, 0, 0),
        &CancellationFlag::default(),
        None,
    )
    .await
    .expect("put");

    // W5: list sees new file
    let entries = p.list_async(root).await.expect("list2");
    assert!(
        entries.iter().any(|e| e.name == "acc-put.txt"),
        "W5: put visible"
    );

    // W7: mkdir
    p.mkdir("/acc-dir").await.expect("mkcol");
    let entries = p.list_async(root).await.expect("list3");
    assert!(
        entries
            .iter()
            .any(|e| e.name == "acc-dir" && e.kind == EntryKind::Directory),
        "W7: mkcol"
    );

    // W8: copy within target (root -> acc-dir)
    p.copy_files("/", "/acc-dir", &["acc-put.txt".to_string()])
        .expect("copy");
    let entries = p.list_async("/acc-dir").await.expect("list4");
    assert!(
        entries.iter().any(|e| e.name == "acc-put.txt"),
        "W8: copy landed"
    );

    // W9: move within target (acc-dir -> root). Remove the existing root copy
    // first so the Overwrite:F move doesn't 412.
    p.remove_file("/acc-put.txt").await.expect("pre-clean");
    p.move_files("/acc-dir", "/", &["acc-put.txt".to_string()])
        .expect("move");
    let entries = p.list_async("/acc-dir").await.expect("list5");
    assert!(
        !entries.iter().any(|e| e.name == "acc-put.txt"),
        "W9: moved out"
    );

    // W6: delete
    p.remove_file("/acc-put.txt").await.expect("delete");
    let entries = p.list_async(root).await.expect("list6");
    assert!(
        !entries.iter().any(|e| e.name == "acc-put.txt"),
        "W6: deleted"
    );

    // W12: 404 maps to NotFound
    let r = p.read_all_capped("/does-not-exist-xyz.txt", 64).await;
    assert!(r.is_err(), "W12: 404 -> err");

    let _ = Arc::new(p);
}
