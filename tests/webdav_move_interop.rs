#![cfg(feature = "physical-webdav")]

use arx::transfer::executor::execute_transfer;
use arx::transfer::webdav_move::prepare_webdav_move_tree;
use arx::transfer::{
    ExecutorAvailability, TransferIntent, TransferPlanner, TransferRequest, WebDavTransferSpec,
};
use arx::transfer_queue::{PauseGate, TypedTransferProgress};
use arx::vfs::webdav::{WebDavProvider, WebDavTarget};
use arx::vfs::{
    CancellationFlag, EntryKind, Location, ProviderRegistry, RemoteEditRevision, VfsProvider,
};
use std::error::Error;
use std::io;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{SystemTime, UNIX_EPOCH};

type AnyError = Box<dyn Error + Send + Sync>;

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
    format!("interop-move-{}-{nanos}", std::process::id())
}

fn location(path: &str) -> Location {
    Location::WebDav {
        target: "accept".into(),
        path: path.into(),
    }
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

fn registry(host: String, user: String) -> Arc<ProviderRegistry> {
    let registry = Arc::new(ProviderRegistry::new());
    registry.register_webdav_targets(&[arx::config::WebDavTargetConfig {
        id: "accept".into(),
        name: "interop".into(),
        url: host,
        username: user,
        auth: "basic".into(),
    }]);
    registry
}

async fn put(provider: &WebDavProvider, path: &str, bytes: &[u8]) -> Result<(), AnyError> {
    provider
        .write_file_bytes_if_unchanged(
            path,
            bytes,
            &RemoteEditRevision::new(vec![], 0, 0, 0),
            &CancellationFlag::default(),
            None,
        )
        .await?;
    Ok(())
}

async fn seed(provider: &WebDavProvider, root: &str) -> Result<(), AnyError> {
    provider.mkdir(&format!("/{root}")).await?;
    provider.mkdir(&format!("/{root}/nested dir")).await?;
    provider.mkdir(&format!("/{root}/empty dir")).await?;
    provider.mkdir(&format!("/{root}/unicodé space")).await?;
    put(provider, &format!("/{root}/root.txt"), b"portable-move\n").await?;
    put(
        provider,
        &format!("/{root}/nested dir/deep.bin"),
        b"\x00\x01portable\xffmove\n",
    )
    .await?;
    put(
        provider,
        &format!("/{root}/unicodé space/zero.bin"),
        b"",
    )
    .await?;
    Ok(())
}

async fn read_exact(provider: &WebDavProvider, path: &str) -> Result<Vec<u8>, AnyError> {
    let read = provider.read_all_capped(path, 1024 * 1024).await?;
    if read.truncated {
        return Err(io::Error::other(format!("unexpected truncation at {path}")).into());
    }
    Ok(read.bytes)
}

async fn collection_exists(provider: &WebDavProvider, path: &str) -> bool {
    provider.list_page(&location(path), None).await.is_ok()
}

#[tokio::test(flavor = "multi_thread")]
async fn physical_webdav_interop_verified_move_same_target() -> Result<(), AnyError> {
    let Some((host, user, pass, kind)) = required_fixture() else {
        eprintln!("skipping verified WebDAV Move interop: no fixture env");
        return Ok(());
    };
    assert!(matches!(kind.as_str(), "nextcloud" | "owncloud"));

    let provider = provider(host.clone(), user.clone(), pass);
    let registry = registry(host, user);
    let id = run_id();
    let root = format!("{id} source unicodé");
    let parent = format!("{id} destination parent");
    seed(provider.as_ref(), &root).await?;
    provider.mkdir(&format!("/{parent}")).await?;

    let source_location = location("/");
    let destination_location = location(&format!("/{parent}"));
    let page = registry.list_page(&source_location, None).await?;
    let mut matches = page.entries.iter().filter(|row| row.entry.name == root);
    let listed = matches
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "portable Move root missing"))?;
    if matches.next().is_some() || listed.entry.kind != EntryKind::Directory {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "portable Move source listing is ambiguous or not a collection",
        )
        .into());
    }
    let current = [listed];
    let (webdav_spec, queue_name) = prepare_webdav_move_tree(
        &source_location,
        &destination_location,
        &[],
        Some(listed),
        &current,
    )
    .map_err(io::Error::other)?;
    let WebDavTransferSpec::MoveTree {
        source,
        destination_root,
    } = &webdav_spec
    else {
        return Err(io::Error::other("portable F6 planning did not freeze MoveTree").into());
    };
    assert_eq!(source.target, "accept");
    assert!(
        source.href.contains("%20") || source.href.contains(' '),
        "exact live href must retain the server's space representation: {}",
        source.href
    );
    assert_eq!(destination_root.target, "accept");
    assert_eq!(destination_root.logical_path, format!("/{parent}/{root}"));

    let mut executors = ExecutorAvailability::NONE;
    executors.webdav = true;
    let plan = TransferPlanner::plan(TransferRequest {
        source_provider: source_location.provider_id(),
        destination_provider: destination_location.provider_id(),
        source_capabilities: registry
            .capabilities_for_location(&source_location)
            .unwrap_or_default(),
        destination_capabilities: registry
            .capabilities_for_location(&destination_location)
            .unwrap_or_default(),
        source: source_location,
        destination: destination_location,
        intent: TransferIntent::Move,
        executors,
        delete_extraneous: false,
        s3_spec: None,
        webdav_spec: Some(webdav_spec),
    })?;

    let mut progress = Vec::new();
    let outcome = execute_transfer(
        &plan,
        &[queue_name],
        &registry,
        Arc::new(AtomicBool::new(false)),
        PauseGate::disabled(),
        |sample| progress.push(sample),
    )
    .await?;

    assert_eq!((outcome.completed, outcome.total), (14, 14));
    assert!(
        progress
            .iter()
            .all(|sample| matches!(sample, TypedTransferProgress::Items { .. })),
        "portable Move must expose truthful item-phase progress"
    );
    assert_eq!(progress.last().map(|sample| sample.completed()), Some(14));
    assert!(!collection_exists(provider.as_ref(), &format!("/{root}")).await);

    let moved = format!("/{parent}/{root}");
    assert_eq!(
        read_exact(provider.as_ref(), &format!("{moved}/root.txt")).await?,
        b"portable-move\n"
    );
    assert_eq!(
        read_exact(provider.as_ref(), &format!("{moved}/nested dir/deep.bin")).await?,
        b"\x00\x01portable\xffmove\n"
    );
    assert_eq!(
        read_exact(
            provider.as_ref(),
            &format!("{moved}/unicodé space/zero.bin")
        )
        .await?,
        b""
    );
    assert!(collection_exists(provider.as_ref(), &format!("{moved}/empty dir")).await);

    provider.remove_dir(&format!("/{parent}")).await?;
    println!("verified same-target WebDAV Move PASS ({kind})");
    Ok(())
}
