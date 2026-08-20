//! Test-only TCP proxy in front of real Apache mod_dav for W15/W17/W18.
//!
//! No new production dependency: pure `tokio::net`. One implementation with
//! modes, recording request metadata (method + path + selected headers) but
//! NEVER Authorization / password / Lock-Token bytes.
//!
//! Modes:
//!   PassThroughRecord — forward everything, record method counts + headers.
//!   DropGetBody       — forward the REAL Apache response head + only part of
//!                       its real body, then close before Content-Length bytes.
//!   AmbiguousPut      — forward the complete PUT by Content-Length, observe
//!                       Apache's response head (confirm server completed), then
//!                       discard it and close the ARX side.
//!
//! ponytail: test infra only, compiled under `physical-webdav`. Not a product.

#![cfg(feature = "physical-webdav")]

use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ProxyMode {
    PassThroughRecord,
    DropGetBody,
    AmbiguousPut,
}

pub struct ProxyRecord {
    pub put_count: usize,
    pub get_count: usize,
    pub head_count: usize,
    pub propfind_count: usize,
    pub seen_if_none_match: bool,
    /// W18: proxy observed a complete Apache response head for the PUT.
    pub apache_response_seen: bool,
}

impl ProxyRecord {
    fn new() -> Self {
        ProxyRecord {
            put_count: 0,
            get_count: 0,
            head_count: 0,
            propfind_count: 0,
            seen_if_none_match: false,
            apache_response_seen: false,
        }
    }
}

/// A running proxy; drop stops the accept loop.
pub struct TestProxy {
    pub listen_addr: String,
    pub record: Arc<Mutex<ProxyRecord>>,
}

/// Start a proxy in front of `upstream_url` (e.g. http://127.0.0.1:PORT/dav/).
/// Returns the proxy base URL the WebDAV client should target.
pub async fn start_proxy(upstream_url: &str, mode: ProxyMode) -> std::io::Result<TestProxy> {
    let upstream = url::Url::parse(upstream_url)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string()))?;
    let host = upstream.host_str().unwrap_or("127.0.0.1").to_string();
    let port = upstream.port_or_known_default().unwrap_or(80);
    let path_prefix = upstream.path().trim_end_matches('/').to_string();

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let listen_port = listener.local_addr()?.port();

    let record = Arc::new(Mutex::new(ProxyRecord::new()));
    let rec = record.clone();
    let upstream_host = host.clone();
    let upstream_port = port;
    let prefix = path_prefix.clone();

    tokio::spawn(async move {
        loop {
            let (client, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => break,
            };
            let rec = rec.clone();
            let uh = upstream_host.clone();
            let up = upstream_port;
            let pfx = prefix.clone();
            tokio::spawn(handle_conn(client, uh, up, pfx, mode, rec));
        }
    });

    Ok(TestProxy {
        // Include the upstream DAV root path so the client's hrefs land inside
        // Apache's DavRoot without the proxy rewriting request paths.
        listen_addr: format!("http://127.0.0.1:{}/dav/", listen_port),
        record,
    })
}

fn header_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4)
}

fn content_length(headers: &[u8]) -> Option<usize> {
    let text = String::from_utf8_lossy(headers);
    text.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.eq_ignore_ascii_case("Content-Length") {
            value.trim().parse().ok()
        } else {
            None
        }
    })
}

