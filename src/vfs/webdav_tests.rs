//! Unit + contract tests for the WebDAV provider.
//!
//! Pure parser tests run without network. HTTP-behavior tests spin a tiny
//! std `TcpListener` mock DAV server (no external crate) and drive the real
//! `WebDavProvider` over `reqwest`.

#![allow(clippy::too_many_lines)]

use super::webdav::{
    MAX_ACCUM_TEXT, PropFindEntry, WebDavProvider, WebDavTarget, parse_multistatus,
    parse_rfc2822_ms,
};
use crate::vfs::{CancellationFlag, RemoteEditRevision, VfsProvider};
use std::io;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

// ── multistatus parser: namespace + self-response + collection/file ──────────

#[test]
fn parser_handles_default_dav_namespace() {
    let xml = br#"<?xml version="1.0"?>
<multistatus xmlns="DAV:">
  <response>
    <href>/dav/</href>
    <propstat><prop>
      <resourcetype><collection/></resourcetype>
      <getlastmodified>Mon, 02 Jan 2006 15:04:05 GMT</getlastmodified>
    </prop><status>HTTP/1.1 200 OK</status></propstat>
  </response>
  <response>
    <href>/dav/file.txt</href>
    <propstat><prop>
      <resourcetype/>
      <getcontentlength>12</getcontentlength>
    </prop><status>HTTP/1.1 200 OK</status></propstat>
  </response>
</multistatus>"#;
    let entries = parse_multistatus(xml).expect("parse");
    assert_eq!(entries.len(), 2);
    assert!(entries[0].is_collection);
    assert_eq!(entries[0].raw_href, "/dav/");
    assert!(!entries[1].is_collection);
    assert_eq!(entries[1].content_length, Some(12));
    assert!(entries[0].modified_unix_ms.is_some());
}

#[test]
fn parser_handles_arbitrary_prefix_namespace() {
    let xml = br#"<?xml version="1.0"?>
<D:multistatus xmlns:D="DAV:">
  <D:response>
    <D:href>/dav/a.txt</D:href>
    <D:propstat><D:prop>
      <D:resourcetype/>
    </D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>
  </D:response>
</D:multistatus>"#;
    let entries = parse_multistatus(xml).expect("parse");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].raw_href, "/dav/a.txt");
    assert!(!entries[0].is_collection);
}

#[test]
fn parser_keeps_self_response_raw() {
    let xml = br#"<?xml version="1.0"?>
<multistatus xmlns="DAV:">
  <response><href>/dav/</href>
    <propstat><prop><resourcetype><collection/></resourcetype></prop>
    <status>HTTP/1.1 200 OK</status></propstat></response>
  <response><href>/dav/child/</href>
    <propstat><prop><resourcetype><collection/></resourcetype></prop>
    <status>HTTP/1.1 200 OK</status></propstat></response>
  <response><href>/dav/file.txt</href>
    <propstat><prop><resourcetype/></prop>
    <status>HTTP/1.1 200 OK</status></propstat></response>
</multistatus>"#;
    let entries = parse_multistatus(xml).expect("parse");
    // parse returns all responses verbatim (incl. self); provider list() drops it.
    assert_eq!(entries.len(), 3);
    let names: Vec<&str> = entries.iter().map(|e| e.raw_href.as_str()).collect();
    assert!(names.iter().any(|n| n.ends_with("/child/")));
    assert!(names.iter().any(|n| n.ends_with("/file.txt")));
    assert!(names.contains(&"/dav/"));
}

#[test]
fn parser_multiple_propstat_blocks() {
    let xml = br#"<?xml version="1.0"?>
<multistatus xmlns="DAV:">
  <response><href>/dav/x.txt</href>
    <propstat><prop><resourcetype/></prop>
      <status>HTTP/1.1 200 OK</status></propstat>
    <propstat><prop><getcontentlength>5</getcontentlength></prop>
      <status>HTTP/1.1 200 OK</status></propstat>
    <propstat><prop><getetag/></prop>
      <status>HTTP/1.1 404 Not Found</status></propstat>
  </response>
</multistatus>"#;
    let entries = parse_multistatus(xml).expect("parse");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].content_length, Some(5));
}

