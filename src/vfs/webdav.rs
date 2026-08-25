//! Production WebDAV `VfsProvider`.
//!
//! Replaces the original prototype: no plaintext password config, no manual
//! string-split XML parsing, real DAV semantics (PROPFIND/GET/PUT/DELETE/
//! MKCOL/COPY/MOVE), truthful status→error mapping, raw href identity retained
//! separately from display, and secret bytes resolved from the OS keyring or
//! the `ARX_WEBDAV_<ID>_PASSWORD` env var (never stored in config).
//!
//! Reference studies (behavior only, no code copied):
//! - rclone/backend/webdav: Depth/propstat/Destination/Overwrite semantics,
//!   redirect credential safety, 401/403/404/409/412/423 mapping.
//! - Stalwart DAV: multistatus shapes, multiple propstat, namespace variants.
//!   Implemented as clean ARX-native code; MIT rclone patterns adapted only at
//!   the behavioral level.

use crate::transfer::WebDavOverwritePolicy;
use crate::vfs::{
    BoundedRead, Entry, EntryIdentity, EntryKind, FileMetadata, ListedEntry, Location,
    ProviderContinuation, ProviderListingPage, RemoteEditProgressFn, RemoteEditRevision,
    VfsProvider,
};
use quick_xml::events::Event;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

/// Hard caps for multistatus parsing (quick-xml security boundary).
const MAX_PROPFIND_BYTES: usize = 16 * 1024 * 1024;
const MAX_RESPONSES: usize = 50_000;
pub(crate) const MAX_ACCUM_TEXT: usize = 64 * 1024;

/// Minimal config: no secret fields. `url` is the absolute target root.
#[derive(Clone, PartialEq, Eq)]
pub struct WebDavTarget {
    pub id: String,
    pub name: String,
    pub url: String,
    pub username: String,
    pub auth: String,
}

impl std::fmt::Debug for WebDavTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebDavTarget")
            .field("id", &self.id)
            .field("name", &self.name)
            // ponytail: output-only redaction; stored url kept verbatim
            .field("url", &redact_url_userinfo(&self.url))
            .field("username", &self.username)
            .field("auth", &self.auth)
            .finish()
    }
}

/// Output-only: redact userinfo from a URL for diagnostics. Stored value
/// untouched. Replaces `user:pass@` with `user:***@`.
pub(crate) fn redact_url_userinfo(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => {
            if let Some((userinfo, host)) = rest.split_once('@') {
                if let Some(user) = userinfo.rsplit_once(':') {
                    return format!("{}://{}:***@{}", scheme, user.0, host);
                }
                return format!("{}://***@{}", scheme, host);
            }
            url.to_string()
        }
        None => url.to_string(),
    }
}

/// One parsed resource from a PROPFIND multistatus response.
#[derive(Debug, Clone)]
pub(crate) struct PropFindEntry {
    /// Raw href exactly as the server returned it (remote identity).
    pub(crate) raw_href: String,
    /// Decoded, presentation-only name (not used for addressing).
    pub(crate) display_name: Option<String>,
    pub(crate) is_collection: bool,
    pub(crate) content_length: Option<u64>,
    pub(crate) modified_unix_ms: Option<u64>,
}

/// Provider-native object identity: exact target id + raw href returned by the
/// server. Never reconstructed from `parent + entry.name` (href may carry
/// percent-encoding, query, or opaque server paths).
// ponytail: raw href is the authoritative WebDAV object key (F3/F5)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebDavObjectRef {
    pub target: String,
    pub href: String,
}

/// Provider-native collection identity: exact target id + raw href.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebDavCollectionRef {
    pub target: String,
    pub href: String,
}

/// A WebDAV provider bound to one configured target.
pub struct WebDavProvider {
    target: WebDavTarget,
    /// Resolved password (from keyring/ARX_WEBDAV_<ID>_PASSWORD at registry
    /// build time). Never exposed via Debug.
    password: String,
    /// Async client with TLS (rustls via reqwest defaults) + timeouts.
    client: reqwest::Client,
}

