#![cfg(feature = "physical-webdav")]

use arx::services::{
    MutationService, WebDavDeleteError, WebDavRecursiveDeletePlan, prepare_webdav_recursive_delete,
    prepare_webdav_recursive_delete_batch,
};
use arx::vfs::webdav::{WebDavProvider, WebDavTarget};
use arx::vfs::{CancellationFlag, Location, RemoteEditRevision, VfsProvider};
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

#[derive(Default)]
struct ProxyRecord {
    delete_count: usize,
    apache_response_seen: bool,
}

struct TestProxy {
    listen_addr: String,
    record: Arc<Mutex<ProxyRecord>>,
}

fn physical_run_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    format!("arx-delete-safety-{}-{nanos}", std::process::id())
}

fn fixture() -> Option<(String, String, String)> {
    Some((
        std::env::var("ARX_WEBDAV_SMOKE_HOST").ok()?,
        std::env::var("ARX_WEBDAV_SMOKE_USER").ok()?,
        std::env::var("ARX_WEBDAV_SMOKE_PASS").ok()?,
    ))
}

fn provider_for(url: String, user: &str, pass: &str) -> Arc<WebDavProvider> {
    Arc::new(
        WebDavProvider::new(
            WebDavTarget {
                id: "accept".into(),
                name: "accept".into(),
                url,
                username: user.into(),
                auth: "basic".into(),
            },
            pass.into(),
        )
        .unwrap(),
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
        .unwrap();
}

async fn plan_for_root(provider: &WebDavProvider, root_name: &str) -> WebDavRecursiveDeletePlan {
    let location = Location::WebDav {
        target: provider.target().id.clone(),
        path: "/".into(),
    };
    let rows = provider.list_page(&location, None).await.unwrap().entries;
    let selected = rows
        .iter()
        .find(|row| row.entry.name == root_name)
        .expect("root must be listed");
    let active = rows.iter().collect::<Vec<_>>();
    prepare_webdav_recursive_delete(&location, &[], Some(selected), &active).unwrap()
}

async fn plan_for_roots(
    provider: &WebDavProvider,
    root_names: &[String],
) -> WebDavRecursiveDeletePlan {
    let location = Location::WebDav {
        target: provider.target().id.clone(),
        path: "/".into(),
    };
    let rows = provider.list_page(&location, None).await.unwrap().entries;
    let active = rows.iter().collect::<Vec<_>>();
    prepare_webdav_recursive_delete_batch(&location, root_names, None, &active).unwrap()
}

async fn list_root_names(provider: &WebDavProvider, root_name: &str) -> Vec<String> {
    provider
        .list_page(
            &Location::WebDav {
                target: provider.target().id.clone(),
                path: format!("/{root_name}"),
            },
            None,
        )
        .await
        .unwrap()
        .entries
        .into_iter()
        .map(|row| row.entry.name)
        .collect()
}

async fn root_exists(provider: &WebDavProvider, root_name: &str) -> bool {
    provider
        .list_page(
            &Location::WebDav {
                target: provider.target().id.clone(),
                path: "/".into(),
            },
            None,
        )
        .await
        .unwrap()
        .entries
        .iter()
        .any(|row| row.entry.name == root_name)
}

async fn cleanup_tree(provider: &WebDavProvider, root_name: &str) {
    let _ = provider.remove_dir(&format!("/{root_name}")).await;
}

fn fixture_resource_url(upstream: &str, path: &str) -> String {
    format!(
        "{}/{}",
        upstream.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

async fn lock_resource(upstream: &str, path: &str, user: &str, pass: &str) -> io::Result<String> {
    let body = r#"<?xml version="1.0" encoding="utf-8"?>
<D:lockinfo xmlns:D="DAV:">
  <D:lockscope><D:exclusive/></D:lockscope>
  <D:locktype><D:write/></D:locktype>
  <D:owner><D:href>arx-delete-safety</D:href></D:owner>
</D:lockinfo>"#;
    let response = reqwest::Client::new()
        .request(
            reqwest::Method::from_bytes(b"LOCK").unwrap(),
            fixture_resource_url(upstream, path),
        )
        .basic_auth(user, Some(pass))
        .header("Content-Type", "application/xml")
        .header("Depth", "0")
        .header("Timeout", "Second-3600")
        .body(body)
        .send()
        .await
        .map_err(|error| io::Error::other(format!("LOCK transport: {error}")))?;
    let status = response.status();
    if status != reqwest::StatusCode::OK {
        return Err(io::Error::other(format!("LOCK expected 200, got {status}")));
    }
    response
        .headers()
        .get("Lock-Token")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .ok_or_else(|| io::Error::other("LOCK response missing Lock-Token"))
}

async fn unlock_resource(
    upstream: &str,
    path: &str,
    token: &str,
    user: &str,
    pass: &str,
) -> io::Result<()> {
    let response = reqwest::Client::new()
        .request(
            reqwest::Method::from_bytes(b"UNLOCK").unwrap(),
            fixture_resource_url(upstream, path),
        )
        .basic_auth(user, Some(pass))
        .header("Lock-Token", token)
        .send()
        .await
        .map_err(|error| io::Error::other(format!("UNLOCK transport: {error}")))?;
    if response.status() != reqwest::StatusCode::NO_CONTENT {
        return Err(io::Error::other(format!(
            "UNLOCK expected 204, got {}",
            response.status()
        )));
    }
    Ok(())
}

fn header_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
}

async fn start_ambiguous_delete_proxy(
    upstream_url: &str,
    ambiguous_delete_number: usize,
) -> io::Result<TestProxy> {
    if ambiguous_delete_number == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "ambiguous DELETE number must be >= 1",
        ));
    }
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
            tokio::spawn(handle_proxy_connection(
                client,
                host,
                port,
                record,
                ambiguous_delete_number,
            ));
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
    ambiguous_delete_number: usize,
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

    // Force one HTTP request per client connection so every DELETE crosses the
    // parser and the requested ordinal can be fault-injected deterministically.
    let mut forwarded = Vec::with_capacity(filled + 24);
    forwarded.extend_from_slice(&request[..request_head_end - 2]);
    forwarded.extend_from_slice(b"Connection: close\r\n\r\n");
    forwarded.extend_from_slice(&request[request_head_end..filled]);
    if upstream.write_all(&forwarded).await.is_err() {
        return;
    }

    if delete_number != Some(ambiguous_delete_number) {
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

#[tokio::test(flavor = "multi_thread")]
async fn physical_webdav_recursive_delete_safety() {
    let Some((upstream, user, pass)) = fixture() else {
        eprintln!("skipping WebDAV delete safety physical test: set ARX_WEBDAV_SMOKE_* env");
        return;
    };
    let provider = provider_for(upstream.clone(), &user, &pass);

    // Cancellation after one definitive success must stop immediately, retain
    // the root, and report the exact partial count. No rollback is claimed.
    let cancel_root = format!("{}-cancel", physical_run_id());
    provider.mkdir(&format!("/{cancel_root}")).await.unwrap();
    put(&provider, &format!("/{cancel_root}/a.txt"), b"a").await;
    put(&provider, &format!("/{cancel_root}/b.txt"), b"b").await;
    let cancel_plan = plan_for_root(&provider, &cancel_root).await;
    let cancel = Arc::new(AtomicBool::new(false));
    let set_cancel = cancel.clone();
    let cancel_result = MutationService::delete_webdav_tree(
        provider.clone(),
        cancel_plan.source,
        cancel,
        move |progress| {
            if progress.completed == 1 {
                set_cancel.store(true, Ordering::Release);
            }
        },
    )
    .await;
    assert!(matches!(
        cancel_result,
        Err(WebDavDeleteError::Cancelled {
            completed: 1,
            total: 3
        })
    ));
    assert!(root_exists(&provider, &cancel_root).await);
    let cancel_names = list_root_names(&provider, &cancel_root).await;
    let remaining_cancel_children = cancel_names.iter().any(|name| name == "a.txt") as usize
        + cancel_names.iter().any(|name| name == "b.txt") as usize;
    assert_eq!(remaining_cancel_children, 1);
    cleanup_tree(&provider, &cancel_root).await;

    // A real Apache write lock must yield a definitive 423 on the first exact
    // DELETE. ARX must not continue to the later peer or delete the root.
    let locked_root = format!("{}-locked", physical_run_id());
    provider.mkdir(&format!("/{locked_root}")).await.unwrap();
    put(
        &provider,
        &format!("/{locked_root}/a-locked.txt"),
        b"locked",
    )
    .await;
    put(&provider, &format!("/{locked_root}/z-later.txt"), b"later").await;
    let locked_path = format!("/{locked_root}/a-locked.txt");
    let token = lock_resource(&upstream, &locked_path, &user, &pass)
        .await
        .unwrap();
    let locked_plan = plan_for_root(&provider, &locked_root).await;
    let locked_result = MutationService::delete_webdav_tree(
        provider.clone(),
        locked_plan.source,
        Arc::new(AtomicBool::new(false)),
        |_| {},
    )
    .await;
    assert!(matches!(
        locked_result,
        Err(WebDavDeleteError::PreMutation { .. })
    ));
    assert!(root_exists(&provider, &locked_root).await);
    let locked_names = list_root_names(&provider, &locked_root).await;
    assert!(locked_names.iter().any(|name| name == "a-locked.txt"));
    assert!(locked_names.iter().any(|name| name == "z-later.txt"));
    unlock_resource(&upstream, &locked_path, &token, &user, &pass)
        .await
        .unwrap();
    cleanup_tree(&provider, &locked_root).await;

    // Ambiguous DELETE: proxy forwards the first exact DELETE to Apache and
    // observes Apache's response, but withholds it from ARX. The destructive
    // state is therefore uncertain to ARX: RecoveryRequired, exactly one
    // DELETE request, and no later manifest node may be attempted.
    let ambiguous_root = format!("{}-ambiguous", physical_run_id());
    provider.mkdir(&format!("/{ambiguous_root}")).await.unwrap();
    put(
        &provider,
        &format!("/{ambiguous_root}/a-first.txt"),
        b"first",
    )
    .await;
    put(
        &provider,
        &format!("/{ambiguous_root}/z-later.txt"),
        b"later",
    )
    .await;

    let proxy = start_ambiguous_delete_proxy(&upstream, 1).await.unwrap();
    let proxy_provider = provider_for(proxy.listen_addr.clone(), &user, &pass);
    let ambiguous_plan = plan_for_root(&proxy_provider, &ambiguous_root).await;
    let ambiguous_result = MutationService::delete_webdav_tree(
        proxy_provider,
        ambiguous_plan.source,
        Arc::new(AtomicBool::new(false)),
        |_| {},
    )
    .await;
    assert!(matches!(
        ambiguous_result,
        Err(WebDavDeleteError::RecoveryRequired {
            completed: 0,
            total: 3,
            ..
        })
    ));
    let record = proxy.record.lock().await;
    assert_eq!(record.delete_count, 1, "ambiguous DELETE must never replay");
    assert!(
        record.apache_response_seen,
        "proxy must prove Apache processed the uncertain DELETE"
    );
    drop(record);
    assert!(root_exists(&provider, &ambiguous_root).await);
    let ambiguous_names = list_root_names(&provider, &ambiguous_root).await;
    let remaining_ambiguous_children = ambiguous_names.iter().any(|name| name == "a-first.txt")
        as usize
        + ambiguous_names.iter().any(|name| name == "z-later.txt") as usize;
    assert_eq!(
        remaining_ambiguous_children, 1,
        "no later manifest node may be deleted after ambiguity"
    );
    cleanup_tree(&provider, &ambiguous_root).await;

    // Multi-root positive path: two selected sibling trees are completely
    // frozen/revalidated before deletion, then each executes child-first and
    // root-last. Include empty/nested collections, zero-byte and Unicode names.
    let positive_id = physical_run_id();
    let positive_a = format!("{positive_id}-a-positive");
    let positive_b = format!("{positive_id}-b-positive");
    provider.mkdir(&format!("/{positive_a}")).await.unwrap();
    provider
        .mkdir(&format!("/{positive_a}/empty"))
        .await
        .unwrap();
    provider
        .mkdir(&format!("/{positive_a}/nested"))
        .await
        .unwrap();
    put(&provider, &format!("/{positive_a}/zero.bin"), b"").await;
    put(
        &provider,
        &format!("/{positive_a}/nested/unicodé spáces.txt"),
        b"unicode",
    )
    .await;
    provider.mkdir(&format!("/{positive_b}")).await.unwrap();
    put(&provider, &format!("/{positive_b}/file.txt"), b"b").await;
    let positive_plan = plan_for_roots(
        &provider,
        &[positive_b.clone(), positive_a.clone()],
    )
    .await;
    let positive_result = MutationService::delete_webdav_trees(
        provider.clone(),
        positive_plan.sources,
        Arc::new(AtomicBool::new(false)),
        |_| {},
    )
    .await
    .unwrap();
    assert_eq!(positive_result.completed, positive_result.total);
    assert!(!root_exists(&provider, &positive_a).await);
    assert!(!root_exists(&provider, &positive_b).await);

    // Cancellation between roots preserves the exact global count and never
    // starts the later root. Empty roots make each definitive success one item.
    let batch_cancel_id = physical_run_id();
    let batch_cancel_a = format!("{batch_cancel_id}-a-cancel");
    let batch_cancel_b = format!("{batch_cancel_id}-b-cancel");
    provider
        .mkdir(&format!("/{batch_cancel_a}"))
        .await
        .unwrap();
    provider
        .mkdir(&format!("/{batch_cancel_b}"))
        .await
        .unwrap();
    let batch_cancel_plan = plan_for_roots(
        &provider,
        &[batch_cancel_b.clone(), batch_cancel_a.clone()],
    )
    .await;
    let batch_cancel = Arc::new(AtomicBool::new(false));
    let set_batch_cancel = batch_cancel.clone();
    let batch_cancel_result = MutationService::delete_webdav_trees(
        provider.clone(),
        batch_cancel_plan.sources,
        batch_cancel,
        move |progress| {
            if progress.completed == 1 {
                set_batch_cancel.store(true, Ordering::Release);
            }
        },
    )
    .await;
    assert!(matches!(
        batch_cancel_result,
        Err(WebDavDeleteError::Cancelled {
            completed: 1,
            total: 2
        })
    ));
    assert!(!root_exists(&provider, &batch_cancel_a).await);
    assert!(root_exists(&provider, &batch_cancel_b).await);
    cleanup_tree(&provider, &batch_cancel_b).await;

    // A deterministic failure in root B occurs only after root A has been
    // definitively deleted. Report global partial truth and stop before B's
    // later peer/root.
    let batch_locked_id = physical_run_id();
    let batch_locked_a = format!("{batch_locked_id}-a-first");
    let batch_locked_b = format!("{batch_locked_id}-b-locked");
    provider
        .mkdir(&format!("/{batch_locked_a}"))
        .await
        .unwrap();
    provider
        .mkdir(&format!("/{batch_locked_b}"))
        .await
        .unwrap();
    put(
        &provider,
        &format!("/{batch_locked_b}/a-locked.txt"),
        b"locked",
    )
    .await;
    put(
        &provider,
        &format!("/{batch_locked_b}/z-later.txt"),
        b"later",
    )
    .await;
    let batch_locked_path = format!("/{batch_locked_b}/a-locked.txt");
    let batch_token = lock_resource(&upstream, &batch_locked_path, &user, &pass)
        .await
        .unwrap();
    let batch_locked_plan = plan_for_roots(
        &provider,
        &[batch_locked_b.clone(), batch_locked_a.clone()],
    )
    .await;
    let batch_locked_result = MutationService::delete_webdav_trees(
        provider.clone(),
        batch_locked_plan.sources,
        Arc::new(AtomicBool::new(false)),
        |_| {},
    )
    .await;
    assert!(matches!(
        batch_locked_result,
        Err(WebDavDeleteError::Partial {
            completed: 1,
            total: 4,
            ..
        })
    ));
    assert!(!root_exists(&provider, &batch_locked_a).await);
    assert!(root_exists(&provider, &batch_locked_b).await);
    let batch_locked_names = list_root_names(&provider, &batch_locked_b).await;
    assert!(batch_locked_names.iter().any(|name| name == "a-locked.txt"));
    assert!(batch_locked_names.iter().any(|name| name == "z-later.txt"));
    unlock_resource(
        &upstream,
        &batch_locked_path,
        &batch_token,
        &user,
        &pass,
    )
    .await
    .unwrap();
    cleanup_tree(&provider, &batch_locked_b).await;

    // Ambiguity in root B: DELETE #1 (root A) is passed through normally;
    // DELETE #2 is processed by Apache but its response is withheld. ARX must
    // return RecoveryRequired at global completed=1 and issue no DELETE #3.
    let batch_ambiguous_id = physical_run_id();
    let batch_ambiguous_a = format!("{batch_ambiguous_id}-a-first");
    let batch_ambiguous_b = format!("{batch_ambiguous_id}-b-ambiguous");
    provider
        .mkdir(&format!("/{batch_ambiguous_a}"))
        .await
        .unwrap();
    provider
        .mkdir(&format!("/{batch_ambiguous_b}"))
        .await
        .unwrap();
    put(
        &provider,
        &format!("/{batch_ambiguous_b}/a-first.txt"),
        b"first",
    )
    .await;
    put(
        &provider,
        &format!("/{batch_ambiguous_b}/z-later.txt"),
        b"later",
    )
    .await;

    let batch_proxy = start_ambiguous_delete_proxy(&upstream, 2).await.unwrap();
    let batch_proxy_provider = provider_for(batch_proxy.listen_addr.clone(), &user, &pass);
    let batch_ambiguous_plan = plan_for_roots(
        &batch_proxy_provider,
        &[batch_ambiguous_b.clone(), batch_ambiguous_a.clone()],
    )
    .await;
    let batch_ambiguous_result = MutationService::delete_webdav_trees(
        batch_proxy_provider,
        batch_ambiguous_plan.sources,
        Arc::new(AtomicBool::new(false)),
        |_| {},
    )
    .await;
    assert!(matches!(
        batch_ambiguous_result,
        Err(WebDavDeleteError::RecoveryRequired {
            completed: 1,
            total: 4,
            ..
        })
    ));
    let batch_record = batch_proxy.record.lock().await;
    assert_eq!(
        batch_record.delete_count, 2,
        "later ambiguous DELETE must never replay or advance to DELETE #3"
    );
    assert!(
        batch_record.apache_response_seen,
        "proxy must prove Apache processed the second uncertain DELETE"
    );
    drop(batch_record);
    assert!(!root_exists(&provider, &batch_ambiguous_a).await);
    assert!(root_exists(&provider, &batch_ambiguous_b).await);
    let batch_ambiguous_names = list_root_names(&provider, &batch_ambiguous_b).await;
    let remaining_batch_children = batch_ambiguous_names
        .iter()
        .any(|name| name == "a-first.txt") as usize
        + batch_ambiguous_names
            .iter()
            .any(|name| name == "z-later.txt") as usize;
    assert_eq!(
        remaining_batch_children, 1,
        "no later node may be deleted after the second DELETE becomes ambiguous"
    );
    cleanup_tree(&provider, &batch_ambiguous_b).await;
}
