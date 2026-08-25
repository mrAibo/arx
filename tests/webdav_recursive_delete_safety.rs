#![cfg(feature = "physical-webdav")]

use arx::services::{MutationService, WebDavDeleteError, WebDavRecursiveDeletePlan, prepare_webdav_recursive_delete};
use arx::vfs::webdav::{WebDavProvider, WebDavTarget};
use arx::vfs::{
    CancellationFlag, Location, RemoteEditRevision, VfsProvider,
};
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

async fn list_root_children(provider: &WebDavProvider, root_name: &str) -> usize {
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
        .len()
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

async fn start_ambiguous_delete_proxy(upstream_url: &str) -> io::Result<TestProxy> {
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

    if method == "DELETE" {
        record.lock().await.delete_count += 1;
    }

    let mut upstream = match TcpStream::connect((upstream_host.as_str(), upstream_port)).await {
        Ok(stream) => stream,
        Err(_) => return,
    };
    if upstream.write_all(&request[..filled]).await.is_err() {
        return;
    }

    if method != "DELETE" {
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
    assert_eq!(list_root_children(&provider, &cancel_root).await, 1);
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
    put(
        &provider,
        &format!("/{locked_root}/z-later.txt"),
        b"later",
    )
    .await;
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
    assert_eq!(list_root_children(&provider, &locked_root).await, 2);
    unlock_resource(&upstream, &locked_path, &token, &user, &pass)
        .await
        .unwrap();
    cleanup_tree(&provider, &locked_root).await;

    // Ambiguous DELETE: proxy forwards the first exact DELETE to Apache and
    // observes Apache's response, but withholds it from ARX. The destructive
    // state is therefore uncertain to ARX: RecoveryRequired, exactly one
    // DELETE request, and no later manifest node may be attempted.
    let ambiguous_root = format!("{}-ambiguous", physical_run_id());
    provider
        .mkdir(&format!("/{ambiguous_root}"))
        .await
        .unwrap();
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

    let proxy = start_ambiguous_delete_proxy(&upstream).await.unwrap();
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
    assert_eq!(
        list_root_children(&provider, &ambiguous_root).await,
        1,
        "no later manifest node may be deleted after ambiguity"
    );
    cleanup_tree(&provider, &ambiguous_root).await;
}