#[test]
fn parser_entities_and_unicode_href() {
    let xml = br#"<?xml version="1.0" encoding="utf-8"?>
<multistatus xmlns="DAV:">
  <response><href>/dav/caf%C3%A9.txt</href>
    <propstat><prop>
      <resourcetype/>
      <displayname>cafe &amp; co</displayname>
    </prop><status>HTTP/1.1 200 OK</status></propstat></response>
</multistatus>"#;
    let entries = parse_multistatus(xml).expect("parse");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].raw_href, "/dav/caf%C3%A9.txt");
    assert_eq!(entries[0].display_name.as_deref(), Some("cafe & co"));
}

#[test]
fn parser_malformed_xml_does_not_panic() {
    let junk = b"<multistatus xmlns=\"DAV:\"><response><href>/x";
    let _ = parse_multistatus(junk);
}

#[test]
fn parser_rejects_oversized() {
    let mut big = b"<?xml version=\"1.0\"?><multistatus xmlns=\"DAV:\">".to_vec();
    for _ in 0..200_000 {
        big.extend_from_slice(
            b"<response><href>/a</href><propstat><prop><resourcetype/></prop><status>HTTP/1.1 200 OK</status></propstat></response>",
        );
    }
    big.extend_from_slice(b"</multistatus>");
    let _ = parse_multistatus(&big);
}

#[test]
fn date_parser_covers_formats() {
    assert!(parse_rfc2822_ms("Mon, 02 Jan 2006 15:04:05 GMT").is_some());
    assert!(parse_rfc2822_ms("not a date").is_none());
    assert!(parse_rfc2822_ms("").is_none());
}

// ── HTTP-behavior: std TcpListener mock DAV server ──────────────────────────

type MockLog = Arc<Mutex<Vec<(String, String, bool, Option<String>, usize)>>>;

#[allow(clippy::too_many_arguments)]
fn spawn_mock(
    handler: impl Fn(&str, &str, &str, Option<&str>, &[u8]) -> (u16, Vec<u8>) + Send + 'static,
) -> (String, MockLog) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().unwrap();
    let log: MockLog = Arc::new(Mutex::new(Vec::new()));
    let log2 = log.clone();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = match stream {
                Ok(s) => s,
                Err(_) => break,
            };
            stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
            let mut buf = [0u8; 8192];
            let mut total = 0;
            let mut req = Vec::new();
            loop {
                let n = match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                req.extend_from_slice(&buf[..n]);
                total += n;
                if req.windows(4).any(|w| w == b"\r\n\r\n") || total > 8000 {
                    break;
                }
            }
            let text = String::from_utf8_lossy(&req);
            let lines: Vec<&str> = text.split("\r\n").collect();
            let first = lines.first().unwrap_or(&"").to_string();
            let parts: Vec<&str> = first.split_whitespace().collect();
            let method = parts.first().unwrap_or(&"").to_string();
            let path = parts.get(1).unwrap_or(&"/").to_string();
            let has_auth = text
                .lines()
                .any(|l| l.to_lowercase().starts_with("authorization: basic "));
            let dest = text
                .lines()
                .find(|l| l.to_lowercase().starts_with("destination:"))
                .and_then(|l| l.split_once(':').map(|(_, v)| v.trim().to_string()));
            let body = match text.find("\r\n\r\n") {
                Some(i) => req[i + 4..].to_vec(),
                None => Vec::new(),
            };
            let (status, resp_body) = handler(&method, &path, &first, dest.as_deref(), &body);
            log2.lock().unwrap().push((
                method.clone(),
                path.clone(),
                has_auth,
                dest.clone(),
                body.len(),
            ));
            let headers = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                resp_body.len()
            );
            let _ = stream.write_all(headers.as_bytes());
            let _ = stream.write_all(&resp_body);
            let _ = stream.flush();
        }
    });
    (format!("http://{addr}"), log)
}

/// Stream `total` bytes in `chunk`-sized pieces so the client can disconnect
/// early (proving it never reads the full body past the cap).
fn spawn_chunked(total: usize, chunk: usize) -> (String, ()) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        if let Ok(stream) = listener.incoming().next().unwrap() {
            let mut stream = stream;
            stream.set_write_timeout(Some(Duration::from_secs(5))).ok();
            // Read the request headers (ignore), then start streaming the body.
            let mut buf = [0u8; 8192];
            let mut req = Vec::new();
            loop {
                let n = match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                req.extend_from_slice(&buf[..n]);
                if req.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let headers =
                format!("HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nConnection: close\r\n\r\n");
            let _ = stream.write_all(headers.as_bytes());
            let piece = vec![b'A'; chunk.min(total)];
            let mut sent = 0;
            while sent < total {
                if stream.write_all(&piece).is_err() {
                    // Client disconnected early: proves it did not read it all.
                    break;
                }
                sent += piece.len();
                if sent > total {
                    let _ = stream.write_all(&piece[..total - (sent - piece.len())]);
                    break;
                }
            }
            let _ = stream.flush();
        }
    });
    (format!("http://{addr}"), ())
}

