//! #275 physical WebDAV -> WebDAV recursive-copy acceptance against two real
//! Apache mod_dav endpoints. Compiled only with `physical-webdav`.

#![cfg(feature = "physical-webdav")]

use super::webdav::{WebDavProvider, WebDavTarget};
use super::webdav_acceptance_proxy::{ProxyMode, start_proxy};
use crate::jobs::{JobEvent, JobManager, JobProgress, JobStatus, Progress};
use crate::transfer::executor::{TransferExecutionError, execute_transfer};
use crate::transfer::webdav_transfer::{build_tree_manifest, revalidate_tree_manifest};
use crate::transfer::{
    ExecutorAvailability, TransferIntent, TransferPlan, TransferPlanner, TransferRequest,
    WebDavOverwritePolicy, WebDavTransferSpec,
};
use crate::transfer_queue::{PauseGate, RetryDisposition, TransferQueueConfig};
use crate::transfer_queue_runtime::TransferQueueRuntime;
use crate::vfs::{EntryKind, ListedEntry, Location, ProviderRegistry};
use std::error::Error;
use std::io;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio::time::timeout;

type AnyError = Box<dyn Error + Send + Sync>;

#[derive(Clone)]
struct Endpoint {
    id: &'static str,
    url: String,
    user: String,
    provider: Arc<WebDavProvider>,
}

struct Fixture {
    a: Endpoint,
    b: Endpoint,
    registry: Arc<ProviderRegistry>,
}

fn env(name: &str) -> Result<String, AnyError> {
    Ok(std::env::var(name)?)
}

fn endpoint(
    id: &'static str,
    host_var: &str,
    user_var: &str,
    pass_var: &str,
) -> Result<Endpoint, AnyError> {
    let url = env(host_var)?;
    let user = env(user_var)?;
    let pass = env(pass_var)?;
    let provider = Arc::new(WebDavProvider::new(
        WebDavTarget {
            id: id.into(),
            name: id.into(),
            url: url.clone(),
            username: user.clone(),
            auth: "basic".into(),
        },
        pass.clone(),
    )?);
    Ok(Endpoint {
        id,
        url,
        user,
        provider,
    })
}

fn registry_for(a: &Endpoint, b: &Endpoint) -> Arc<ProviderRegistry> {
    let registry = Arc::new(ProviderRegistry::new());
    registry.register_webdav_targets(&[
        crate::config::WebDavTargetConfig {
            id: a.id.into(),
            name: a.id.into(),
            url: a.url.clone(),
            username: a.user.clone(),
            auth: "basic".into(),
        },
        crate::config::WebDavTargetConfig {
            id: b.id.into(),
            name: b.id.into(),
            url: b.url.clone(),
            username: b.user.clone(),
            auth: "basic".into(),
        },
    ]);
    registry
}

fn fixture() -> Result<Fixture, AnyError> {
    if std::env::var("ARX_WEBDAV_COPY_PHYSICAL").as_deref() != Ok("1") {
        return Err(io::Error::other(
            "ARX_WEBDAV_COPY_PHYSICAL=1 is required; source setup_webdav_copy_acceptance.sh",
        )
        .into());
    }
    let a = endpoint(
        "copya",
        "ARX_WEBDAV_COPY_A_HOST",
        "ARX_WEBDAV_COPY_A_USER",
        "ARX_WEBDAV_COPY_A_PASS",
    )?;
    let b = endpoint(
        "copyb",
        "ARX_WEBDAV_COPY_B_HOST",
        "ARX_WEBDAV_COPY_B_USER",
        "ARX_WEBDAV_COPY_B_PASS",
    )?;
    if a.url == b.url {
        return Err(
            io::Error::other("physical copy requires two distinct WebDAV endpoints").into(),
        );
    }
    let registry = registry_for(&a, &b);
    // Production resolver/secret path must resolve both independently.
    registry.webdav_provider_for_transfer(a.id)?;
    registry.webdav_provider_for_transfer(b.id)?;
    Ok(Fixture { a, b, registry })
}

