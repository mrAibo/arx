//! Test-only TCP proxy in front of real Apache mod_dav for W15/W17/W18.
//!
//! No new production dependency: pure `tokio::net`. One implementation with
//! modes, recording request metadata (method + path + selected headers) but
//! NEVER Authorization / password / Lock-Token bytes.
//!
//! Modes:
//!   PassThroughRecord — forward everything, record method counts + headers.
//!   DropGetBody       — forward the REAL Apache response, then truncate the
//!                       body mid-stream (real Apache status + partial body).
//!   AmbiguousPut      — forward complete PUT, read Apache's full response
//!                       (confirm server committed), then discard it and close
//!                       the ARX side (ambiguous: server may have applied it).
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
    /// W18: proxy observed Apache's response for the PUT (server committed).
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

async fn handle_conn(
    mut client: TcpStream,
    upstream_host: String,
    upstream_port: u16,
    _prefix: String,
    mode: ProxyMode,
    rec: Arc<Mutex<ProxyRecord>>,
) {
    // Read the request line + headers (bounded).
    let mut buf = vec![0u8; 64 * 1024];
    let mut filled = 0usize;
    loop {
        if filled >= buf.len() {
            return;
        }
        let n = match client.read(&mut buf[filled..]).await {
            Ok(n) if n > 0 => n,
            _ => return,
        };
        filled += n;
        if buf[..filled].windows(4).any(|w| &w[..4] == b"\r\n\r\n") {
            break;
        }
    }
    let header_text = String::from_utf8_lossy(&buf[..filled]);
    let first_line = header_text.lines().next().unwrap_or("");
    let method = first_line
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
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

    // Forward the captured request head.
    if upstream.write_all(&buf[..filled]).await.is_err() {
        return;
    }

    match mode {
        ProxyMode::PassThroughRecord => {
            pipe(&mut client, &mut upstream).await;
        }
        ProxyMode::DropGetBody => {
            if method == "GET" {
                // Forward the REAL Apache response, then truncate mid-body.
                // Read response head + first body chunk from Apache...
                let mut resp = vec![0u8; 64 * 1024];
                let mut got = 0usize;
                loop {
                    if got >= resp.len() {
                        break;
                    }
                    match upstream.read(&mut resp[got..]).await {
                        Ok(n) if n > 0 => {
                            got += n;
                            // Stop once we have the head and a little body.
                            if resp[..got].windows(4).any(|w| &w[..4] == b"\r\n\r\n") {
                                // Read a bit more real body, then truncate.
                                let mut tail = [0u8; 64];
                                match upstream.read(&mut tail).await {
                                    Ok(b) if b > 0 => {
                                        resp.extend_from_slice(&tail[..b]);
                                        got += b;
                                    }
                                    _ => {}
                                }
                                break;
                            }
                        }
                        _ => break,
                    }
                }
                // ...forward the real Apache head+partial body to ARX...
                let _ = client.write_all(&resp[..got]).await;
                // ...then abruptly close ARX side before the body completes.
                let _ = client.shutdown().await;
            } else {
                pipe(&mut client, &mut upstream).await;
            }
        }
        ProxyMode::AmbiguousPut => {
            if method == "PUT" {
                // Forward complete PUT body until client EOF (full request sent).
                let _ = tokio::io::copy(&mut client, &mut upstream).await;
                // Read Apache's FULL response to confirm the backend committed
                // the mutation. Do NOT forward it to ARX.
                let mut sink = vec![0u8; 64 * 1024];
                let mut total = 0usize;
                loop {
                    match upstream.read(&mut sink).await {
                        Ok(n) if n > 0 => {
                            total += n;
                            if total > sink.len() {
                                // already proved a response arrived
                                break;
                            }
                        }
                        _ => break,
                    }
                }
                if total > 0 {
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