impl WebDavProvider {
    pub fn new(target: WebDavTarget, password: String) -> io::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()
            .map_err(|e| io::Error::other(format!("webdav client: {e}")))?;
        Ok(Self {
            target,
            password,
            client,
        })
    }

    /// Accessor for the bound target (target id, username, url, auth).
    pub fn target(&self) -> &WebDavTarget {
        &self.target
    }

    fn secret(&self) -> io::Result<String> {
        // Password already resolved from the keyring/env path at registry
        // build time (see ProviderRegistry::resolve_webdav_provider). `auth`
        // is only the scheme; never a secret here.
        Ok(self.password.clone())
    }

    /// Join the target root with a logical (decoded) path. Preserves the exact
    /// target root verbatim and applies per-segment percent-encoding. Does NOT
    /// normalize away duplicate slashes, trailing slashes, or Unicode.
    fn join_url(&self, path: &str) -> String {
        // Preserve the target root verbatim (including a trailing slash, which
        // Apache requires to address a collection). Only strip the root slash
        // when joining a non-empty sub-path.
        let root = &self.target.url;
        // The listing identity's raw href may carry the target's own path prefix
        // (e.g. "/dav/foo"); strip it so we don't double-prefix under the root.
        // ponytail: one prefix-strip; if the server returns an absolute-origin
        // href, url::Url parsing would be the upgrade path.
        let root_path = match root.split_once("://") {
            Some((_, rest)) => rest
                .split_once('/')
                .map(|(_, p)| p)
                .unwrap_or("")
                .trim_end_matches('/')
                .to_string(),
            None => String::new(),
        };
        let after_root = if !root_path.is_empty() {
            path.trim_start_matches('/')
                .strip_prefix(&root_path)
                .map(|s| s.trim_start_matches('/'))
                .unwrap_or_else(|| path.trim_start_matches('/'))
        } else {
            path.trim_start_matches('/')
        };
        if after_root.is_empty() {
            return root.to_string();
        }
        let root_base = root.trim_end_matches('/');
        let encoded: String = after_root
            .split('/')
            .map(encode_segment)
            .collect::<Vec<_>>()
            .join("/");
        format!("{root_base}/{encoded}")
    }

    /// Path portion of the configured target URL (e.g. `/dav`), no trailing
    /// slash. Empty when the target root is the origin itself.
    fn root_path(&self) -> String {
        let url = &self.target.url;
        if let Some(idx) = url.find("://") {
            let after = &url[idx + 3..];
            if let Some(slash) = after.find('/') {
                return after[slash..].trim_end_matches('/').to_string();
            }
        }
        String::new()
    }

    /// Resolve an authoritative raw href (server-supplied identity) into a fully
    /// qualified wire URL, strictly bounded to the target root.
    ///
    /// Fail-closed, origin-aware (scheme+host+port), using a real URL parser:
    ///
    /// Rejects:
    ///   - a href outside the target root (`/dav2/...`, `/other/...`),
    ///   - encoded path traversal (`/dav/%2e%2e/secret`),
    ///   - cross-origin absolute hrefs (different scheme/host/port),
    ///   - userinfo or fragment in an absolute href,
    ///   - relative href forms that are not path-absolute.
    ///
    /// The href is used VERBATIM (no percent re-encoding): a server `%20` stays
    /// `%20` and never becomes `%2520`.
    pub(crate) fn wire_url_for_href(&self, href: &str) -> io::Result<String> {
        let root = &self.target.url;
        let root_parsed = url::Url::parse(root).map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidInput, format!("bad target url: {e}"))
        })?;
        let root_path = root_parsed.path().trim_end_matches('/');

        // Absolute href: parse with a real URL parser and compare origin.
        if let Ok(h) = url::Url::parse(href) {
            if h.scheme() != root_parsed.scheme() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "href scheme mismatch",
                ));
            }
            if h.host_str() != root_parsed.host_str()
                || h.port_or_known_default() != root_parsed.port_or_known_default()
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "href origin mismatch",
                ));
            }
            if h.username() != "" || h.password().is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "href must not carry credentials",
                ));
            }
            if h.fragment().is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "href must not carry fragment",
                ));
            }
            // Same-origin absolute href: the server gave us an authoritative URL.
            // Return it verbatim (raw path + query), do NOT re-encode or drop the
            // query. Origin/creds/fragment already validated above.
            return Ok(href.to_string());
        }

        // Path-absolute href: preserve raw path/query verbatim, prefix origin.
        if href.starts_with('/') {
            return self.check_contained(root_path, href, href);
        }

        // Other relative forms (e.g. `a/b`, `./a`) are not supported for PACK E.
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsupported relative href: {href}"),
        ))
    }

    /// Containment + traversal check on an already-path (raw) href.
    fn check_contained(&self, root_path: &str, href_path: &str, raw: &str) -> io::Result<String> {
        // Fragments are never part of a wire URL.
        if raw.contains('#') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "href must not carry fragment",
            ));
        }
        let contained = root_path.is_empty()
            || href_path == root_path
            || href_path.starts_with(&format!("{root_path}/"));
        if !contained {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("href escapes target root: {raw}"),
            ));
        }
        for seg in href_path.trim_start_matches('/').split('/') {
            if seg.is_empty() {
                continue;
            }
            let dec = percent_decode(seg);
            if dec == ".." || dec == "." {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("href traversal rejected: {raw}"),
                ));
            }
        }
        let origin_base = self
            .target
            .url
            .trim_end_matches('/')
            .trim_end_matches(root_path);
        Ok(format!("{origin_base}{href_path}"))
    }

    /// Smart URL resolver: raw hrefs (under the target root) go through
    /// `wire_url_for_href` (verbatim, contained); logical paths (no root
    /// prefix, e.g. upload destinations) go through `join_url` (re-encoded).
    fn resolve_url(&self, path: &str) -> io::Result<String> {
        let rp = self.root_path();
        if !rp.is_empty() && path.starts_with(&rp) {
            self.wire_url_for_href(path)
        } else {
            Ok(self.join_url(path))
        }
    }

    fn auth_req(&self, req: reqwest::RequestBuilder) -> io::Result<reqwest::RequestBuilder> {
        let pw = self.secret()?;
        Ok(req.basic_auth(&self.target.username, Some(pw)))
    }

    /// Map an HTTP status + body into a truthful `io::Error`.
    fn status_error(&self, method: &str, status: reqwest::StatusCode, body: &str) -> io::Error {
        let msg = format!("{method} {}: {} ", status, summarize(body));
        match status.as_u16() {
            401 => io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("auth failed: {msg}"),
            ),
            403 => io::Error::new(io::ErrorKind::PermissionDenied, format!("forbidden: {msg}")),
            404 | 410 => io::Error::new(io::ErrorKind::NotFound, msg),
            405 => io::Error::new(
                io::ErrorKind::Unsupported,
                format!("method not allowed: {msg}"),
            ),
            409 => io::Error::new(io::ErrorKind::AlreadyExists, format!("conflict: {msg}")),
            412 => io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("precondition failed (overwrite not allowed): {msg}"),
            ),
            423 => io::Error::new(io::ErrorKind::PermissionDenied, format!("locked: {msg}")),
            400..=499 => {
                io::Error::new(io::ErrorKind::InvalidInput, format!("client error: {msg}"))
            }
            500..=599 => io::Error::other(format!("server error: {msg}")),
            _ => io::Error::other(msg),
        }
    }

    /// Join a destination path to a fully-qualified Destination header URL.
    #[allow(dead_code)] // wired through the async transfer executor (Blocker B)
    fn destination_url(&self, dst: &str) -> String {
        self.join_url(dst)
    }

    /// PROPFIND Depth:1 and parse the multistatus response.
    async fn propfind(&self, path: &str, depth: &str) -> io::Result<Vec<PropFindEntry>> {
        let url = self.join_url(path);
        let body = r#"<?xml version="1.0" encoding="utf-8"?><D:propfind xmlns:D="DAV:"><D:prop><D:resourcetype/><D:getcontentlength/><D:getlastmodified/><D:displayname/></D:prop></D:propfind>"#;
        let req = self.auth_req(
            self.client
                .request(reqwest::Method::from_bytes(b"PROPFIND").unwrap(), &url)
                .header("Depth", depth)
                .header("Content-Type", "application/xml; charset=utf-8")
                .body(body),
        )?;
        let resp = req.send().await.map_err(map_reqwest)?;
        let status = resp.status();
        if status != reqwest::StatusCode::OK && status != reqwest::StatusCode::MULTI_STATUS {
            let text = read_fixed_text(resp).await;
            return Err(self.status_error("PROPFIND", status, &text));
        }
        let bytes = read_body_bounded(resp, MAX_PROPFIND_BYTES).await?;
        if bytes.truncated {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "PROPFIND response exceeds size cap",
            ));
        }
        parse_multistatus(&bytes.bytes)
    }

    /// Bounded GET: request a Range, then read at most `max_bytes` and track
    /// truncation. If the server ignores Range and returns 200 with the full
    /// body, we still truncate locally (no unbounded memory). 416 means an
    /// empty file.
    pub(crate) async fn get_bounded(
        &self,
        path: &str,
        max_bytes: usize,
    ) -> io::Result<BoundedRead> {
        let url = self.resolve_url(path)?;
        let req = self.auth_req(
            self.client
                .get(&url)
                .header("Range", format!("bytes=0-{max_bytes}")),
        )?;
        let resp = req.send().await.map_err(map_reqwest)?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND
            || status == reqwest::StatusCode::FORBIDDEN
            || status == reqwest::StatusCode::UNAUTHORIZED
        {
            let text = read_fixed_text(resp).await;
            return Err(self.status_error("GET", status, &text));
        }
        // Only 200/206 carry a body we may read. Any other status (incl. 416
        // without proof of a zero-length resource via Content-Range) is a
        // protocol/status error, never treated as an empty file.
        if status != reqwest::StatusCode::OK && status != reqwest::StatusCode::PARTIAL_CONTENT {
            let text = read_fixed_text(resp).await;
            return Err(self.status_error("GET", status, &text));
        }
        let body = read_body_bounded(resp, max_bytes).await?;
        Ok(BoundedRead {
            bytes: body.bytes,
            truncated: body.truncated,
            unix_mode: None,
            unix_uid: None,
            unix_gid: None,
        })
    }

    /// Stream a GET response body into `sink`, chunk by chunk, bounded by
    /// `max_bytes`. Never buffers the whole object in memory (true streaming),
    /// unlike `get_bounded` which loads up to the cap first.
    /// Checks `cancel` between chunks for cooperative cancellation.
    pub(crate) async fn get_stream(
        &self,
        href: &str,
        max_bytes: usize,
        sink: &mut (impl tokio::io::AsyncWrite + Unpin),
        cancel: Option<&Arc<AtomicBool>>,
        pause: Option<&crate::transfer_queue::PauseGate>,
        mut on_progress: impl FnMut(u64, Option<u64>),
    ) -> io::Result<u64> {
        let url = self.resolve_url(href)?;
        let req = self.auth_req(self.client.get(&url))?;
        let mut resp = req.send().await.map_err(map_reqwest)?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND
            || status == reqwest::StatusCode::FORBIDDEN
            || status == reqwest::StatusCode::UNAUTHORIZED
        {
            let text = read_fixed_text(resp).await;
            return Err(self.status_error("GET", status, &text));
        }
        if status != reqwest::StatusCode::OK && status != reqwest::StatusCode::PARTIAL_CONTENT {
            let text = read_fixed_text(resp).await;
            return Err(self.status_error("GET", status, &text));
        }
        let total = resp.content_length();
        let mut written: u64 = 0;
        loop {
            if let Some(pause) = pause {
                pause.checkpoint().await;
            }
            if cancel.is_some_and(|c| c.load(Ordering::Acquire)) {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "download cancelled",
                ));
            }
            let next = resp.chunk().await.map_err(map_reqwest)?;
            let Some(chunk) = next else {
                break;
            };
            if cancel.is_some_and(|c| c.load(Ordering::Acquire)) {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "download cancelled",
                ));
            }
            if written + chunk.len() as u64 > max_bytes as u64 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "object exceeds size cap",
                ));
            }
            sink.write_all(&chunk).await.map_err(io::Error::other)?;
            written += chunk.len() as u64;
            on_progress(written, total);
        }
        on_progress(written, total);
        Ok(written)
    }

    /// PUT with the overwrite policy enforced at the HTTP layer (no
    /// existence preflight — racing TOCTOU is unsafe). For `Forbid` we send
    /// `If-None-Match: *` so the server rejects an existing resource with 412.
    pub(crate) async fn put_with_policy(
        &self,
        path: &str,
        data: &[u8],
        policy: WebDavOverwritePolicy,
    ) -> io::Result<()> {
        let url = self.resolve_url(path)?;
        let mut builder = self.client.put(&url).body(data.to_vec());
        if matches!(policy, WebDavOverwritePolicy::Forbid) {
            builder = builder.header("If-None-Match", "*");
        }
        let req = self.auth_req(builder)?;
        let resp = req.send().await.map_err(map_reqwest)?;
        let status = resp.status();
        if status == reqwest::StatusCode::CREATED
            || status == reqwest::StatusCode::NO_CONTENT
            || status == reqwest::StatusCode::OK
        {
            return Ok(());
        }
        if status == reqwest::StatusCode::PRECONDITION_FAILED {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "refusing to overwrite existing resource (policy Forbid)",
            ));
        }
        let text = read_fixed_text(resp).await;
        Err(self.status_error("PUT", status, &text))
    }

    /// Plain PUT that allows overwriting (used by internal callers that have
    /// already resolved the policy elsewhere). Thin wrapper over
    /// `put_with_policy(Allow)`.
    pub(crate) async fn put(&self, path: &str, data: &[u8]) -> io::Result<()> {
        self.put_with_policy(path, data, WebDavOverwritePolicy::Allow)
            .await
    }

    async fn delete_url(&self, url: String) -> io::Result<()> {
        let req = self.auth_req(self.client.delete(&url))?;
        let resp = req.send().await.map_err(map_reqwest)?;
        let status = resp.status();
        if status == reqwest::StatusCode::NO_CONTENT || status == reqwest::StatusCode::OK {
            return Ok(());
        }
        let text = read_fixed_text(resp).await;
        Err(self.status_error("DELETE", status, &text))
    }

    // ponytail: collections must be deleted with a trailing-slash URL; Apache
    // 301-redirects a collection DELETE without it. Files must NOT carry the
    // slash. Two thin methods over one DELETE body — no shared ambiguity.
    async fn mkcol(&self, path: &str) -> io::Result<()> {
        // Apache redirects MKCOL on a collection without a trailing slash to the
        // slashed form; send it directly (matches rclone behavior).
        let mut url = self.join_url(path);
        if !url.ends_with('/') {
            url.push('/');
        }
        let req = self.auth_req(
            self.client
                .request(reqwest::Method::from_bytes(b"MKCOL").unwrap(), &url),
        )?;
        let resp = req.send().await.map_err(map_reqwest)?;
        let status = resp.status();
        if status == reqwest::StatusCode::CREATED || status == reqwest::StatusCode::OK {
            return Ok(());
        }
        let text = read_fixed_text(resp).await;
        Err(self.status_error("MKCOL", status, &text))
    }

    #[allow(dead_code)] // wired into the async transfer executor in B
    pub(crate) async fn copy_or_move(
        &self,
        method: reqwest::Method,
        src: &str,
        dst: &str,
        overwrite: bool,
    ) -> io::Result<()> {
        let src_url = self.resolve_url(src)?;
        let dst_url = self.destination_url(dst);
        // #242: RFC 4918 §9.9.2 — MOVE acts with implicit Depth: infinity and
        // sabre/DAV servers (ownCloud 11, Nextcloud) reject `Depth: 0` on
        // MOVE with 400. COPY keeps the explicit `Depth: 0`; MOVE omits the
        // header entirely (never sends `infinity`).
        let depth_header = if method == reqwest::Method::from_bytes(b"COPY").unwrap() {
            Some(("Depth", "0"))
        } else {
            None
        };
        let base = self
            .client
            .request(method, &src_url)
            .header("Destination", dst_url)
            .header("Overwrite", if overwrite { "T" } else { "F" });
        let req = self.auth_req(match depth_header {
            Some((name, value)) => base.header(name, value),
            None => base,
        })?;
        let resp = req.send().await.map_err(map_reqwest)?;
        let status = resp.status();
        if status == reqwest::StatusCode::CREATED
            || status == reqwest::StatusCode::NO_CONTENT
            || status == reqwest::StatusCode::OK
        {
            return Ok(());
        }
        let text = read_fixed_text(resp).await;
        Err(self.status_error("COPY/MOVE", status, &text))
    }
}