fn token(label: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("arx-{label}-{}-{nanos}", std::process::id())
}

fn dav(id: &str, path: &str) -> Location {
    Location::WebDav {
        target: id.to_string(),
        path: path.to_string(),
    }
}

async fn mkcol(provider: &WebDavProvider, path: &str) -> Result<(), AnyError> {
    provider
        .create_new_collection(path)
        .await
        .map_err(|error| io::Error::other(error.to_string()))?;
    Ok(())
}

async fn put(provider: &WebDavProvider, path: &str, bytes: &[u8]) -> Result<(), AnyError> {
    provider
        .put_logical_with_policy(path, bytes, WebDavOverwritePolicy::Forbid)
        .await?;
    Ok(())
}

async fn replace(provider: &WebDavProvider, path: &str, bytes: &[u8]) -> Result<(), AnyError> {
    provider
        .put_logical_with_policy(path, bytes, WebDavOverwritePolicy::Allow)
        .await?;
    Ok(())
}

async fn read_exact(provider: &WebDavProvider, path: &str) -> Result<Vec<u8>, AnyError> {
    let read = provider.get_bounded(path, 512 * 1024 * 1024).await?;
    if read.truncated {
        return Err(
            io::Error::other(format!("unexpected bounded-read truncation at {path}")).into(),
        );
    }
    Ok(read.bytes)
}

async fn collection_exists(provider: &WebDavProvider, path: &str) -> bool {
    provider
        .resolve_logical_collection_exact(path)
        .await
        .is_ok()
}

async fn listed_named(
    registry: &ProviderRegistry,
    location: &Location,
    name: &str,
) -> Result<ListedEntry, AnyError> {
    let page = registry.list_page(location, None).await?;
    let mut found = page
        .entries
        .into_iter()
        .filter(|entry| entry.entry.name == name);
    let first = found.next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("missing listed root {name}"),
        )
    })?;
    if found.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("ambiguous listed root {name}"),
        )
        .into());
    }
    if first.entry.kind != EntryKind::Directory {
        return Err(
            io::Error::new(io::ErrorKind::InvalidData, "copy root is not a collection").into(),
        );
    }
    Ok(first)
}

fn build_plan(
    registry: &ProviderRegistry,
    source_location: Location,
    destination_location: Location,
    source: &ListedEntry,
) -> Result<(TransferPlan, Vec<String>), AnyError> {
    let current = [source];
    let (webdav_spec, names) = crate::transfer::webdav_batch::prepare_webdav_copy_batch(
        &source_location,
        &destination_location,
        &[],
        Some(source),
        &current,
    )
    .map_err(io::Error::other)?;
    if !matches!(webdav_spec, WebDavTransferSpec::CopyTree { .. }) {
        return Err(io::Error::other("physical WebDAV remote copy did not freeze CopyTree").into());
    }
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
        intent: TransferIntent::Copy,
        executors,
        delete_extraneous: false,
        s3_spec: None,
        webdav_spec: Some(webdav_spec),
    })?;
    Ok((plan, names))
}

async fn execute(
    registry: &ProviderRegistry,
    plan: &TransferPlan,
    names: &[String],
) -> Result<crate::transfer::executor::TransferOutcome, TransferExecutionError> {
    execute_transfer(
        plan,
        names,
        registry,
        Arc::new(AtomicBool::new(false)),
        PauseGate::disabled(),
        |_| {},
    )
    .await
}

async fn seed_tree(provider: &WebDavProvider, root: &str) -> Result<(), AnyError> {
    mkcol(provider, &format!("/{root}")).await?;
    mkcol(provider, &format!("/{root}/nested")).await?;
    mkcol(provider, &format!("/{root}/empty")).await?;
    mkcol(provider, &format!("/{root}/unicodé space")).await?;
    put(provider, &format!("/{root}/root.txt"), b"root-bytes\n").await?;
    put(
        provider,
        &format!("/{root}/nested/deep.bin"),
        b"\x00\x01deep\xffbytes\n",
    )
    .await?;
    put(provider, &format!("/{root}/unicodé space/zero.bin"), b"").await?;
    Ok(())
}

