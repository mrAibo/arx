#![cfg(feature = "physical-webdav")]

use arx::services::{MutationService, prepare_webdav_recursive_delete_batch};
use arx::vfs::webdav::{WebDavProvider, WebDavTarget};
use arx::vfs::{CancellationFlag, Location, RemoteEditRevision, VfsProvider};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{SystemTime, UNIX_EPOCH};

fn required_fixture() -> Option<(String, String, String, String)> {
    let required = std::env::var("ARX_WEBDAV_INTEROP_REQUIRED").ok().as_deref() == Some("1");
    let host = std::env::var("ARX_WEBDAV_SMOKE_HOST").ok();
    if host.is_none() {
        assert!(!required, "required WebDAV interop fixture is missing");
        return None;
    }
    Some((
        host.unwrap(),
        std::env::var("ARX_WEBDAV_SMOKE_USER").expect("interop user"),
        std::env::var("ARX_WEBDAV_SMOKE_PASS").expect("interop password"),
        std::env::var("ARX_WEBDAV_INTEROP_KIND").expect("interop kind"),
    ))
}

fn run_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    format!("interop-multi-delete-{}-{nanos}", std::process::id())
}

fn provider(host: String, user: String, pass: String) -> Arc<WebDavProvider> {
    Arc::new(
        WebDavProvider::new(
            WebDavTarget {
                id: "accept".into(),
                name: "interop".into(),
                url: host,
                username: user,
                auth: "basic".into(),
            },
            pass,
        )
        .expect("WebDAV interop provider"),
    )
}

async fn put(provider: &WebDavProvider, path: &str, bytes: &[u8]) {
    provider
        .write_file_bytes_if_unchanged(
            path,
            bytes,
            &RemoteEditRevision::new(vec![], 0, 0, 0),
            &CancellationFlag::default(),
            None,
        )
        .await
        .unwrap_or_else(|error| panic!("PUT {path}: {error}"));
}

#[tokio::test(flavor = "multi_thread")]
async fn physical_webdav_interop_multi_root_recursive_delete() {
    let Some((host, user, pass, kind)) = required_fixture() else {
        eprintln!("skipping multi-root WebDAV delete interop: no fixture env");
        return;
    };
    assert!(matches!(kind.as_str(), "nextcloud" | "owncloud"));

    let provider = provider(host, user, pass);
    let target = provider.target().id.clone();
    let id = run_id();
    let root_a = format!("{id}-a root");
    let root_b = format!("{id}-b root");

    provider.mkdir(&format!("/{root_a}")).await.unwrap();
    provider
        .mkdir(&format!("/{root_a}/empty dir"))
        .await
        .unwrap();
    provider
        .mkdir(&format!("/{root_a}/nested dir"))
        .await
        .unwrap();
    put(provider.as_ref(), &format!("/{root_a}/zero.bin"), b"").await;
    put(
        provider.as_ref(),
        &format!("/{root_a}/nested dir/uñicode file.txt"),
        b"portable-delete",
    )
    .await;

    provider.mkdir(&format!("/{root_b}")).await.unwrap();
    put(
        provider.as_ref(),
        &format!("/{root_b}/plain.txt"),
        b"root-b",
    )
    .await;

    // Execution roots are frozen only from the real current listing. Display
    // names select rows, but the batch planner carries exact provider hrefs.
    let location = Location::WebDav {
        target: target.clone(),
        path: "/".into(),
    };
    let rows = provider.list_page(&location, None).await.unwrap().entries;
    let active = rows.iter().collect::<Vec<_>>();
    let plan = prepare_webdav_recursive_delete_batch(
        &location,
        &[root_b.clone(), root_a.clone()],
        None,
        &active,
    )
    .expect("exact multi-root delete plan from listing");
    assert_eq!(plan.sources.len(), 2);

    let mut observed = Vec::new();
    let outcome = MutationService::delete_webdav_trees(
        provider.clone(),
        plan.sources,
        Arc::new(AtomicBool::new(false)),
        |progress| observed.push((progress.completed, progress.total)),
    )
    .await
    .expect("portable multi-root recursive delete");

    assert_eq!(outcome.total, 7);
    assert_eq!(outcome.completed, 7);
    assert_eq!(
        observed,
        (1..=7).map(|done| (done, 7)).collect::<Vec<_>>(),
        "portable delete progress must be exact global item truth"
    );

    let after = provider.list_page(&location, None).await.unwrap().entries;
    assert!(after.iter().all(|row| row.entry.name != root_a));
    assert!(after.iter().all(|row| row.entry.name != root_b));
    println!("multi-root recursive delete PASS ({kind})");
}