#[async_trait::async_trait]
impl VfsProvider for WebDavProvider {
    fn list(&self, path: &str) -> io::Result<Vec<Entry>> {
        let handle = tokio::runtime::Handle::try_current()
            .map_err(|_| io::Error::other("WebDAV list requires a Tokio runtime"))?;
        let entries = handle.block_on(self.propfind(path, "1"))?;
        let self_href = self.join_url(path);
        Ok(entries
            .into_iter()
            .filter(|e| e.raw_href != self_href)
            .map(|e| Entry {
                name: e.display_name.unwrap_or_else(|| href_leaf(&e.raw_href)),
                kind: if e.is_collection {
                    EntryKind::Directory
                } else {
                    EntryKind::File
                },
                size: e.content_length,
                modified_unix_ms: e.modified_unix_ms,
            })
            .collect())
    }

    async fn list_async(&self, path: &str) -> io::Result<Vec<Entry>> {
        // Apache (and most servers) redirect PROPFIND on a collection without a
        // trailing slash to the slashed form; send it directly (rclone behavior).
        let mut url_path = path.to_string();
        if !url_path.ends_with('/') {
            url_path.push('/');
        }
        let entries = self.propfind(&url_path, "1").await?;
        let self_href = self.join_url(&url_path);
        let self_norm = self_href.trim_end_matches('/');
        let logical = path.trim_start_matches('/').trim_end_matches('/');
        Ok(entries
            .into_iter()
            .filter(|e| {
                let h = href_path_only(&e.raw_href)
                    .trim_end_matches('/')
                    .trim_start_matches('/');
                h != self_norm.trim_start_matches('/') && h != logical
            })
            .map(|e| Entry {
                name: e.display_name.unwrap_or_else(|| href_leaf(&e.raw_href)),
                kind: if e.is_collection {
                    EntryKind::Directory
                } else {
                    EntryKind::File
                },
                size: e.content_length,
                modified_unix_ms: e.modified_unix_ms,
            })
            .collect())
    }