async fn assert_seed_tree(provider: &WebDavProvider, root: &str) -> Result<(), AnyError> {
    assert_eq!(
        read_exact(provider, &format!("/{root}/root.txt")).await?,
        b"root-bytes\n"
    );
    assert_eq!(
        read_exact(provider, &format!("/{root}/nested/deep.bin")).await?,
        b"\x00\x01deep\xffbytes\n"
    );
    assert_eq!(
        read_exact(provider, &format!("/{root}/unicodé space/zero.bin")).await?,
        b""
    );
    assert!(collection_exists(provider, &format!("/{root}/empty")).await);
    Ok(())
}

async fn assert_manifest(provider: &WebDavProvider, root: &str) -> Result<(), AnyError> {
    let exact = provider
        .resolve_logical_collection_exact(&format!("/{root}"))
        .await?;
    let cancel = AtomicBool::new(false);
    let manifest = build_tree_manifest(provider, &exact, &cancel, &PauseGate::disabled()).await?;
    assert_eq!(manifest.directories.len(), 3);
    assert_eq!(manifest.files.len(), 3);
    assert_eq!(manifest.descendant_count, 6);
    assert_eq!(manifest.total_bytes, Some(24));
    Ok(())
}

async fn case_cross_target(f: &Fixture) -> Result<(), AnyError> {
    let root = token("cross");
    seed_tree(&f.a.provider, &root).await?;
    let src = dav(f.a.id, "/");
    let dst = dav(f.b.id, "/");
    let listed = listed_named(&f.registry, &src, &root).await?;
    let (plan, names) = build_plan(&f.registry, src, dst, &listed)?;
    let WebDavTransferSpec::CopyTree {
        source,
        destination_root,
    } = plan.webdav_spec.as_ref().expect("frozen WebDAV spec")
    else {
        unreachable!();
    };
    assert_eq!(source.target, f.a.id);
    assert_eq!(destination_root.target, f.b.id);
    assert_eq!(destination_root.logical_path, format!("/{root}"));
    let outcome = execute(&f.registry, &plan, &names).await?;
    assert_eq!(outcome.completed, 7);
    assert_eq!(outcome.total, 7);
    assert_seed_tree(&f.b.provider, &root).await?;
    assert_manifest(&f.b.provider, &root).await?;
    Ok(())
}

async fn case_same_target(f: &Fixture) -> Result<(), AnyError> {
    let root = token("same-source");
    let parent = token("same-destination");
    seed_tree(&f.a.provider, &root).await?;
    mkcol(&f.a.provider, &format!("/{parent}")).await?;
    let src = dav(f.a.id, "/");
    let dst = dav(f.a.id, &format!("/{parent}"));
    let listed = listed_named(&f.registry, &src, &root).await?;
    let (plan, names) = build_plan(&f.registry, src, dst, &listed)?;
    execute(&f.registry, &plan, &names).await?;
    let copied = format!("{parent}/{root}");
    assert_seed_tree(&f.a.provider, &copied).await?;
    assert_seed_tree(&f.a.provider, &root).await?;
    Ok(())
}

async fn case_preexisting_refusal(f: &Fixture) -> Result<(), AnyError> {
    let root = token("noclobber");
    mkcol(&f.a.provider, &format!("/{root}")).await?;
    put(&f.a.provider, &format!("/{root}/source.txt"), b"source").await?;
    mkcol(&f.b.provider, &format!("/{root}")).await?;
    put(&f.b.provider, &format!("/{root}/keep.txt"), b"keep").await?;
    let src = dav(f.a.id, "/");
    let listed = listed_named(&f.registry, &src, &root).await?;
    let (plan, names) = build_plan(&f.registry, src, dav(f.b.id, "/"), &listed)?;
    let error = execute(&f.registry, &plan, &names)
        .await
        .expect_err("pre-existing destination root must fail closed");
    assert_eq!(error.retry_disposition(), RetryDisposition::NeverRetry);
    assert_eq!(
        read_exact(&f.b.provider, &format!("/{root}/keep.txt")).await?,
        b"keep"
    );
    assert!(
        f.b.provider
            .get_bounded(&format!("/{root}/source.txt"), 64)
            .await
            .is_err(),
        "copy must not merge into pre-existing root"
    );
    Ok(())
}