async fn handle_conn(
    mut client: TcpStream,
    upstream_host: String,
    upstream_port: u16,
    _prefix: String,
    mode: ProxyMode,
    rec: Arc<Mutex<ProxyRecord>>,
) {
    // Read the request line + headers (bounded). The final read may also contain
    // some request-body bytes; `request_head_end` lets fault modes account for
    // those bytes exactly instead of waiting for a keep-alive EOF.
    let mut buf = vec![0u8; 64 * 1024];
    let mut filled = 0usize;
    let request_head_end = loop {
        if filled >= buf.len() {
            return;
        }
        let n = match client.read(&mut buf[filled..]).await {
            Ok(n) if n > 0 => n,
            _ => return,
        };
        filled += n;
        if let Some(end) = header_end(&buf[..filled]) {
            break end;
        }
    };
    let header_text = String::from_utf8_lossy(&buf[..request_head_end]);
    let first_line = header_text.lines().next().unwrap_or("");
    let method = first_line
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    let request_content_length = content_length(&buf[..request_head_end]);
    let mut seen_inm = false;
    for line in header_text.lines().skip(1) {
        let (k, _v) = match line.split_once(':') {
            Some(kv) => kv,
            None => continue,
        };
        // Record only non-secret headers.
        if k.eq_ignore_ascii_case("If-None-Match") {
            seen_inm = true;
        }
        if k.eq_ignore_ascii_case("Authorization")
            || k.eq_ignore_ascii_case("Lock-Token")
            || k.eq_ignore_ascii_case("X-Lock-Token")
        {
            // explicitly NOT recorded
            continue;
        }
    }
    // Record safe method counts (never Authorization/password/Lock-Token).
    {
        let mut r = rec.lock().await;
        match method.as_str() {
            "PUT" => r.put_count += 1,
            "GET" => r.get_count += 1,
            "HEAD" => r.head_count += 1,
            "PROPFIND" => r.propfind_count += 1,
            _ => {}
        }
        if seen_inm {
            r.seen_if_none_match = true;
        }
    }

    // Connect to real Apache.
    let mut upstream = match TcpStream::connect((upstream_host.as_str(), upstream_port)).await {
        Ok(u) => u,
        Err(_) => return,
    };

    // Forward everything already captured (headers plus any body bytes that
    // arrived in the same read).
    if upstream.write_all(&buf[..filled]).await.is_err() {
        return;
    }

    match mode {
        ProxyMode::PassThroughRecord => {
            pipe(&mut client, &mut upstream).await;
        }
        ProxyMode::DropGetBody => {
            if method == "GET" {
                // Read through the REAL Apache response header. The read that
                // finds CRLFCRLF may already contain the whole short body, so we
                // deliberately forward only < Content-Length bytes from it.
                let mut resp = vec![0u8; 64 * 1024];
                let mut got = 0usize;
                let response_head_end = loop {
                    if got >= resp.len() {
                        return;
                    }
                    let n = match upstream.read(&mut resp[got..]).await {
                        Ok(n) if n > 0 => n,
                        _ => return,
                    };
                    got += n;
                    if let Some(end) = header_end(&resp[..got]) {
                        break end;
                    }
                };

                let declared = match content_length(&resp[..response_head_end]) {
                    Some(n) if n > 0 => n,
                    _ => return,
                };
                let partial_target = declared.saturating_sub(1).min(8);
                let captured_body = got.saturating_sub(response_head_end);
                let mut partial_body = resp
                    [response_head_end..response_head_end + captured_body.min(partial_target)]
                    .to_vec();

                while partial_body.len() < partial_target {
                    let need = partial_target - partial_body.len();
                    let mut tail = [0u8; 64];
                    let want = need.min(tail.len());
                    match upstream.read(&mut tail[..want]).await {
                        Ok(n) if n > 0 => partial_body.extend_from_slice(&tail[..n]),
                        _ => break,
                    }
                }

                // Preserve Apache's real status and headers, including its
                // original Content-Length, but deliver fewer real body bytes.
                let _ = client.write_all(&resp[..response_head_end]).await;
                let _ = client.write_all(&partial_body).await;
                let _ = client.shutdown().await;
            } else {
                pipe(&mut client, &mut upstream).await;
            }
        }
        ProxyMode::AmbiguousPut => {
            if method == "PUT" {
                // reqwest sends this Vec-backed PUT with Content-Length. Forward
                // exactly the remaining declared bytes; do not wait for EOF on
                // the keep-alive client connection.
                let declared = match request_content_length {
                    Some(n) => n,
                    None => return,
                };
                let captured_body = filled.saturating_sub(request_head_end);
                if captured_body > declared {
                    return;
                }
                let mut remaining = declared - captured_body;
                let mut body_buf = [0u8; 16 * 1024];
                while remaining > 0 {
                    let want = remaining.min(body_buf.len());
                    let n = match client.read(&mut body_buf[..want]).await {
                        Ok(n) if n > 0 => n,
                        _ => return,
                    };
                    if upstream.write_all(&body_buf[..n]).await.is_err() {
                        return;
                    }
                    remaining -= n;
                }
                if upstream.flush().await.is_err() {
                    return;
                }

                // Receiving a complete Apache response head proves the server
                // finished processing the PUT. Intentionally discard it so ARX
                // cannot know whether the mutation committed.
                let mut response = vec![0u8; 64 * 1024];
                let mut response_len = 0usize;
                let saw_response_head = loop {
                    if response_len >= response.len() {
                        break false;
                    }
                    let n = match upstream.read(&mut response[response_len..]).await {
                        Ok(n) if n > 0 => n,
                        _ => break false,
                    };
                    response_len += n;
                    if header_end(&response[..response_len]).is_some() {
                        break true;
                    }
                };
                if saw_response_head {
                    let mut r = rec.lock().await;
                    r.apache_response_seen = true;
                }
                // Close ARX side: ARX never learns the outcome (ambiguous).
                let _ = client.shutdown().await;
            } else {
                pipe(&mut client, &mut upstream).await;
            }
        }
    }
}

/// Bidirectional pipe until either side closes.
async fn pipe(a: &mut TcpStream, b: &mut TcpStream) {
    let (mut ra, mut wa) = a.split();
    let (mut rb, mut wb) = b.split();
    let t1 = tokio::io::copy(&mut ra, &mut wb);
    let t2 = tokio::io::copy(&mut rb, &mut wa);
    let _ = tokio::join!(t1, t2);
}
