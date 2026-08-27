#![cfg(feature = "physical-webdav")]

use arx::transfer::executor::{TransferExecutionError, execute_transfer};
use arx::transfer::webdav_move::prepare_webdav_move_tree;
use arx::transfer::{
    ExecutorAvailability, TransferIntent, TransferPlan, TransferPlanner, TransferRequest,
    WebDavTransferSpec,
};
use arx::transfer_queue::{PauseGate, RetryDisposition, TypedTransferProgress};
use arx::vfs::webdav::{WebDavProvider, WebDavTarget};
use arx::vfs::{
    CancellationFlag, EntryKind, ListedEntry, Location, ProviderRegistry, RemoteEditRevision,
    VfsProvider,
};
use std::error::Error;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

type AnyError = Box<dyn Error + Send + Sync>;

#[derive(Clone)]
struct Endpoint {
    id: &'static str,
    url: String,
    user: String,
    pass: String,
    provider: Arc<WebDavProvider>,
}

struct Fixture {
    a: Endpoint,
    b: Endpoint,
}

#[derive(Default)]
struct ProxyRecord {
    delete_count: usize,
    apache_response_seen: bool,
}

struct TestProxy {
    listen_addr: String,
    record: Arc<Mutex<ProxyRecord>>,
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
        pass,
        provider,
    })
}

fn fixture() -> Result<Option<Fixture>, AnyError> {
    if std::env::var("ARX_WEBDAV_COPY_PHYSICAL").as_deref() != Ok("1") {
        eprintln!("skipping WebDAV Move physical matrix: ARX_WEBDAV_COPY_PHYSICAL=1 not set");
        return Ok(None);
    }
    Ok(Some(Fixture {
        a: endpoint(
            "copya",
            "ARX_WEBDAV_COPY_A_HOST",
            "ARX_WEBDAV_COPY_A_USER",
            "ARX_WEBDAV_COPY_A_PASS",
        )?,
        b: endpoint(
            "copyb",
            "ARX_WEBDAV_COPY_B_HOST",
            "ARX_WEBDAV_COPY_B_USER",
            "ARX_WEBDAV_COPY_B_PASS",
        )?,
    }))
}

fn registry_for(a: &Endpoint, b: &Endpoint) -> Arc<ProviderRegistry> {
    registry_for_urls(a, a.url.clone(), b, b.url.clone())
}

fn registry_for_urls(
    a: &Endpoint,
    a_url: String,
    b: &Endpoint,
    b_url: String,
) -> Arc<ProviderRegistry> {
    let registry = Arc::new(ProviderRegistry::new());
    registry.register_webdav_targets(&[
        arx::config::WebDavTargetConfig {
            id: a.id.into(),
            name: a.id.into(),
            url: a_url,
            username: a.user.clone(),
            auth: "basic".into(),
        },
        arx::config::WebDavTargetConfig {
            id: b.id.into(),
            name: b.id.into(),
            url: b_url,
            username: b.user.clone(),
            auth: "basic".into(),
        },
    ]);
    registry
}

fn token(label: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("arx-move-{label}-{}-{nanos}", std::process::id())
}

fn dav(target: &str, path: &str) -> Location {
    Location::WebDav {
        target: target.into(),
        path: path.into(),
    }
}