async fn case_stale_manifest(f: &Fixture) -> Result<(), AnyError> {
    let root = token("stale");
    mkcol(&f.a.provider, &format!("/{root}")).await?;
    put(&f.a.provider, &format!("/{root}/file.txt"), b"old").await?;
    let exact =
        f.a.provider
            .resolve_logical_collection_exact(&format!("/{root}"))
            .await?;
    let cancel = AtomicBool::new(false);
    let pause = PauseGate::disabled();
    let frozen = build_tree_manifest(&f.a.provider, &exact, &cancel, &pause).await?;
    replace(
        &f.a.provider,
        &format!("/{root}/file.txt"),
        b"source changed after freeze",
    )
    .await?;
    let error = revalidate_tree_manifest(&f.a.provider, &exact, &frozen, &cancel, &pause)
        .await
        .expect_err("stale real WebDAV source must fail revalidation");
    assert!(error.to_string().contains("changed after manifest freeze"));
    assert!(!collection_exists(&f.b.provider, &format!("/{root}")).await);
    Ok(())
}

fn registry_with_urls(
    f: &Fixture,
    source_url: String,
    destination_url: String,
) -> Arc<ProviderRegistry> {
    let mut a = f.a.clone();
    let mut b = f.b.clone();
    a.url = source_url;
    b.url = destination_url;
    registry_for(&a, &b)
}

async fn case_source_get_failure_cleanup(f: &Fixture) -> Result<(), AnyError> {
    let root = token("get-failure");
    mkcol(&f.a.provider, &format!("/{root}")).await?;
    put(
        &f.a.provider,
        &format!("/{root}/file.bin"),
        b"source bytes that must be truncated by the physical proxy",
    )
    .await?;
    let proxy = start_proxy(&f.a.url, ProxyMode::DropGetBody).await?;
    let registry = registry_with_urls(f, proxy.listen_addr.clone(), f.b.url.clone());
    let src = dav(f.a.id, "/");
    let listed = listed_named(&registry, &src, &root).await?;
    let (plan, names) = build_plan(&registry, src, dav(f.b.id, "/"), &listed)?;
    execute(&registry, &plan, &names)
        .await
        .expect_err("truncated real GET must fail copy");
    assert!(!collection_exists(&f.b.provider, &format!("/{root}")).await);
    assert!(proxy.record.lock().await.get_count >= 1);
    Ok(())
}

async fn case_ambiguous_put_recovery(f: &Fixture) -> Result<(), AnyError> {
    let root = token("ambiguous-put");
    mkcol(&f.a.provider, &format!("/{root}")).await?;
    put(
        &f.a.provider,
        &format!("/{root}/file.bin"),
        b"ambiguous-put-bytes",
    )
    .await?;
    let proxy = start_proxy(&f.b.url, ProxyMode::AmbiguousPut).await?;
    let registry = registry_with_urls(f, f.a.url.clone(), proxy.listen_addr.clone());
    let src = dav(f.a.id, "/");
    let listed = listed_named(&registry, &src, &root).await?;
    let (plan, names) = build_plan(&registry, src, dav(f.b.id, "/"), &listed)?;
    let error = execute(&registry, &plan, &names)
        .await
        .expect_err("dropped PUT response must be recovery-required");
    assert_eq!(
        error.retry_disposition(),
        RetryDisposition::RecoveryRequired
    );
    let record = proxy.record.lock().await;
    assert_eq!(
        record.put_count, 1,
        "ambiguous mutation must not be replayed"
    );
    assert!(
        record.apache_response_seen,
        "proxy must prove Apache processed PUT"
    );
    drop(record);
    assert!(!collection_exists(&f.b.provider, &format!("/{root}")).await);
    Ok(())
}