    fn read_head(&self, path: &str, lines: usize) -> io::Result<Vec<String>> {
        let handle = tokio::runtime::Handle::try_current()
            .map_err(|_| io::Error::other("WebDAV read requires a Tokio runtime"))?;
        let read = handle.block_on(self.get_bounded(path, 64 * 1024))?;
        let text = String::from_utf8_lossy(&read.bytes);
        Ok(text.lines().take(lines).map(|s| s.to_string()).collect())
    }

    async fn read_prefix_bytes(&self, path: &str, max_bytes: usize) -> io::Result<BoundedRead> {
        self.get_bounded(path, max_bytes).await
    }

    async fn read_listed_prefix_bytes(
        &self,
        _location: &Location,
        listed: &ListedEntry,
        max_bytes: usize,
    ) -> io::Result<BoundedRead> {
        match &listed.identity {
            EntryIdentity::WebDavObject(identity) => {
                // Verify the target matches the provider's bound target, then GET
                // the exact raw href — never reconstruct from parent + name.
                if identity.target != self.target.id {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "listed entry belongs to a different WebDAV target",
                    ));
                }
                self.get_bounded(&identity.href, max_bytes).await
            }
            _ => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "WebDAV read requires a WebDavObject identity (use list_page)",
            )),
        }
    }

    /// Provider-side paginated listing with authoritative WebDAV identity.
    ///
    /// Each entry carries a `WebDavObjectRef`/`WebDavCollectionRef` holding the
    /// exact raw href, so downstream F5/F3 never reconstruct an address from
    /// `entry.name` (which is presentation-only and may differ from the href).
    async fn list_page(
        &self,
        location: &Location,
        continuation: Option<&ProviderContinuation>,
    ) -> io::Result<ProviderListingPage> {
        if continuation.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "WebDAV provider does not support listing continuation",
            ));
        }
        let path = Location::legacy_listing_path(location)?;
        let mut url_path = path.to_string();
        if !url_path.ends_with('/') {
            url_path.push('/');
        }
        let entries = self.propfind(&url_path, "1").await?;
        let self_href = self.join_url(&url_path);
        let self_norm = self_href.trim_end_matches('/');
        let logical = path.trim_start_matches('/').trim_end_matches('/');
        let listed: Vec<ListedEntry> = entries
            .into_iter()
            .filter(|e| {
                let h = href_path_only(&e.raw_href)
                    .trim_end_matches('/')
                    .trim_start_matches('/');
                h != self_norm.trim_start_matches('/') && h != logical
            })
            .map(|e| {
                let is_collection = e.is_collection;
                let name = e
                    .display_name
                    .clone()
                    .unwrap_or_else(|| href_leaf(&e.raw_href));
                let identity = if is_collection {
                    EntryIdentity::WebDavCollection(WebDavCollectionRef {
                        target: self.target.id.clone(),
                        href: e.raw_href.clone(),
                    })
                } else {
                    EntryIdentity::WebDavObject(WebDavObjectRef {
                        target: self.target.id.clone(),
                        href: e.raw_href.clone(),
                    })
                };
                ListedEntry {
                    entry: Entry {
                        name,
                        kind: if is_collection {
                            EntryKind::Directory
                        } else {
                            EntryKind::File
                        },
                        size: e.content_length,
                        modified_unix_ms: e.modified_unix_ms,
                    },
                    identity,
                }
            })
            .collect();
        Ok(ProviderListingPage {
            entries: listed,
            continuation: None,
        })
    }

    async fn read_all_capped(&self, path: &str, max_bytes: usize) -> io::Result<BoundedRead> {
        self.get_bounded(path, max_bytes).await
    }

    fn copy_files(&self, _src: &str, _dst: &str, _names: &[String]) -> io::Result<usize> {
        // ponytail/Blocker E: WebDAV copy/move is served by the async transfer
        // executor (WebDavTransferSpec), not the sync Trait path. The sync
        // VfsProvider file-ops would need a nested Tokio runtime here, which
        // panics on a current-thread runtime. Fail closed instead of building
        // one. Dataset-copy via F5 routes through `execute_webdav_transfer`.
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "WebDAV server-side copy is only available via the async transfer executor",
        ))
    }

    fn move_files(&self, _src: &str, _dst: &str, _names: &[String]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "WebDAV move is only available via the async transfer executor",
        ))
    }

    fn delete_files(&self, _dir: &str, _names: &[String]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "WebDAV delete is only available via the async transfer executor",
        ))
    }

    async fn mkdir(&self, path: &str) -> io::Result<()> {
        self.mkcol(path).await
    }

    async fn remove_file(&self, path: &str) -> io::Result<()> {
        // Files: DELETE the exact URL, no trailing slash.
        self.delete_url(self.join_url(path)).await
    }

    async fn remove_dir(&self, path: &str) -> io::Result<()> {
        // Collections: DELETE requires a trailing-slash URL (Apache 301
        // otherwise); canonicalize here so callers need not know.
        let mut url = self.join_url(path);
        if !url.ends_with('/') {
            url.push('/');
        }
        self.delete_url(url).await
    }

    async fn metadata(&self, path: &str) -> io::Result<FileMetadata> {
        let entries = self.propfind(path, "0").await?;
        let self_href = self.join_url(path);
        let e = entries
            .into_iter()
            .find(|e| e.raw_href == self_href)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("metadata: {path}")))?;
        Ok(FileMetadata {
            len: e.content_length.unwrap_or(0),
            is_regular: !e.is_collection,
            unix_mode: None,
            unix_uid: None,
            unix_gid: None,
        })
    }

    async fn write_file_bytes_if_unchanged(
        &self,
        path: &str,
        data: &[u8],
        _revision: &RemoteEditRevision,
        _cancellation: &crate::vfs::CancellationFlag,
        _progress: Option<RemoteEditProgressFn>,
    ) -> io::Result<()> {
        // MVP: PUT is sent exactly once. WebDAV has no server-agnostic
        // compare-and-swap, so revision is advisory; a transport error after
        // the body is returned as an error WITHOUT blind retry/replay.
        self.put(path, data).await
    }
}