async fn put_new(provider: &WebDavProvider, path: &str, bytes: &[u8]) -> Result<(), AnyError> {
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

async fn seed_full(provider: &WebDavProvider, root: &str) -> Result<(), AnyError> {
    provider.mkdir(&format!("/{root}")).await?;
    provider.mkdir(&format!("/{root}/nested")).await?;
    provider.mkdir(&format!("/{root}/empty")).await?;
    provider.mkdir(&format!("/{root}/unicodé space")).await?;
    put_new(provider, &format!("/{root}/root.txt"), b"root-bytes\n").await?;
    put_new(
        provider,
        &format!("/{root}/nested/deep.bin"),
        b"\x00\x01deep\xffbytes\n",
    )
    .await?;
    put_new(provider, &format!("/{root}/unicodé space/zero.bin"), b"").await?;
    Ok(())
}

async fn seed_one(provider: &WebDavProvider, root: &str) -> Result<(), AnyError> {
    provider.mkdir(&format!("/{root}")).await?;
    put_new(provider, &format!("/{root}/a.txt"), b"a").await?;
    Ok(())
}

async fn seed_two(
    provider: &WebDavProvider,
    root: &str,
    locked_name: bool,
) -> Result<(), AnyError> {
    provider.mkdir(&format!("/{root}")).await?;
    put_new(provider, &format!("/{root}/a.txt"), b"a").await?;
    let second = if locked_name { "z-locked.txt" } else { "z.txt" };
    put_new(provider, &format!("/{root}/{second}"), b"z").await?;
    Ok(())
}

async fn read_exact(provider: &WebDavProvider, path: &str) -> Result<Vec<u8>, AnyError> {
    let read = provider.read_all_capped(path, 1024 * 1024).await?;
    if read.truncated {
        return Err(io::Error::other(format!("unexpected truncation at {path}")).into());
    }
    Ok(read.bytes)
}

async fn collection_exists(provider: &WebDavProvider, target: &str, path: &str) -> bool {
    provider.list_page(&dav(target, path), None).await.is_ok()
}

async fn file_exists(provider: &WebDavProvider, path: &str) -> bool {
    provider.read_all_capped(path, 1024).await.is_ok()
}

async fn cleanup(provider: &WebDavProvider, path: &str) {
    let _ = provider.remove_dir(path).await;
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
        .filter(|row| row.entry.name == name);
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
            io::Error::new(io::ErrorKind::InvalidData, "Move root is not a directory").into(),
        );
    }
    Ok(first)
}

async fn build_move_plan(
    registry: &ProviderRegistry,
    source_location: Location,
    destination_location: Location,
    root_name: &str,
) -> Result<(TransferPlan, Vec<String>), AnyError> {
    let listed = listed_named(registry, &source_location, root_name).await?;
    let current = [&listed];
    let (webdav_spec, queue_name) = prepare_webdav_move_tree(
        &source_location,
        &destination_location,
        &[],
        Some(&listed),
        &current,
    )
    .map_err(io::Error::other)?;
    if !matches!(webdav_spec, WebDavTransferSpec::MoveTree { .. }) {
        return Err(io::Error::other("F6 did not freeze MoveTree").into());
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
        intent: TransferIntent::Move,
        executors,
        delete_extraneous: false,
        archive_spec: None,
        s3_spec: None,
        webdav_spec: Some(webdav_spec),
    })?;
    Ok((plan, vec![queue_name]))
}

async fn execute(
    registry: &ProviderRegistry,
    plan: &TransferPlan,
    names: &[String],
    cancel: Arc<AtomicBool>,
    pause: PauseGate,
    on_progress: impl FnMut(TypedTransferProgress),
) -> Result<arx::transfer::TransferOutcome, TransferExecutionError> {
    execute_transfer(plan, names, registry, cancel, pause, on_progress).await
}

async fn assert_full_tree(provider: &WebDavProvider, root: &str) -> Result<(), AnyError> {
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
    assert!(
        collection_exists(
            provider,
            provider.target().id.as_str(),
            &format!("/{root}/empty")
        )
        .await
    );
    Ok(())
}

async fn case_cross_target_success(f: &Fixture) -> Result<(), AnyError> {
    let root = format!("{} unicodé", token("cross"));
    seed_full(&f.a.provider, &root).await?;
    let registry = registry_for(&f.a, &f.b);
    let (plan, names) =
        build_move_plan(&registry, dav(f.a.id, "/"), dav(f.b.id, "/"), &root).await?;
    let WebDavTransferSpec::MoveTree {
        source,
        destination_root,
    } = plan.webdav_spec.as_ref().expect("MoveTree spec")
    else {
        unreachable!();
    };
    assert_eq!(source.target, f.a.id);
    assert!(
        source.href.contains("%20") || source.href.contains(' '),
        "exact root href must retain the server's raw space form: {}",
        source.href
    );
    assert_eq!(destination_root.target, f.b.id);
    assert_eq!(destination_root.logical_path, format!("/{root}"));

    let mut progress = Vec::new();
    let outcome = execute(
        &registry,
        &plan,
        &names,
        Arc::new(AtomicBool::new(false)),
        PauseGate::disabled(),
        |sample| progress.push(sample),
    )
    .await?;
    assert_eq!(outcome.completed, 14);
    assert_eq!(outcome.total, 14);
    assert!(
        progress
            .iter()
            .all(|sample| matches!(sample, TypedTransferProgress::Items { .. }))
    );
    assert_eq!(progress.last().map(|sample| sample.completed()), Some(14));
    assert!(!collection_exists(&f.a.provider, f.a.id, &format!("/{root}")).await);
    assert_full_tree(&f.b.provider, &root).await?;
    cleanup(&f.b.provider, &format!("/{root}")).await;
    Ok(())
}