async fn case_cleanup_failure_recovery(f: &Fixture) -> Result<(), AnyError> {
    let root = token("cleanup-failure");
    mkcol(&f.a.provider, &format!("/{root}")).await?;
    put(
        &f.a.provider,
        &format!("/{root}/file.bin"),
        b"cleanup-failure-bytes",
    )
    .await?;
    let proxy = start_proxy(&f.b.url, ProxyMode::AmbiguousPutDropDelete).await?;
    let registry = registry_with_urls(f, f.a.url.clone(), proxy.listen_addr.clone());
    let src = dav(f.a.id, "/");
    let listed = listed_named(&registry, &src, &root).await?;
    let (plan, names) = build_plan(&registry, src, dav(f.b.id, "/"), &listed)?;
    let error = execute(&registry, &plan, &names)
        .await
        .expect_err("failed cleanup must be recovery-required");
    assert_eq!(
        error.retry_disposition(),
        RetryDisposition::RecoveryRequired
    );
    let record = proxy.record.lock().await;
    assert_eq!(record.put_count, 1);
    assert_eq!(record.delete_count, 1);
    drop(record);
    assert!(collection_exists(&f.b.provider, &format!("/{root}")).await);
    // Test-only recovery so the disposable fixture remains internally tidy.
    f.b.provider
        .delete_logical_collection(&format!("/{root}"))
        .await?;
    Ok(())
}

async fn case_runtime_cancellation(f: &Fixture) -> Result<(), AnyError> {
    let root = token("cancel");
    mkcol(&f.a.provider, &format!("/{root}")).await?;
    let large = vec![0x5au8; 128 * 1024 * 1024];
    put(&f.a.provider, &format!("/{root}/large.bin"), &large).await?;
    drop(large);
    let src = dav(f.a.id, "/");
    let listed = listed_named(&f.registry, &src, &root).await?;
    let (plan, names) = build_plan(&f.registry, src, dav(f.b.id, "/"), &listed)?;

    let jobs = JobManager::new();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let runtime = TransferQueueRuntime::new(
        jobs.clone(),
        tx,
        f.registry.as_ref().clone(),
        TransferQueueConfig::new(1)?,
    );
    let id = runtime.enqueue(plan, names)?;
    let terminal = timeout(Duration::from_secs(45), async {
        let mut requested = false;
        loop {
            let event = rx
                .recv()
                .await
                .ok_or_else(|| io::Error::other("job event stream closed"))?;
            if event.id() != id {
                continue;
            }
            if !requested
                && matches!(
                    &event,
                    JobEvent::Progress {
                        progress: JobProgress::Generic(Progress::Bytes { done, .. }),
                        ..
                    } if *done > 0
                )
            {
                runtime.cancel(&id).map_err(io::Error::other)?;
                requested = true;
            }
            if event.is_terminal() {
                return Ok::<_, io::Error>((event, requested));
            }
        }
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "cancellation case timed out"))??;
    assert!(
        terminal.1,
        "cancellation must be requested after streamed bytes were observed"
    );
    assert!(
        matches!(terminal.0, JobEvent::Cancelled { .. }),
        "terminal={:?}",
        terminal.0
    );
    assert_eq!(
        jobs.get(&id).expect("cancel job").status,
        JobStatus::Cancelled
    );
    runtime.shutdown().await;
    assert!(!collection_exists(&f.b.provider, &format!("/{root}")).await);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn physical_webdav_remote_recursive_copy() -> Result<(), AnyError> {
    if std::env::var("ARX_WEBDAV_COPY_PHYSICAL").as_deref() != Ok("1") {
        return Ok(());
    }
    let f = fixture()?;
    case_cross_target(&f).await?;
    case_same_target(&f).await?;
    case_preexisting_refusal(&f).await?;
    case_stale_manifest(&f).await?;
    case_source_get_failure_cleanup(&f).await?;
    case_ambiguous_put_recovery(&f).await?;
    case_cleanup_failure_recovery(&f).await?;
    case_runtime_cancellation(&f).await?;
    Ok(())
}