impl std::fmt::Debug for WebDavProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebDavProvider")
            .field("target", &self.target)
            .finish()
    }
}

// ---- helpers ----

/// Extract the path portion of an href, stripping any `scheme://host[:port]`
/// prefix so logical-path comparisons are host-agnostic.
fn href_path_only(href: &str) -> &str {
    if let Some(idx) = href.find("://") {
        // find end of authority (next '/' after the host)
        if let Some(slash) = href[idx + 3..].find('/') {
            return &href[idx + 3 + slash..];
        }
        return "";
    }
    href
}

/// Last path segment of a raw href, percent-decoded for presentation only.
fn href_leaf(raw_href: &str) -> String {
    let path = raw_href.split('?').next().unwrap_or(raw_href);
    let leaf = path
        .trim_end_matches('/')
        .rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or(path);
    percent_decode(leaf)
}

fn summarize(body: &str) -> String {
    body.chars()
        .take(200)
        .collect::<String>()
        .replace('\n', " ")
        .trim()
        .to_string()
}

fn map_reqwest(e: reqwest::Error) -> io::Error {
    if e.is_timeout() {
        io::Error::new(io::ErrorKind::TimedOut, format!("webdav request: {e}"))
    } else if e.is_connect() {
        io::Error::new(
            io::ErrorKind::ConnectionRefused,
            format!("webdav connect: {e}"),
        )
    } else {
        io::Error::other(format!("webdav request: {e}"))
    }
}

async fn read_fixed_text(resp: reqwest::Response) -> String {
    match read_body_bounded(resp, 4096).await {
        Ok(b) => String::from_utf8_lossy(&b.bytes)
            .chars()
            .take(4096)
            .collect(),
        Err(_) => String::new(),
    }
}