async fn case_same_target_success(f: &Fixture) -> Result<(), AnyError> {
    let root = token("same-source");
    let parent = token("same-destination");
    seed_full(&f.a.provider, &root).await?;
    f.a.provider.mkdir(&format!("/{parent}")).await?;
    let registry = registry_for(&f.a, &f.b);
    let (plan, names) = build_move_plan(
        &registry,
        dav(f.a.id, "/"),
        dav(f.a.id, &format!("/{parent}")),
        &root,
    )
    .await?;
    execute(
        &registry,
        &plan,
        &names,
        Arc::new(AtomicBool::new(false)),
        PauseGate::disabled(),
        |_| {},
    )
    .await?;
    assert!(!collection_exists(&f.a.provider, f.a.id, &format!("/{root}")).await);
    assert_full_tree(&f.a.provider, &format!("{parent}/{root}")).await?;
    cleanup(&f.a.provider, &format!("/{parent}")).await;
    Ok(())
}

async fn case_cancel_after_verified_copy(f: &Fixture) -> Result<(), AnyError> {
    let root = token("cancel-precommit");
    seed_one(&f.a.provider, &root).await?;
    let registry = registry_for(&f.a, &f.b);
    let (plan, names) =
        build_move_plan(&registry, dav(f.a.id, "/"), dav(f.b.id, "/"), &root).await?;
    let cancel = Arc::new(AtomicBool::new(false));
    let set_cancel = cancel.clone();
    let error = execute(
        &registry,
        &plan,
        &names,
        cancel,
        PauseGate::disabled(),
        move |progress| {
            if matches!(
                progress,
                TypedTransferProgress::Items {
                    completed: 2,
                    total: Some(4)
                }
            ) {
                set_cancel.store(true, Ordering::Release);
            }
        },
    )
    .await
    .expect_err("cancel after verified copy must not commit source delete");
    assert!(matches!(
        error,
        TransferExecutionError::Cancelled { completed: 2 }
    ));
    assert!(collection_exists(&f.a.provider, f.a.id, &format!("/{root}")).await);
    assert!(!collection_exists(&f.b.provider, f.b.id, &format!("/{root}")).await);
    cleanup(&f.a.provider, &format!("/{root}")).await;
    Ok(())
}

