//! Test-only TCP proxy in front of real Apache mod_dav for W15/W17/W18.
//!
//! No new production dependency: pure `tokio::net`. One implementation with
//! modes, recording request metadata (method + path + selected headers) but
//! NEVER Authorization / password / Lock-Token bytes.
//!
//! Modes:
//!   PassThroughRecord — forward everything, record PUT count + headers.
//!   DropGetBody       — forward GET, stream first 1 chunk, then close ARX side.
//!   AmbiguousPut      — forward complete PUT, get Apache response, discard it,
//!                       close ARX side (ambiguous: server may have applied it).
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
    pub seen_if_none_match: bool,
}

impl ProxyRecord {
    fn new() -> Self {
        ProxyRecord {
            put_count: 0,
            seen_if_none_match: false,
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
        listen_addr: format!("http://127.0.0.1:{}/", listen_port),
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
    let method = first_line.split_whitespace().next().unwrap_or("");
    let mut is_put = false;
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
    if method.eq_ignore_ascii_case("PUT") {
        is_put = true;
        let mut r = rec.lock().await;
        r.put_count += 1;
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
            if method.eq_ignore_ascii_case("GET") {
                // Forward upstream response headers, then one body chunk, then
                // close the ARX-facing socket to simulate a mid-body drop.
                let mut resp = vec![0u8; 64 * 1024];
                let mut got = 0usize;
                loop {
                    if got >= resp.len() {
                        break;
                    }
                    match upstream.read(&mut resp[got..]).await {
                        Ok(n) if n > 0 => {
                            got += n;
                            if resp[..got].windows(4).any(|w| &w[..4] == b"\r\n\r\n") {
                                break;
                            }
                        }
                        _ => break,
                    }
                }
                let _ = client.write_all(&resp[..got]).await;
                // One small body chunk, then drop.
                let mut body = [0u8; 16];
                let n = upstream.read(&mut body).await;
                match n {
                    Ok(n) if n > 0 => {
                        let _ = client.write_all(&body[..n]).await;
                    }
                    _ => {}
                }
                // Close ARX side; leave upstream to timeout.
                let _ = client.shutdown().await;
            } else {
                pipe(&mut client, &mut upstream).await;
            }
        }
        ProxyMode::AmbiguousPut => {
            if is_put {
                // Forward complete body until client EOF.
                let _ = tokio::io::copy(&mut client, &mut upstream).await;
                // Discard upstream response, close ARX side (ambiguous).
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