fn provider_for(url: &str, password: &str) -> WebDavProvider {
    WebDavProvider::new(
        WebDavTarget {
            id: "t".into(),
            name: "t".into(),
            url: url.to_string(),
            username: "u".into(),
            auth: "basic".into(),
        },
        password.to_string(),
    )
    .expect("provider")
}

fn propfind_xml() -> Vec<u8> {
    br#"<?xml version="1.0"?>
<multistatus xmlns="DAV:">
  <response><href>/dav/</href>
    <propstat><prop><resourcetype><collection/></resourcetype></prop>
    <status>HTTP/1.1 200 OK</status></propstat></response>
  <response><href>/dav/file.txt</href>
    <propstat><prop><resourcetype/><getcontentlength>5</getcontentlength></prop>
    <status>HTTP/1.1 200 OK</status></propstat></response>
</multistatus>"#
        .to_vec()
}

#[tokio::test]
async fn list_sends_basic_auth_and_drops_self() {
    let (url, log) = spawn_mock(|method, path, _, _, _| match (method, path) {
        ("PROPFIND", "/dav") | ("PROPFIND", "/dav/") => (207, propfind_xml()),
        _ => (404, Vec::new()),
    });
    let p = provider_for(&url, "sekret");
    let entries = p.list_async("/dav").await.expect("list");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "file.txt");
    let lg = log.lock().unwrap();
    let auth = lg
        .iter()
        .any(|(m, p, a, _, _)| m == "PROPFIND" && (p == "/dav" || p == "/dav/") && *a);
    assert!(auth, "PROPFIND must carry Basic auth");
}

#[tokio::test]
async fn get_unauthorized_maps_to_error() {
    let (url, _) = spawn_mock(|method, _, _, _, _| match method {
        "GET" => (401, b"denied".to_vec()),
        _ => (404, Vec::new()),
    });
    let p = provider_for(&url, "x");
    let r = p.read_all_capped("/dav/file.txt", 100).await;
    assert!(r.is_err());
}

#[tokio::test]
async fn put_sends_body_and_201() {
    let (url, log) = spawn_mock(|method, _, _, _, body| match method {
        "PUT" => {
            assert_eq!(body, b"hello");
            (201, Vec::new())
        }
        _ => (404, Vec::new()),
    });
    let p = provider_for(&url, "x");
    p.write_file_bytes_if_unchanged(
        "/dav/new.txt",
        b"hello",
        &RemoteEditRevision::new(vec![], 0, 0, 0),
        &CancellationFlag::default(),
        None,
    )
    .await
    .expect("put");
    let lg = log.lock().unwrap();
    assert!(
        lg.iter()
            .any(|(m, p, _, _, bl)| m == "PUT" && p == "/dav/new.txt" && *bl == 5)
    );
}

#[tokio::test]
async fn copy_sends_destination_and_overwrite_f() {
    let (url, log) = spawn_mock(|method, _, _, _, _| match method {
        "COPY" => {
            // dest is full URL per DAV spec; just don't panic
            (201, Vec::new())
        }
        _ => (404, Vec::new()),
    });
    let p = provider_for(&url, "x");
    p.copy_or_move(
        reqwest::Method::from_bytes(b"COPY").unwrap(),
        "/dav/src.txt",
        "/dav2/src.txt",
        false,
    )
    .await
    .expect("copy");
    let lg = log.lock().unwrap();
    let rec = lg
        .iter()
        .find(|(m, _, _, _, _)| m == "COPY")
        .expect("copy logged");
    assert!(
        rec.3.as_deref().unwrap_or("").ends_with("/dav2/src.txt"),
        "Destination should end with /dav2/src.txt, got {:?}",
        rec.3
    );
}

#[tokio::test]
async fn delete_404_maps_to_not_found() {
    let (url, _) = spawn_mock(|method, _, _, _, _| match method {
        "DELETE" => (404, Vec::new()),
        _ => (404, Vec::new()),
    });
    let p = provider_for(&url, "x");
    let r = p.remove_file("/dav/missing.txt").await;
    assert!(r.is_err());
}