async fn run_paused_after_verified_copy(
    registry: Arc<ProviderRegistry>,
    plan: TransferPlan,
    names: Vec<String>,
    source_items: u64,
) -> (
    PauseGate,
    tokio::sync::oneshot::Receiver<()>,
    tokio::task::JoinHandle<Result<arx::transfer::TransferOutcome, TransferExecutionError>>,
) {
    let cancel = Arc::new(AtomicBool::new(false));
    let gate = PauseGate::new(cancel.clone());
    let run_gate = gate.clone();
    let callback_gate = gate.clone();
    let (signal_tx, signal_rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(async move {
        let mut signal_tx = Some(signal_tx);
        execute_transfer(
            &plan,
            &names,
            &registry,
            cancel,
            run_gate,
            move |progress| {
                if progress.completed() == source_items {
                    callback_gate.request();
                    if let Some(tx) = signal_tx.take() {
                        let _ = tx.send(());
                    }
                }
            },
        )
        .await
    });
    (gate, signal_rx, handle)
}

async fn case_source_drift_before_delete(f: &Fixture) -> Result<(), AnyError> {
    let root = token("source-drift");
    seed_one(&f.a.provider, &root).await?;
    let registry = registry_for(&f.a, &f.b);
    let (plan, names) =
        build_move_plan(&registry, dav(f.a.id, "/"), dav(f.b.id, "/"), &root).await?;
    let (gate, signal, handle) = run_paused_after_verified_copy(registry, plan, names, 2).await;
    signal
        .await
        .map_err(|_| io::Error::other("copy verification signal dropped"))?;
    gate.wait_checkpoint().await;
    put_new(&f.a.provider, &format!("/{root}/late.txt"), b"late").await?;
    gate.resume();
    let result = handle.await?;
    let error = result.expect_err("source drift must abort before source DELETE");
    assert_eq!(error.retry_disposition(), RetryDisposition::NeverRetry);
    assert!(
        error.to_string().contains("source tree changed"),
        "source drift must be reported factually: {error}"
    );
    assert!(
        file_exists(&f.a.provider, &format!("/{root}/a.txt")).await,
        "original source child must remain: source DELETE must not start"
    );
    assert!(file_exists(&f.a.provider, &format!("/{root}/late.txt")).await);
    assert!(!collection_exists(&f.b.provider, f.b.id, &format!("/{root}")).await);
    cleanup(&f.a.provider, &format!("/{root}")).await;
    Ok(())
}

async fn case_destination_drift_requires_recovery(f: &Fixture) -> Result<(), AnyError> {
    let root = token("destination-drift");
    seed_one(&f.a.provider, &root).await?;
    let registry = registry_for(&f.a, &f.b);
    let (plan, names) =
        build_move_plan(&registry, dav(f.a.id, "/"), dav(f.b.id, "/"), &root).await?;
    let (gate, signal, handle) = run_paused_after_verified_copy(registry, plan, names, 2).await;
    signal
        .await
        .map_err(|_| io::Error::other("copy verification signal dropped"))?;
    gate.wait_checkpoint().await;
    put_new(&f.b.provider, &format!("/{root}/intruder.txt"), b"intruder").await?;
    gate.resume();
    let result = handle.await?;
    let error = result.expect_err("destination drift must require recovery");
    assert_eq!(
        error.retry_disposition(),
        RetryDisposition::RecoveryRequired
    );
    assert!(collection_exists(&f.a.provider, f.a.id, &format!("/{root}")).await);
    assert!(file_exists(&f.b.provider, &format!("/{root}/intruder.txt")).await);
    cleanup(&f.a.provider, &format!("/{root}")).await;
    cleanup(&f.b.provider, &format!("/{root}")).await;
    Ok(())
}

fn fixture_resource_url(upstream: &str, path: &str) -> String {
    format!(
        "{}/{}",
        upstream.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

async fn lock_resource(endpoint: &Endpoint, path: &str) -> Result<String, AnyError> {
    let body = r#"<?xml version="1.0" encoding="utf-8"?>
<D:lockinfo xmlns:D="DAV:">
  <D:lockscope><D:exclusive/></D:lockscope>
  <D:locktype><D:write/></D:locktype>
  <D:owner><D:href>arx-move-physical</D:href></D:owner>
</D:lockinfo>"#;
    let response = reqwest::Client::new()
        .request(
            reqwest::Method::from_bytes(b"LOCK")?,
            fixture_resource_url(&endpoint.url, path),
        )
        .basic_auth(&endpoint.user, Some(&endpoint.pass))
        .header("Content-Type", "application/xml")
        .header("Depth", "0")
        .header("Timeout", "Second-3600")
        .body(body)
        .send()
        .await?;
    if response.status() != reqwest::StatusCode::OK {
        return Err(
            io::Error::other(format!("LOCK expected 200, got {}", response.status())).into(),
        );
    }
    Ok(response
        .headers()
        .get("Lock-Token")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| io::Error::other("LOCK response missing token"))?
        .to_string())
}

async fn unlock_resource(endpoint: &Endpoint, path: &str, token: &str) -> Result<(), AnyError> {
    let response = reqwest::Client::new()
        .request(
            reqwest::Method::from_bytes(b"UNLOCK")?,
            fixture_resource_url(&endpoint.url, path),
        )
        .basic_auth(&endpoint.user, Some(&endpoint.pass))
        .header("Lock-Token", token)
        .send()
        .await?;
    if response.status() != reqwest::StatusCode::NO_CONTENT {
        return Err(
            io::Error::other(format!("UNLOCK expected 204, got {}", response.status())).into(),
        );
    }
    Ok(())
}

async fn case_definitive_late_delete_failure(f: &Fixture) -> Result<(), AnyError> {
    let root = token("locked-delete");
    seed_two(&f.a.provider, &root, true).await?;
    let locked_path = format!("/{root}/z-locked.txt");
    let lock_token = lock_resource(&f.a, &locked_path).await?;
    let registry = registry_for(&f.a, &f.b);
    let (plan, names) =
        build_move_plan(&registry, dav(f.a.id, "/"), dav(f.b.id, "/"), &root).await?;
    let error = execute(
        &registry,
        &plan,
        &names,
        Arc::new(AtomicBool::new(false)),
        PauseGate::disabled(),
        |_| {},
    )
    .await
    .expect_err("later locked DELETE must expose partial source state");
    assert_eq!(error.retry_disposition(), RetryDisposition::NeverRetry);
    assert!(error.to_string().contains("partially deleted"));
    assert!(!file_exists(&f.a.provider, &format!("/{root}/a.txt")).await);
    assert!(file_exists(&f.a.provider, &locked_path).await);
    assert!(file_exists(&f.b.provider, &format!("/{root}/a.txt")).await);
    assert!(file_exists(&f.b.provider, &format!("/{root}/z-locked.txt")).await);
    unlock_resource(&f.a, &locked_path, &lock_token).await?;
    cleanup(&f.a.provider, &format!("/{root}")).await;
    cleanup(&f.b.provider, &format!("/{root}")).await;
    Ok(())
}

fn header_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
}

async fn start_ambiguous_second_delete_proxy(upstream_url: &str) -> io::Result<TestProxy> {
    let upstream = url::Url::parse(upstream_url)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
    let host = upstream.host_str().unwrap_or("127.0.0.1").to_string();
    let port = upstream.port_or_known_default().unwrap_or(80);
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let listen_port = listener.local_addr()?.port();
    let record = Arc::new(Mutex::new(ProxyRecord::default()));
    let shared_record = record.clone();
    tokio::spawn(async move {
        loop {
            let (client, _) = match listener.accept().await {
                Ok(value) => value,
                Err(_) => break,
            };
            let record = shared_record.clone();
            let host = host.clone();
            tokio::spawn(handle_proxy_connection(client, host, port, record));
        }
    });
    Ok(TestProxy {
        listen_addr: format!("http://127.0.0.1:{listen_port}/dav/"),
        record,
    })
}

async fn handle_proxy_connection(
    mut client: TcpStream,
    upstream_host: String,
    upstream_port: u16,
    record: Arc<Mutex<ProxyRecord>>,
) {
    let mut request = vec![0u8; 64 * 1024];
    let mut filled = 0usize;
    let request_head_end = loop {
        if filled >= request.len() {
            return;
        }
        let read = match client.read(&mut request[filled..]).await {
            Ok(read) if read > 0 => read,
            _ => return,
        };
        filled += read;
        if let Some(end) = header_end(&request[..filled]) {
            break end;
        }
    };
    let head = String::from_utf8_lossy(&request[..request_head_end]);
    let method = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().next())
        .unwrap_or("")
        .to_ascii_uppercase();
    let delete_number = if method == "DELETE" {
        let mut record = record.lock().await;
        record.delete_count += 1;
        Some(record.delete_count)
    } else {
        None
    };
    let mut upstream = match TcpStream::connect((upstream_host.as_str(), upstream_port)).await {
        Ok(stream) => stream,
        Err(_) => return,
    };
    let mut forwarded = Vec::with_capacity(filled + 24);
    forwarded.extend_from_slice(&request[..request_head_end - 2]);
    forwarded.extend_from_slice(b"Connection: close\r\n\r\n");
    forwarded.extend_from_slice(&request[request_head_end..filled]);
    if upstream.write_all(&forwarded).await.is_err() {
        return;
    }
    if delete_number != Some(2) {
        pipe(&mut client, &mut upstream).await;
        return;
    }
    if upstream.flush().await.is_err() {
        return;
    }
    let mut response = vec![0u8; 64 * 1024];
    let mut response_len = 0usize;
    let saw_response_head = loop {
        if response_len >= response.len() {
            break false;
        }
        let read = match upstream.read(&mut response[response_len..]).await {
            Ok(read) if read > 0 => read,
            _ => break false,
        };
        response_len += read;
        if header_end(&response[..response_len]).is_some() {
            break true;
        }
    };
    if saw_response_head {
        record.lock().await.apache_response_seen = true;
    }
    let _ = client.shutdown().await;
}