/// Stream a response body, stopping at `max` bytes (+1 probe). Never reads past
/// the cap: once `max+1` bytes are observed we truncate to `max` and drop the
/// response, so the client never allocates or consumes the unbounded body.
async fn read_body_bounded(mut resp: reqwest::Response, max: usize) -> io::Result<BoundedBody> {
    let mut out: Vec<u8> = Vec::with_capacity(max.min(64 * 1024));
    loop {
        let chunk = match resp.chunk().await {
            Ok(Some(c)) => c,
            Ok(None) => break,
            Err(e) => return Err(map_reqwest(e)),
        };
        let remaining_probe = max.saturating_add(1).saturating_sub(out.len());
        if remaining_probe == 0 {
            return Ok(BoundedBody {
                bytes: out[..max].to_vec(),
                truncated: true,
            });
        }
        let take = chunk.len().min(remaining_probe);
        out.extend_from_slice(&chunk[..take]);
        if out.len() > max {
            out.truncate(max);
            return Ok(BoundedBody {
                bytes: out,
                truncated: true,
            });
        }
    }
    Ok(BoundedBody {
        bytes: out,
        truncated: false,
    })
}

struct BoundedBody {
    bytes: Vec<u8>,
    truncated: bool,
}

/// Percent-encode one path segment for the wire. Encodes everything except a
/// small unreserved set, preserving `%xx` already present in the logical path
/// input (we operate on decoded names, so we re-encode fully).
/// Percent-encode a single path segment (RFC 3986). Used for URL assembly.
/// Ponytail: no normalization; only encodes bytes not in the unreserved set.
pub fn encode_segment(seg: &str) -> String {
    let mut out = String::with_capacity(seg.len());
    for b in seg.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => {
                out.push('%');
                out.push(hex_digit(b >> 4));
                out.push(hex_digit(b & 0x0f));
            }
        }
    }
    out
}

fn hex_digit(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        _ => (b'A' + (n - 10)) as char,
    }
}

/// Percent-decode a segment for display only; leaves the input unchanged on
/// invalid sequences.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() && is_hex(bytes[i + 1]) && is_hex(bytes[i + 2]) {
            let hi = hex_val(bytes[i + 1]);
            let lo = hex_val(bytes[i + 2]);
            out.push(hi * 16 + lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn is_hex(b: u8) -> bool {
    b.is_ascii_hexdigit()
}

fn hex_val(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => 0,
    }
}

// ---- multistatus parser (quick-xml NsReader, DAV-namespace + propstat aware) ----

const MAX_PROPERTIES_PER_RESPONSE: usize = 256;
const MAX_HREF_BYTES: usize = 8192;

/// Parse a WebDAV multistatus PROPFIND response into entries.
///
/// Resolved-namespace parser: only elements whose resolved namespace URI equals
/// the DAV namespace (`DAV:`) drive protocol state. Non-DAV same-local-name
/// elements are ignored. Multiple `propstat` blocks are supported; only 2xx
/// propstat properties contribute, and a 404/403 propstat never overwrites a
/// successful value or marks a collection. Hard caps (response count, properties
/// per response, accumulated text, href length) return `InvalidData` rather than
/// silently truncating.
pub(crate) fn parse_multistatus(bytes: &[u8]) -> io::Result<Vec<PropFindEntry>> {
    use quick_xml::NsReader;
    use quick_xml::events::Event;

    let mut reader = NsReader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();

    let mut entries: Vec<PropFindEntry> = Vec::new();

    // Per-<response> accumulation.
    let mut in_response = false;
    let mut resp_props: std::collections::BTreeMap<PropKey, PropValue> =
        std::collections::BTreeMap::new();
    let mut resp_prop_count: usize = 0;

    // Per-<propstat> accumulation.
    let mut in_propstat = false;
    let mut ps_status_2xx = false;
    let mut ps_have_status = false;
    let mut ps_props: std::collections::BTreeMap<PropKey, PropValue> =
        std::collections::BTreeMap::new();

    // Per-property text accumulation.
    let mut text_for: Option<PropKey> = None;
    let mut text_accum: String = String::new();

    loop {
        // quick_xml NsReader resolves the namespace prefix to a stable id; we
        // map it back to the URI only to compare against the DAV namespace.
        let (ns, event) = reader.read_resolved_event_into(&mut buf).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("WebDAV XML read error: {e}"),
            )
        })?;
        match event {
            Event::Start(e) | Event::Empty(e) => {
                let name = e.local_name();
                let local = std::str::from_utf8(name.as_ref()).unwrap_or("");
                let is_dav = match ns {
                    quick_xml::name::ResolveResult::Bound(n) => n.0 == b"DAV:",
                    // No explicit namespace => NOT DAV. Namespace-less XML must
                    // not be mistaken for DAV protocol elements.
                    quick_xml::name::ResolveResult::Unbound => false,
                    // Unknown prefix or error: do not treat as DAV.
                    quick_xml::name::ResolveResult::Unknown(_) => false,
                };
                if !is_dav {
                    // Non-DAV element with identical local name: skip it and its
                    // subtree so it cannot perturb DAV state.
                    skip_element(&mut reader, &mut buf, &mut text_for, &mut text_accum);
                    continue;
                }
                if local == "response" {
                    if entries.len() >= MAX_RESPONSES {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "PROPFIND response count exceeds cap",
                        ));
                    }
                    in_response = true;
                    resp_props.clear();
                    resp_prop_count = 0;
                } else if local == "propstat" {
                    in_propstat = true;
                    ps_status_2xx = false;
                    ps_have_status = false;
                    ps_props.clear();
                } else if local == "prop" || local == "resourcetype" {
                    // containers; children handled below
                } else if local == "status" {
                    text_for = Some(PropKey::Status);
                    text_accum.clear();
                } else if local == "href" {
                    text_for = Some(PropKey::Href);
                    text_accum.clear();
                } else if local == "displayname" {
                    text_for = Some(PropKey::DisplayName);
                    text_accum.clear();
                } else if local == "getcontentlength" {
                    text_for = Some(PropKey::ContentLength);
                    text_accum.clear();
                } else if local == "getlastmodified" {
                    text_for = Some(PropKey::LastModified);
                    text_accum.clear();
                } else if local == "collection" {
                    if in_propstat {
                        ps_props.insert(PropKey::Collection, PropValue::Collection);
                    } else if in_response {
                        resp_props.insert(PropKey::Collection, PropValue::Collection);
                    }
                } else {
                    // Any other DAV leaf element is a property; capture its text so
                    // it is committed and counts toward the per-response cap.
                    text_for = Some(PropKey::Other(local.to_string()));
                    text_accum.clear();
                }
            }
            Event::End(e) => {
                let name = e.local_name();
                let local = std::str::from_utf8(name.as_ref()).unwrap_or("");
                match local {
                    "response" => {
                        if in_response {
                            let href = resp_props
                                .get(&PropKey::Href)
                                .and_then(|v| match v {
                                    PropValue::Text(t) => Some(t.clone()),
                                    _ => None,
                                })
                                .filter(|h| !h.is_empty())
                                .ok_or_else(|| {
                                    io::Error::new(
                                        io::ErrorKind::InvalidData,
                                        "PROPFIND response missing href",
                                    )
                                })?;
                            if href.len() > MAX_HREF_BYTES {
                                return Err(io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "PROPFIND href exceeds cap",
                                ));
                            }
                            let is_collection = resp_props
                                .get(&PropKey::Collection)
                                .map(|v| matches!(v, PropValue::Collection))
                                .unwrap_or(false);
                            let content_length =
                                resp_props
                                    .get(&PropKey::ContentLength)
                                    .and_then(|v| match v {
                                        PropValue::Text(t) => t.trim().parse::<u64>().ok(),
                                        _ => None,
                                    });
                            let modified_unix_ms =
                                resp_props
                                    .get(&PropKey::LastModified)
                                    .and_then(|v| match v {
                                        PropValue::Text(t) => parse_rfc2822_ms(t),
                                        _ => None,
                                    });
                            let display_name =
                                resp_props.get(&PropKey::DisplayName).and_then(|v| match v {
                                    PropValue::Text(t) => Some(percent_decode(t)),
                                    _ => None,
                                });
                            entries.push(PropFindEntry {
                                raw_href: href,
                                display_name,
                                is_collection,
                                content_length,
                                modified_unix_ms,
                            });
                        }
                        in_response = false;
                    }
                    "propstat" => {
                        if in_propstat && ps_status_2xx {
                            // Merge successful props into the response (first 2xx wins).
                            for (k, v) in ps_props.iter() {
                                if matches!(k, PropKey::Status) {
                                    continue;
                                }
                                resp_props.entry(k.clone()).or_insert_with(|| v.clone());
                            }
                        }
                        in_propstat = false;
                        ps_props.clear();
                        ps_have_status = false;
                        ps_status_2xx = false;
                    }
                    _ => {
                        // Any element that accumulated text (href, displayname,
                        // getcontentlength, getlastmodified, status, or an unknown
                        // property) commits its value through the same path.
                        if let Some(key) = text_for.take() {
                            let ok = commit_prop_value(
                                key,
                                &text_accum,
                                if in_propstat {
                                    &mut ps_props
                                } else {
                                    &mut resp_props
                                },
                                &mut ps_status_2xx,
                                &mut ps_have_status,
                                &mut resp_prop_count,
                            );
                            if !ok {
                                return Err(io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "PROPFIND properties exceed cap",
                                ));
                            }
                        }
                        text_accum.clear();
                    }
                }
            }
            Event::Text(e) => {
                if text_for.is_some() {
                    let t = e.unescape().unwrap_or_default();
                    // Checked arithmetic: when current == MAX and more arrives,
                    // reject rather than silently ignore the overflow.
                    let next = text_accum.len().checked_add(t.len()).ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            "PROPFIND accumulated text exceeds cap",
                        )
                    })?;
                    if next > MAX_ACCUM_TEXT {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "PROPFIND accumulated text exceeds cap",
                        ));
                    }
                    text_accum.push_str(&t);
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(entries)
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum PropKey {
    Href,
    DisplayName,
    ContentLength,
    LastModified,
    Collection,
    Status,
    Other(String),
}