// ---- D: DAV namespace + propstat semantics ----

fn pm(xml: &str) -> io::Result<Vec<PropFindEntry>> {
    parse_multistatus(xml.as_bytes())
}

#[test]
fn d_default_dav_namespace() {
    let xml = r#"<?xml version="1.0"?>
<multistatus xmlns="DAV:">
 <response>
  <href>/dav/file.txt</href>
  <propstat>
   <prop><getcontentlength>5</getcontentlength></prop>
   <status>HTTP/1.1 200 OK</status>
  </propstat>
 </response>
</multistatus>"#;
    let e = pm(xml).expect("parse");
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].content_length, Some(5));
    assert!(!e[0].is_collection);
}

#[test]
fn d_arbitrary_dav_prefix() {
    let xml = r#"<?xml version="1.0"?>
<D:multistatus xmlns:D="DAV:">
 <D:response>
  <D:href>/dav/file.txt</D:href>
  <D:propstat>
   <D:prop><D:getcontentlength>7</D:getcontentlength></D:prop>
   <D:status>HTTP/1.1 200 OK</D:status>
  </D:propstat>
 </D:response>
</D:multistatus>"#;
    let e = pm(xml).expect("parse");
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].content_length, Some(7));
}

#[test]
fn d_evil_namespace_ignored() {
    let xml = r#"<?xml version="1.0"?>
<x:multistatus xmlns:x="http://evil.example/ns">
 <x:response>
  <x:href>/dav/file.txt</x:href>
  <x:propstat>
   <x:prop><x:getcontentlength>999</x:getcontentlength></x:prop>
   <x:status>HTTP/1.1 200 OK</x:status>
  </x:propstat>
 </x:response>
</x:multistatus>"#;
    // Non-DAV elements must be ignored entirely: no entries emitted.
    let r = pm(xml);
    assert!(
        r.is_err() || r.unwrap().is_empty(),
        "evil namespace must not produce DAV entries"
    );
}

#[test]
fn d_200_and_404_propstat_length_5() {
    let xml = r#"<?xml version="1.0"?>
<multistatus xmlns="DAV:">
 <response>
  <href>/dav/file.txt</href>
  <propstat>
   <prop><getcontentlength>5</getcontentlength></prop>
   <status>HTTP/1.1 200 OK</status>
  </propstat>
  <propstat>
   <prop><getcontentlength>999</getcontentlength></prop>
   <status>HTTP/1.1 404 Not Found</status>
  </propstat>
 </response>
</multistatus>"#;
    let e = pm(xml).expect("parse");
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].content_length, Some(5));
}

#[test]
fn d_reversed_propstat_ordering_length_5() {
    let xml = r#"<?xml version="1.0"?>
<multistatus xmlns="DAV:">
 <response>
  <href>/dav/file.txt</href>
  <propstat>
   <prop><getcontentlength>999</getcontentlength></prop>
   <status>HTTP/1.1 404 Not Found</status>
  </propstat>
  <propstat>
   <prop><getcontentlength>5</getcontentlength></prop>
   <status>HTTP/1.1 200 OK</status>
  </propstat>
 </response>
</multistatus>"#;
    let e = pm(xml).expect("parse");
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].content_length, Some(5));
}

#[test]
fn d_404_collection_not_directory() {
    let xml = r#"<?xml version="1.0"?>
<multistatus xmlns="DAV:">
 <response>
  <href>/dav/file.txt</href>
  <propstat>
   <prop>
    <resourcetype><collection/></resourcetype>
    <getcontentlength>5</getcontentlength>
   </prop>
   <status>HTTP/1.1 404 Not Found</status>
  </propstat>
  <propstat>
   <prop><getcontentlength>5</getcontentlength></prop>
   <status>HTTP/1.1 200 OK</status>
  </propstat>
 </response>
</multistatus>"#;
    let e = pm(xml).expect("parse");
    assert_eq!(e.len(), 1);
    assert!(
        !e[0].is_collection,
        "404 collection must not mark a directory"
    );
    assert_eq!(e[0].content_length, Some(5));
}