async fn pipe(first: &mut TcpStream, second: &mut TcpStream) {
    let (mut first_read, mut first_write) = first.split();
    let (mut second_read, mut second_write) = second.split();
    let request = tokio::io::copy(&mut first_read, &mut second_write);
    let response = tokio::io::copy(&mut second_read, &mut first_write);
    let _ = tokio::join!(request, response);
}

async fn case_ambiguous_late_delete_recovery(f: &Fixture) -> Result<(), AnyError> {
    let root = token("ambiguous-delete");
    seed_two(&f.a.provider, &root, false).await?;
    let proxy = start_ambiguous_second_delete_proxy(&f.a.url).await?;
    let registry = registry_for_urls(&f.a, proxy.listen_addr.clone(), &f.b, f.b.url.clone());
    let (plan, names) =
        build_move_plan(&registry, dav(f.a.id, "/"), dav(f.b.id, "/"), &root).await?;
    let error = execute(
        &registry,
        &plan,
        &names,
        Arc::new(AtomicBool::new(false)),
        PauseGate::disabled(),
        |_| {},
    )
    .await
    .expect_err("lost second DELETE response must require recovery");
    assert_eq!(
        error.retry_disposition(),
        RetryDisposition::RecoveryRequired
    );
    let record = proxy.record.lock().await;
    assert_eq!(
        record.delete_count, 2,
        "ambiguous DELETE must not replay or continue to root"
    );
    assert!(
        record.apache_response_seen,
        "real Apache must have processed the ambiguous DELETE"
    );
    drop(record);
    assert!(collection_exists(&f.a.provider, f.a.id, &format!("/{root}")).await);
    assert!(file_exists(&f.b.provider, &format!("/{root}/a.txt")).await);
    assert!(file_exists(&f.b.provider, &format!("/{root}/z.txt")).await);
    cleanup(&f.a.provider, &format!("/{root}")).await;
    cleanup(&f.b.provider, &format!("/{root}")).await;
    Ok(())
}