#[derive(Clone)]
enum PropValue {
    Text(String),
    Collection,
}

/// Skip a non-DAV element (and its descendants) so their text is not captured.
fn skip_element(
    reader: &mut quick_xml::NsReader<&[u8]>,
    buf: &mut Vec<u8>,
    text_for: &mut Option<PropKey>,
    text_accum: &mut String,
) {
    *text_for = None;
    text_accum.clear();
    let mut depth = 1usize;
    while let Ok((_id, ev)) = reader.read_resolved_event_into(buf) {
        match ev {
            Event::Start(_) | Event::Empty(_) => depth += 1,
            Event::End(_) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
}

/// Fold a captured text value into the right property slot, enforcing caps.
/// Returns false if the value was rejected (e.g. property-count cap exceeded).
fn commit_prop_value(
    key: PropKey,
    text: &str,
    props: &mut std::collections::BTreeMap<PropKey, PropValue>,
    ps_status_2xx: &mut bool,
    ps_have_status: &mut bool,
    resp_prop_count: &mut usize,
) -> bool {
    match key {
        PropKey::Status => {
            *ps_have_status = true;
            *ps_status_2xx = text.trim().starts_with("HTTP/1.1 2");
        }
        PropKey::Collection => {
            if *ps_status_2xx || !*ps_have_status {
                props.insert(PropKey::Collection, PropValue::Collection);
            }
        }
        other => {
            let value = text.trim().to_string();
            if value.is_empty() {
                return true;
            }
            if other == PropKey::ContentLength && value.parse::<u64>().is_err() {
                return true;
            }
            if !props.contains_key(&other) {
                *resp_prop_count += 1;
                if *resp_prop_count > MAX_PROPERTIES_PER_RESPONSE {
                    // Hard cap: reject rather than silently drop.
                    return false;
                }
            }
            props.insert(other, PropValue::Text(value));
        }
    }
    true
}

/// Best-effort RFC2822 ("Mon, 02 Jan 2006 15:04:05 GMT") → unix millis.
/// Falls back to None on unrecognized formats (no external date crate).
pub(crate) fn parse_rfc2822_ms(s: &str) -> Option<u64> {
    // Common layout: "Day, DD Mon YYYY HH:MM:SS GMT"
    let s = s.trim();
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() < 6 {
        return None;
    }
    let day: u64 = parts[1].parse().ok()?;
    let mon = MONTHS
        .iter()
        .position(|m| m.eq_ignore_ascii_case(parts[2]))? as u64
        + 1;
    let year: u64 = parts[3].parse().ok()?;
    let (hh, mm, ss) = parse_hms(parts[4])?;
    let _tz = parts[5]; // assume GMT/UTC for the common case
    // Days before month (non-leap).
    let mut days = day - 1;
    for m in 1..mon {
        days += MONTH_DAYS[(m - 1) as usize] as u64;
    }
    let y = year - 1;
    days += y * 365 + (y / 4) - (y / 100) + (y / 400);
    let secs = days * 86_400 + hh * 3600 + mm * 60 + ss;
    Some(secs * 1000)
}

fn parse_hms(s: &str) -> Option<(u64, u64, u64)> {
    let mut it = s.split(':');
    let h: u64 = it.next()?.parse().ok()?;
    let m: u64 = it.next()?.parse().ok()?;
    let s: u64 = it.next()?.parse().ok()?;
    Some((h, m, s))
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const MONTH_DAYS: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

/// Capability-gated, physical-identity-aware F5 helper: Local → WebDAV PUT
/// (one-file MVP). The generic transfer path calls this via the provider.
pub async fn upload_local_to_webdav(
    provider: &WebDavProvider,
    local_path: &std::path::Path,
    remote_path: &str,
) -> io::Result<()> {
    let data = tokio::fs::read(local_path)
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::NotFound, format!("read local: {e}")))?;
    provider.put(remote_path, &data).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dav_provider(root: &str) -> WebDavProvider {
        WebDavProvider::new(
            WebDavTarget {
                id: "t".into(),
                name: "t".into(),
                url: root.to_string(),
                username: "u".into(),
                auth: "basic".into(),
            },
            "p".into(),
        )
        .unwrap()
    }

    // Security boundary: an authoritative href must stay inside the target
    // root and never allow traversal out of it.
    #[test]
    fn wire_url_for_href_rejects_escapes() {
        let p = dav_provider("http://example/dav/");

        // inside root -> resolved verbatim (no re-encoding)
        let ok = p.wire_url_for_href("/dav/file.txt").unwrap();
        assert_eq!(ok, "http://example/dav/file.txt");

        // sibling collection at root level -> REJECT
        assert!(p.wire_url_for_href("/dav2/file.txt").is_err());
        // different root -> REJECT
        assert!(p.wire_url_for_href("/other/file.txt").is_err());
        // encoded traversal -> REJECT (must not escape target root)
        assert!(p.wire_url_for_href("/dav/%2e%2e/secret").is_err());
        // literal traversal -> REJECT
        assert!(p.wire_url_for_href("/dav/../secret").is_err());
    }

    #[test]
    fn wire_url_for_href_preserves_percent_encoding() {
        let p = dav_provider("http://example/dav/");
        // server-supplied `%20` must stay `%20`, never become `%2520`
        let ok = p.wire_url_for_href("/dav/my%20file.txt").unwrap();
        assert_eq!(ok, "http://example/dav/my%20file.txt");
    }

    #[test]
    fn wire_url_for_href_rejects_cross_origin() {
        let p = dav_provider("http://example/dav/");
        assert!(p.wire_url_for_href("http://evil/dav/file.txt").is_err());
    }

    #[test]
    fn wire_url_for_href_preserves_query_same_origin_absolute() {
        let p = dav_provider("http://example/dav/");
        // authoritative absolute href keeps raw path + query verbatim.
        let ok = p
            .wire_url_for_href("http://example/dav/a%20b?version=7")
            .unwrap();
        assert_eq!(ok, "http://example/dav/a%20b?version=7");
    }

    #[test]
    fn wire_url_for_href_preserves_query_path_absolute() {
        let p = dav_provider("http://example/dav/");
        let ok = p.wire_url_for_href("/dav/a%20b?version=7").unwrap();
        assert_eq!(ok, "http://example/dav/a%20b?version=7");
    }

    #[test]
    fn wire_url_for_href_rejects_fragment() {
        let p = dav_provider("http://example/dav/");
        // same-origin absolute with fragment -> rejected.
        assert!(p.wire_url_for_href("http://example/dav/x#frag").is_err());
        // path-absolute with fragment -> rejected.
        assert!(p.wire_url_for_href("/dav/x#frag").is_err());
    }

    #[test]
    fn parse_multistatus_rejects_namespace_less_xml() {
        // Namespace-less XML is NOT DAV; must not drive protocol state.
        let body = br#"<?xml version="1.0" encoding="utf-8"?>
<multistatus xmlns="not-dav:">
  <response>
    <href>/dav/x</href>
    <propstat><prop><displayname>x</displayname></prop></propstat>
  </response>
</multistatus>"#;
        let entries = crate::vfs::webdav::parse_multistatus(body).unwrap();
        assert!(
            entries.is_empty(),
            "namespace-less XML must not parse as DAV"
        );
    }

    #[test]
    fn parse_multistatus_accepts_default_dav_namespace() {
        let body = br#"<?xml version="1.0" encoding="utf-8"?>
<multistatus xmlns="DAV:">
  <response>
    <href>/dav/x</href>
    <propstat><prop><displayname>x</displayname></prop></propstat>
  </response>
</multistatus>"#;
        let entries = crate::vfs::webdav::parse_multistatus(body).unwrap();
        assert_eq!(entries.len(), 1, "default DAV: namespace parsed");
    }

    #[test]
    fn parse_multistatus_rejects_max_accum_text_plus_one() {
        // Exactly MAX_ACCUM_TEXT bytes is allowed; one more must be rejected.
        let big = "a".repeat(crate::vfs::webdav::MAX_ACCUM_TEXT);
        let ok = format!(
            "<?xml version=\"1.0\"?><multistatus xmlns=\"DAV:\"><response><href>/dav/x</href>\
             <propstat><prop><displayname>{big}</displayname></prop></propstat></response></multistatus>"
        );
        assert!(crate::vfs::webdav::parse_multistatus(ok.as_bytes()).is_ok());
        let over = format!(
            "<?xml version=\"1.0\"?><multistatus xmlns=\"DAV:\"><response><href>/dav/x</href>\
             <propstat><prop><displayname>{}{}</displayname></prop></propstat></response></multistatus>",
            big, "b"
        );
        assert!(
            crate::vfs::webdav::parse_multistatus(over.as_bytes()).is_err(),
            "MAX+1 bytes must be rejected (InvalidData), not silently ignored"
        );
    }
}