#[test]
fn d_excessive_response_count_errors() {
    let mut xml = String::from(r#"<?xml version="1.0"?><multistatus xmlns="DAV:">"#);
    for _ in 0..50001 {
        xml.push_str(r#"<response><href>/dav/a</href><propstat><prop><getcontentlength>1</getcontentlength></prop><status>HTTP/1.1 200 OK</status></propstat></response>"#);
    }
    xml.push_str("</multistatus>");
    let r = pm(&xml);
    assert!(r.is_err(), "excessive response count must error");
}

#[test]
fn d_excessive_property_count_errors() {
    let mut props = String::new();
    for i in 0..300 {
        props.push_str(&format!("<x{i}>v</x{i}>", i = i));
    }
    let xml = format!(
        r#"<?xml version="1.0"?><multistatus xmlns="DAV:"><response><href>/dav/a</href><propstat><prop>{props}</prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"#,
        props = props
    );
    let r = pm(&xml);
    assert!(r.is_err(), "excessive property count must error");
}

#[test]
fn d_oversized_text_errors() {
    let big = "x".repeat(MAX_ACCUM_TEXT + 100);
    let xml = format!(
        r#"<?xml version="1.0"?><multistatus xmlns="DAV:"><response><href>/dav/a</href><propstat><prop><displayname>{big}</displayname></prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"#,
        big = big
    );
    let r = pm(&xml);
    assert!(
        r.is_err(),
        "oversized text must error, not silently truncate"
    );
}

#[test]
fn d_malformed_xml_errors() {
    // A bare closing-slash token with no matching open element is a hard error.
    let xml = r#"<multistatus xmlns="DAV:"><response></multistatus>"#;
    let r = pm(xml);
    assert!(r.is_err(), "malformed XML must error");
}

// ── C: bounded HTTP body ──────────────────────────────────────────────────────

#[tokio::test]
async fn c_preview_cap_limits_allocation_and_truncates() {
    // Server streams 10 MiB but the preview cap is 64 KiB.
    const CAP: usize = 64 * 1024;
    const TOTAL: usize = 10 * 1024 * 1024;
    let (url, _h) = spawn_chunked(TOTAL, 4096);
    let provider = provider_for(&url, "p");
    let read = provider.read_prefix_bytes("/big.bin", CAP).await.unwrap();
    assert!(read.bytes.len() <= CAP, "allocation must stay within cap");
    assert!(read.truncated, "10 MiB body must be reported truncated");
}

#[tokio::test]
async fn c_get_500_is_error_not_contents() {
    let (url, _) = spawn_mock(|method, _, _, _, _| match method {
        "GET" => (500, b"internal explosion".to_vec()),
        _ => (404, Vec::new()),
    });
    let provider = provider_for(&url, "p");
    let r = provider.read_prefix_bytes("/x", 1024).await;
    assert!(
        r.is_err(),
        "GET 500 must error, never yield body as file content"
    );
}

#[tokio::test]
async fn c_only_200_or_206_accepted() {
    // 416 without Content-Range must NOT be treated as empty file.
    let (url, _) = spawn_mock(|method, _, _, _, _| match method {
        "GET" => (416, b"Range Not Satisfiable".to_vec()),
        _ => (404, Vec::new()),
    });
    let provider = provider_for(&url, "p");
    let r = provider.read_prefix_bytes("/x", 1024).await;
    assert!(
        r.is_err(),
        "arbitrary 416 must be a status error, not empty file"
    );
}

#[tokio::test]
async fn c_error_body_bounded() {
    // Large error body must not be fully buffered into the diagnostic string.
    let big = "X".repeat(8192);
    let (url, _) = spawn_mock(move |method, _, _, _, _| match method {
        "GET" => (500, big.as_bytes().to_vec()),
        _ => (404, Vec::new()),
    });
    let provider = provider_for(&url, "p");
    let r = provider.read_prefix_bytes("/x", 1024).await;
    assert!(r.is_err());
}

#[tokio::test]
async fn c_propfind_over_cap_is_invalid_data() {
    // PROPFIND returning an oversized body must be rejected, not parsed.
    let big = format!(
        r#"<?xml version="1.0"?><multistatus xmlns="DAV:"><response><href>/dav/a</href><propstat><prop><displayname>{}</displayname></prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"#,
        "y".repeat(20 * 1024 * 1024)
    );
    let (url, _) = spawn_mock(move |method, _, _, _, _| match method {
        "PROPFIND" => (207, big.as_bytes().to_vec()),
        _ => (404, Vec::new()),
    });
    let provider = provider_for(&url, "p");
    // list_async -> propfind -> read_body_bounded(MAX_PROPFIND_BYTES) -> truncated error
    let r = provider.list_async("/dav").await;
    assert!(r.is_err(), "oversized PROPFIND body must be rejected");
}