async fn case_cancel_after_source_commit_starts(f: &Fixture) -> Result<(), AnyError> {
    let root = token("cancel-partial");
    seed_two(&f.a.provider, &root, false).await?;
    let registry = registry_for(&f.a, &f.b);
    let (plan, names) =
        build_move_plan(&registry, dav(f.a.id, "/"), dav(f.b.id, "/"), &root).await?;
    let cancel = Arc::new(AtomicBool::new(false));
    let set_cancel = cancel.clone();
    let error = execute(
        &registry,
        &plan,
        &names,
        cancel,
        PauseGate::disabled(),
        move |progress| {
            if matches!(
                progress,
                TypedTransferProgress::Items {
                    completed: 4,
                    total: Some(6)
                }
            ) {
                set_cancel.store(true, Ordering::Release);
            }
        },
    )
    .await
    .expect_err("cancel after first source DELETE must remain truthful partial state");
    assert_eq!(error.retry_disposition(), RetryDisposition::NeverRetry);
    assert!(error.to_string().contains("partially deleted"));
    assert!(collection_exists(&f.a.provider, f.a.id, &format!("/{root}")).await);
    let remaining = file_exists(&f.a.provider, &format!("/{root}/a.txt")).await as usize
        + file_exists(&f.a.provider, &format!("/{root}/z.txt")).await as usize;
    assert_eq!(
        remaining, 1,
        "exactly one source child must remain after cancel"
    );
    assert!(file_exists(&f.b.provider, &format!("/{root}/a.txt")).await);
    assert!(file_exists(&f.b.provider, &format!("/{root}/z.txt")).await);
    cleanup(&f.a.provider, &format!("/{root}")).await;
    cleanup(&f.b.provider, &format!("/{root}")).await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn physical_webdav_verified_move_transaction() -> Result<(), AnyError> {
    let Some(fixture) = fixture()? else {
        return Ok(());
    };
    case_cross_target_success(&fixture).await?;
    case_same_target_success(&fixture).await?;
    case_cancel_after_verified_copy(&fixture).await?;
    case_source_drift_before_delete(&fixture).await?;
    case_destination_drift_requires_recovery(&fixture).await?;
    case_definitive_late_delete_failure(&fixture).await?;
    case_ambiguous_late_delete_recovery(&fixture).await?;
    case_cancel_after_source_commit_starts(&fixture).await?;
    Ok(())
}
