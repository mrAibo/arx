//! S3/MinIO VfsProvider stub + AWS client factory (S3-16).
use crate::config::S3TargetConfig;
use crate::config::sanitize_diag;
use crate::vfs::{
    BoundedRead, Entry, EntryIdentity, EntryKind, ListedEntry, Location, ProviderContinuation,
    ProviderListingPage, VfsOps, VfsProvider,
};
use aws_config::BehaviorVersion;
use aws_config::Region;
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{Builder, retry::RetryConfig};
use aws_sdk_s3::error::ProvideErrorMetadata;
use aws_sdk_s3::operation::get_object::GetObjectError;
use aws_sdk_s3::operation::list_buckets::ListBucketsOutput;
use aws_sdk_s3::operation::list_objects_v2::ListObjectsV2Output;
use std::borrow::Cow;
use std::io;
use tokio::io::AsyncReadExt;
use tokio::pin;

pub struct S3Fs;
/// Per-configured-target S3 provider.
///
/// Construction is cheap and offline: it stores the exact target config and a
/// lazy, per-target AWS client cell. The client is built only on first use via
/// `client()` (which routes through the S3-16 `client_for_target` boundary),
/// never at startup (DESIGN_S3 §10 lazy per-target model).
// ponytail: OnceCell per provider, not a global — different targets get
// independent clients; no eager AWS config load / network at construction.
pub struct S3Provider {
    pub(crate) target: S3TargetConfig,
    client: tokio::sync::OnceCell<aws_sdk_s3::Client>,
}

impl S3Provider {
    pub(crate) fn new(target: S3TargetConfig) -> Self {
        Self {
            target,
            client: tokio::sync::OnceCell::new(),
        }
    }

    /// Lazily construct (once) and return the AWS client for this target.
    ///
    /// Routes through the S3-16 `client_for_target` security/config boundary;
    /// never duplicates region/profile/endpoint/retry logic here.
    pub(crate) async fn client(&self) -> io::Result<&aws_sdk_s3::Client> {
        self.client
            .get_or_try_init(|| client_for_target(&self.target))
            .await
            .map_err(|e| io::Error::new(e.kind(), e.to_string()))
    }

    /// Pure decision boundary: which listing shape does this location request?
    ///
    /// Enforces the configured target binding and the exact target-root form
    /// BEFORE any AWS work, so a bucket-bound target can never trigger
    /// `s3:ListAllMyBuckets` and a malformed root prefix can never widen into
    /// an account-root ListBuckets. Performs no network; initializes no client.
    // ponytail: single guard point; all callers route through this, so a
    // sibling list path cannot forget the binding/prefix checks.
    fn classify_listing_location<'a>(
        &'a self,
        location: &'a Location,
    ) -> io::Result<S3ListingScope<'a>> {
        let (target, bucket, prefix) = match location {
            Location::S3 {
                target,
                bucket,
                prefix,
            } => (target, bucket, prefix),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "S3Provider::list_page requires Location::S3",
                ));
            }
        };

        // Exact target id (no trim/lowercase/normalization).
        if target != &self.target.id {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("S3 target mismatch: {}", sanitize_diag(target)),
            ));
        }

        // Configured bucket binding: a bucket-bound target must never reach
        // account-root ListBuckets, and must not escape to another bucket.
        match &self.target.bucket {
            Some(bound) => {
                let Some(requested) = bucket else {
                    // bucket == None on a bucket-bound target: fail closed,
                    // never silently widen to ListAllMyBuckets.
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!(
                            "S3 target {} is bucket-bound; account-root listing forbidden",
                            sanitize_diag(&self.target.id)
                        ),
                    ));
                };
                if requested != bound {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!(
                            "S3 bucket escape rejected: target bound to {}, requested {}",
                            sanitize_diag(bound),
                            sanitize_diag(requested)
                        ),
                    ));
                }
                // Bucket-bound location is valid but object listing (ListObjectsV2)
                // is S3-20.
                Ok(S3ListingScope::Bucket {
                    bucket: requested,
                    prefix,
                })
            }
            None => {
                // Account-style target: ListBuckets only for the exact root
                // shape (bucket == None AND prefix == ""). A non-empty prefix
                // on a root is contradictory (prefix lives inside a bucket) and
                // must fail closed, never normalized into "".
                if bucket.is_some() {
                    return Ok(S3ListingScope::Bucket {
                        bucket: bucket.as_deref().unwrap(),
                        prefix,
                    });
                }
                if !prefix.is_empty() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!(
                            "S3 target-root listing requires empty prefix, got {}",
                            sanitize_diag(prefix)
                        ),
                    ));
                }
                Ok(S3ListingScope::TargetRoot)
            }
        }
    }
    /// Fail-closed identity/location/target/bucket validation for a bounded
    /// GetObject preview. Runs BEFORE any AWS client construction, auth, or
    /// network. Never proves identity from `listed.entry.name` — only the exact
    /// `S3ObjectRef` is authoritative. Returns the live `S3ObjectRef` on success.
    // ponytail: single guard point for the read seam, mirroring
    // classify_listing_location; no sibling path can forget the checks.
    fn classify_read_identity<'a>(
        &'a self,
        location: &Location,
        listed: &'a ListedEntry,
    ) -> io::Result<&'a S3ObjectRef> {
        let EntryIdentity::S3Object(refr) = &listed.identity else {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "read_listed_prefix_bytes requires an S3Object identity (provider-native)",
            ));
        };
        let Location::S3 { target, bucket, .. } = location else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "S3 GetObject preview requires Location::S3",
            ));
        };
        // Exact target id (no normalization).
        if target != &self.target.id {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("S3 target mismatch: {}", sanitize_diag(target)),
            ));
        }
        if refr.target != self.target.id {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("S3 object target mismatch: {}", sanitize_diag(&refr.target)),
            ));
        }
        // Bucket-bound configured target: never escape to another bucket.
        if matches!(self.target.bucket.as_deref(), Some(bound) if bound != refr.bucket) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "S3 bucket escape rejected: target bound to {}, object in {}",
                    sanitize_diag(self.target.bucket.as_deref().unwrap_or("")),
                    sanitize_diag(&refr.bucket)
                ),
            ));
        }
        if bucket.as_deref() != Some(refr.bucket.as_str()) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "S3 location bucket mismatch: location {}, object {}",
                    sanitize_diag(bucket.as_deref().unwrap_or("")),
                    sanitize_diag(&refr.bucket)
                ),
            ));
        }
        Ok(refr)
    }
}

/// Listing scope decision produced by `classify_listing_location`.
// ponytail: no filesystem semantics; `Bucket` here is the S3 listing scope,
// distinct from filesystem directory traversal.
#[derive(Debug, Clone, Copy)]
enum S3ListingScope<'a> {
    TargetRoot,
    Bucket { bucket: &'a str, prefix: &'a str },
}

impl std::fmt::Debug for S3Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Provider")
            .field("target_id", &self.target.id)
            .field("client_initialized", &self.client.get().is_some())
            .finish()
    }
}

/// Provider-native S3 identity types.
///
/// These mirror ARX `Location::S3` semantics: `target`/`bucket`/`key`/`prefix`
/// are opaque provider strings stored verbatim. They are NOT filesystem paths;
/// `foo//bar`, `foo/../bar`, `foo/./bar`, `foo/` and Unicode values are preserved
/// byte-for-byte. No normalization, trimming, canonicalization, or `//`/`.`/trailing
/// slash rewriting happens here.
// ponytail: identity boundary only — no AWS client, no listing yet
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct S3BucketRef {
    pub target: String,
    pub bucket: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct S3ObjectRef {
    pub target: String,
    pub bucket: String,
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct S3PrefixRef {
    pub target: String,
    pub bucket: String,
    pub prefix: String,
}

/// Pure translation of an `S3TargetConfig` into the client-construction
/// parameters. No credentials, no id/name/bucket (those are not client
/// construction parameters). Accepted config strings are preserved verbatim.
// ponytail: separates pure translation from environment loading so unit tests
// need no AWS credentials / network
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct S3ClientSettings {
    pub region: Option<String>,
    pub profile: Option<String>,
    pub endpoint_url: Option<String>,
    pub force_path_style: bool,
}

impl S3ClientSettings {
    #[allow(dead_code)]
    fn from_target(target: &S3TargetConfig) -> Self {
        Self {
            region: target.region.clone(),
            profile: target.profile.clone(),
            endpoint_url: target.endpoint_url.clone(),
            force_path_style: target.force_path_style,
        }
    }
}

/// Reject custom endpoints that embed credentials (DESIGN_S3 §27).
/// Returns a redacted local configuration error — never echoes the URL,
/// password, or X-Amz values.
#[allow(dead_code)]
fn validate_endpoint(url_str: &str) -> io::Result<()> {
    let url = reqwest::Url::parse(url_str).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid S3 endpoint configuration: endpoint URL did not parse",
        )
    })?;

    if !url.username().is_empty() || url.password().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid S3 endpoint configuration: embedded credentials are not allowed",
        ));
    }

    for (key, _) in url.query_pairs() {
        if key.to_ascii_lowercase().starts_with("x-amz-") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid S3 endpoint configuration: embedded credentials are not allowed",
            ));
        }
    }

    Ok(())
}

/// Build the S3 service `Config` from shared SDK config + translated settings.
/// Retries are explicitly disabled (DESIGN_S3 S3-03 / S3-DESIGN-AF-03): ARX owns
/// later operation-class retry behavior.
///
/// Endpoint security lives at the S3 service-builder boundary: any endpoint
/// URL inherited from the shared SDK config (AWS_ENDPOINT_URL,
/// AWS_ENDPOINT_URL_S3, shared-profile endpoint_url) is cleared first, then
/// only the validated ARX `S3TargetConfig.endpoint_url` is applied. The
/// original `SdkConfig` is passed through untouched — all other SDK
/// configuration (credentials provider, region, HTTP client, identity cache,
/// time/sleep/timeout, behavior version) is preserved by `Builder::from`.
// ponytail: single deterministic helper so the retry-invariant is observable
// and testable without performing real AWS calls
#[allow(dead_code)]
pub(crate) fn build_s3_config(
    settings: &S3ClientSettings,
    sdk_config: &aws_config::SdkConfig,
) -> aws_sdk_s3::Config {
    let mut builder = Builder::from(sdk_config);

    // Discard endpoint URL inherited from AWS global/service configuration.
    // ARX target config is authoritative for custom S3 endpoints (DESIGN_S3 §27).
    builder.set_endpoint_url(None);

    if let Some(endpoint) = &settings.endpoint_url {
        builder.set_endpoint_url(Some(endpoint.clone()));
    }
    builder = builder.force_path_style(settings.force_path_style);
    builder = builder.retry_config(RetryConfig::disabled());
    builder.build()
}

/// Construct an `aws_sdk_s3::Client` for exactly one `S3TargetConfig`.
///
/// Uses the official AWS SDK credential/config chain (no manual chain, no
/// static credentials). The validated ARX target endpoint is the only custom
/// S3 endpoint; any endpoint inherited from the shared SDK config is cleared
/// at the S3 service-builder boundary (see `build_s3_config`). Creates NO
/// requests and performs NO listing.
#[allow(dead_code)]
pub(crate) async fn client_for_target(target: &S3TargetConfig) -> io::Result<Client> {
    if let Some(endpoint) = &target.endpoint_url {
        validate_endpoint(endpoint)?;
    }

    let settings = S3ClientSettings::from_target(target);

    let mut loader = aws_config::defaults(BehaviorVersion::latest());
    if let Some(region) = &settings.region {
        loader = loader.region(Region::new(region.clone()));
    }
    if let Some(profile) = &settings.profile {
        loader = loader.profile_name(profile.clone());
    }
    let sdk_config = loader.load().await;

    Ok(Client::from_conf(build_s3_config(&settings, &sdk_config)))
}

#[async_trait::async_trait]
impl VfsProvider for S3Provider {
    fn list(&self, _path: &str) -> io::Result<Vec<Entry>> {
        Err(io::Error::other("S3: not implemented"))
    }
    fn read_head(&self, _path: &str, _lines: usize) -> io::Result<Vec<String>> {
        Err(io::Error::other("S3: not implemented"))
    }
    fn copy_files(&self, _src: &str, _dst: &str, _names: &[String]) -> io::Result<usize> {
        Err(io::Error::other("S3: not implemented"))
    }
    fn move_files(&self, _src: &str, _dst: &str, _names: &[String]) -> io::Result<usize> {
        Err(io::Error::other("S3: not implemented"))
    }
    fn delete_files(&self, _dir: &str, _names: &[String]) -> io::Result<usize> {
        Err(io::Error::other("S3: not implemented"))
    }

    async fn list_page(
        &self,
        location: &Location,
        continuation: Option<&ProviderContinuation>,
    ) -> io::Result<ProviderListingPage> {
        // Pure offline classification: enforce target binding + exact root form
        // BEFORE any AWS work. Bucket-bound targets cannot reach ListBuckets;
        // malformed root prefixes fail closed (never normalized).
        let scope = self.classify_listing_location(location)?;

        match scope {
            S3ListingScope::TargetRoot => {
                // Validate the incoming continuation token offline, before the
                // AWS client is built. An empty token is a local protocol error;
                // ListBuckets has no independent has-more signal, so we never
                // invent one.
                let consumed = match continuation {
                    Some(c) if c.token.is_empty() => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "S3 ListBuckets pagination protocol error: empty continuation token",
                        ));
                    }
                    Some(c) => Some(c.token.as_str()),
                    None => None,
                };

                // S3-17 lazy per-target lifecycle: only this boundary builds the
                // client. Exactly one bounded ListBuckets .send() per page.
                let client = self.client().await?;
                let output = list_buckets_page(client, consumed).await?;
                let page = map_list_buckets_page(&self.target.id, &output)?;
                // Output continuation protocol: None => end-of-list; Some => next
                // page. Repeated/empty token is a ProtocolError (no loop).
                let continuation =
                    next_list_buckets_continuation(consumed, output.continuation_token())?;
                Ok(ProviderListingPage {
                    entries: page.entries,
                    continuation,
                })
            }
            S3ListingScope::Bucket { bucket, prefix } => {
                // Validate continuation offline before the lazy AWS client exists.
                // The token is provider-owned and remains byte-for-byte opaque.
                let consumed = match continuation {
                    Some(c) if c.token.is_empty() => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "S3 ListObjectsV2 pagination protocol error: empty continuation token",
                        ));
                    }
                    Some(c) => Some(c.token.as_str()),
                    None => None,
                };

                // S3-17 lazy per-target lifecycle: only this boundary builds the
                // client. Exactly one bounded ListObjectsV2 .send() per page.
                let client = self.client().await?;
                let wire_prefix = list_objects_wire_prefix(prefix);
                let output = list_objects_v2_page(client, bucket, &wire_prefix, consumed).await?;
                let page =
                    map_list_objects_v2_page(&self.target.id, bucket, &wire_prefix, &output)?;
                // Per-page continuation truth: IsTruncated/NextContinuationToken.
                let continuation = next_list_objects_v2_continuation(
                    consumed,
                    output.is_truncated(),
                    output.next_continuation_token(),
                )?;
                Ok(ProviderListingPage {
                    entries: page.entries,
                    continuation,
                })
            }
        }
    }

    /// Identity-aware bounded preview read for a listed S3 object.
    ///
    /// Overrides the default seam with a fail-closed, exact-identity GetObject
    /// using a byte `Range`. Never reconstructs a key from `entry.name`; the
    /// `S3ObjectRef` is the sole authority for bucket/key. No capability or
    /// availability change — this is purely a narrower, identity-bound read.
    async fn read_listed_prefix_bytes(
        &self,
        location: &Location,
        listed: &ListedEntry,
        max_bytes: usize,
    ) -> io::Result<BoundedRead> {
        // Fail-closed validation BEFORE any AWS client/auth/network work.
        let refr = self.classify_read_identity(location, listed)?;
        let params = build_get_object_params(refr, max_bytes)?;

        let client = self.client().await?;
        let out = client
            .get_object()
            .bucket(&params.bucket)
            .key(&params.key)
            .range(&params.range)
            .send()
            .await;

        let out = match out {
            Ok(out) => out,
            // Diagnostics are sanitized to a static label: no key, credentials,
            // signed query, or authorization header ever reaches the caller.
            // A zero-byte object cannot satisfy `bytes=0-N` (N>=0), so S3
            // returns 416 InvalidRange — the ONLY modeled condition we map to an
            // empty preview. Every other error (NoSuchKey, AccessDenied,
            // transport) stays a failure.
            Err(sdk_err) => return map_get_object_error(&sdk_err.into_service_error()),
        };

        // Bounded local read: never collect an unbounded stream. `Take` enforces
        // the cap even if the endpoint ignores the Range header.
        let reader = out.body.into_async_read();
        let (body, truncated) = read_bounded_body(reader, params.probe_len).await?;
        let bytes: Vec<u8> = body.into_iter().take(max_bytes).collect();
        Ok(s3_bounded_read(bytes, truncated))
    }
}

/// Bounded page size for every ListBuckets request (first and next pages).
// ponytail: stays well under the 10k quota ceiling; one page is the unit of work.
const LIST_BUCKETS_PAGE_SIZE: i32 = 1000;

/// One bounded ListBuckets request. Exactly one `.send()` per invocation.
/// `continuation` (verbatim token) is applied as `ContinuationToken` when
/// present; absent for the first page. No loop, no paginator helper.
async fn list_buckets_page(
    client: &Client,
    continuation: Option<&str>,
) -> io::Result<ListBucketsOutput> {
    let mut request = client.list_buckets().max_buckets(LIST_BUCKETS_PAGE_SIZE);
    if let Some(token) = continuation {
        request = request.continuation_token(token);
    }
    request
        .send()
        .await
        .map_err(|e| io::Error::other(format!("S3 ListBuckets failed: {}", e)))
}

/// Pure output-continuation protocol for ListBuckets. No network.
///
/// ListBuckets exposes no independent `IsTruncated`/`has-more` signal: the
/// presence/absence of `ContinuationToken` IS the end-of-list decision.
/// `None` => end-of-list. `Some` => next page. An empty/unusable returned
/// token, or one identical to the consumed token (non-advancing), is a
/// ProtocolError — never re-requested, so no infinite loop.
// ponytail: token values are never echoed into errors (ProviderContinuation is
// opaque/provider-owned); only a factual "did not advance" message is safe.
fn next_list_buckets_continuation(
    consumed: Option<&str>,
    returned: Option<&str>,
) -> io::Result<Option<ProviderContinuation>> {
    match returned {
        None => Ok(None),
        Some(next) => {
            if next.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "S3 ListBuckets pagination protocol error: empty returned token",
                ));
            }
            if Some(next) == consumed {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "S3 ListBuckets pagination protocol error: continuation token did not advance",
                ));
            }
            Ok(Some(ProviderContinuation {
                token: next.to_string(),
            }))
        }
    }
}

/// Pure AWS-response → provider-page mapping. No network, no SDK config dump.
/// Every usable bucket name becomes exactly one `ListedEntry` with an exact
/// `S3BucketRef` identity. Skips name-less records; never invents one.
/// Continuation is handled separately by `next_list_buckets_continuation`.
fn map_list_buckets_page(
    target_id: &str,
    output: &ListBucketsOutput,
) -> io::Result<ProviderListingPage> {
    let mut entries = Vec::new();
    for bucket in output.buckets() {
        // No name => malformed/unusable record; skip, never invent identity.
        let Some(name) = bucket.name() else {
            continue;
        };
        entries.push(ListedEntry {
            entry: Entry {
                name: name.to_string(),
                kind: EntryKind::Directory,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::S3Bucket(S3BucketRef {
                target: target_id.to_string(),
                bucket: name.to_string(),
            }),
        });
    }
    Ok(ProviderListingPage {
        entries,
        continuation: None,
    })
}

/// Exact, sanitized GetObject request parameters for one bounded preview.
/// `probe_len` = `max_bytes + 1`: the extra byte is the truncation probe.
#[derive(Debug)]
struct GetObjectParams {
    bucket: String,
    key: String,
    range: String,
    probe_len: usize,
}

/// Pure request-parameter construction for a bounded S3 GetObject preview.
///
/// Takes ONLY the exact `S3ObjectRef` — `entry.name` is never consulted, so a
/// wrong display name can never leak into the request. Fail-closed on a zero
/// preview cap (no unbounded GET) and on `usize` overflow of `max_bytes + 1`.
// ponytail: pure so the request shape is unit-testable without an AWS client.
fn build_get_object_params(refr: &S3ObjectRef, max_bytes: usize) -> io::Result<GetObjectParams> {
    if max_bytes == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "S3 GetObject preview cap of 0 bytes is not supported",
        ));
    }
    let probe_len = max_bytes.checked_add(1).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "S3 GetObject preview cap overflows usize",
        )
    })?;
    Ok(GetObjectParams {
        bucket: refr.bucket.clone(),
        key: refr.key.clone(),
        // S3 Range end is inclusive; `bytes=0-{max_bytes}` requests max_bytes+1 bytes.
        range: format!("bytes=0-{max_bytes}"),
        probe_len,
    })
}

/// The ONLY `BoundedRead` constructor the S3 provider uses.
///
/// S3 objects carry no POSIX metadata (mode/uid/gid), so these fields are
/// always `None`. Because every S3 `BoundedRead` has `None` unix fields,
/// `BoundedRead::into_revision()` can never succeed for S3 — a preview can
/// never become a `RemoteEditRevision`. Isolating construction here keeps the
/// "no POSIX fields for S3" invariant in exactly one auditable place and makes
/// it impossible for a sibling read path to forget it.
fn s3_bounded_read(bytes: Vec<u8>, truncated: bool) -> BoundedRead {
    BoundedRead {
        bytes,
        truncated,
        unix_mode: None,
        unix_uid: None,
        unix_gid: None,
    }
}

/// Read at most `probe_len` bytes from an `AsyncRead` and stop.
///
/// `Take` enforces the cap even if the endpoint ignores the Range header, so an
/// unbounded stream is never collected. Truncation is PROVABLE from the local
/// length (conservative, never claims complete when uncertain):
/// - `buf.len() < probe_len` → stream EOF reached before cap → object PROVES
///   ended at ≤ max_bytes → `truncated = false`.
/// - `buf.len() == probe_len` → cap reached → object ≥ max_bytes + 1 bytes
///   (could be exactly or much larger) → `truncated = true`.
///   This is the only truncation decision; no metadata parsing, no heuristics.
// ponytail: take() is the whole bound; no Content-Length parsing needed.
async fn read_bounded_body<R: tokio::io::AsyncRead>(
    reader: R,
    probe_len: usize,
) -> io::Result<(Vec<u8>, bool)> {
    let limited = reader.take(probe_len as u64);
    let mut buf = Vec::with_capacity(probe_len.clamp(1, 64 * 1024));
    pin!(limited);
    limited.read_to_end(&mut buf).await?;
    let truncated = buf.len() >= probe_len;
    Ok((buf, truncated))
}

/// Sanitized mapping of a GetObject service error to a preview result.
///
/// Only the exact `InvalidRange` (416, from a zero-byte object that cannot
/// satisfy `bytes=0-N`) maps to an empty, non-truncated preview. Every other
/// error — NoSuchKey, AccessDenied, InvalidObjectState, transport — becomes a
/// static, key-free failure. The diagnostic never contains the key, a signed
/// query, a credential, or an authorization header.
// ponytail: code() is the only signal inspected; the message is discarded.
fn map_get_object_error(svc: &GetObjectError) -> io::Result<BoundedRead> {
    match svc {
        err if err.code() == Some("InvalidRange") => Ok(s3_bounded_read(Vec::new(), false)),
        _ => Err(io::Error::other("S3 GetObject preview request failed")),
    }
}

/// Bounded page size for every ListObjectsV2 request.
// ponytail: one page is the unit of work; no eager enumeration.
const LIST_OBJECTS_PAGE_SIZE: i32 = 1000;

/// Construct the wire prefix for a ListObjectsV2 request.
///
/// Three distinct values exist (see `docs/DESIGN_S3.md`): the exact provider
/// `CommonPrefix`, the navigation `Location::S3.prefix`, and this wire prefix.
///
/// The wire prefix is protocol/navigation construction, NOT filesystem
/// normalization:
/// - nav `""` => wire `""` (bucket root)
/// - nav non-empty => append EXACTLY ONE `/` UNCONDITIONALLY, even when `nav`
///   already ends in `/` (that trailing slash is literal namespace structure,
///   not a protocol delimiter to skip).
///
/// This keeps the seam reversible: `nav(P)` removes one delimiter from the
/// exact provider prefix, and `wire(nav(P))` re-adds it, so
/// `wire(nav(P)) == P` for every valid repeated-delimiter prefix.
/// Never trim, collapse `//`, resolve `./`/`../`, or canonicalize.
fn list_objects_wire_prefix(nav_prefix: &str) -> Cow<'_, str> {
    if nav_prefix.is_empty() {
        Cow::Borrowed("")
    } else {
        Cow::Owned(format!("{nav_prefix}/"))
    }
}

/// Build one bounded ListObjectsV2 request for any page.
fn list_objects_v2_request(
    client: &Client,
    bucket: &str,
    wire_prefix: &str,
    continuation: Option<&str>,
) -> aws_sdk_s3::operation::list_objects_v2::builders::ListObjectsV2FluentBuilder {
    let mut request = client
        .list_objects_v2()
        .bucket(bucket)
        .prefix(wire_prefix)
        .delimiter("/")
        .max_keys(LIST_OBJECTS_PAGE_SIZE);
    if let Some(token) = continuation {
        request = request.continuation_token(token);
    }
    request
}

/// Send one bounded ListObjectsV2 request. No loop, no paginator helper.
/// Exactly one `.send()` per invocation.
async fn list_objects_v2_page(
    client: &Client,
    bucket: &str,
    wire_prefix: &str,
    continuation: Option<&str>,
) -> io::Result<ListObjectsV2Output> {
    list_objects_v2_request(client, bucket, wire_prefix, continuation)
        .send()
        .await
        // The SDK error may include the request URI, whose query can contain
        // the opaque continuation token. Keep diagnostics factual and redacted.
        .map_err(|_| io::Error::other("S3 ListObjectsV2 request failed"))
}

/// Pure per-page continuation protocol for ListObjectsV2.
/// ListObjectsV2 exposes IsTruncated + NextContinuationToken.
/// - IsTruncated == false => None
/// - IsTruncated == true AND usable NextContinuationToken => Some(ProviderContinuation)
/// - IsTruncated == true AND token missing/empty => InvalidData (ProtocolError)
/// - Returned token identical to consumed token => InvalidData (non-advancing)
/// - Missing IsTruncated => InvalidData
/// - IsTruncated == false BUT NextContinuationToken present => contradictory InvalidData
fn next_list_objects_v2_continuation(
    consumed: Option<&str>,
    is_truncated: Option<bool>,
    next_token: Option<&str>,
) -> io::Result<Option<ProviderContinuation>> {
    match is_truncated {
        Some(false) => match next_token {
            None => Ok(None),
            Some(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "S3 ListObjectsV2 pagination protocol error: IsTruncated=false with NextContinuationToken present",
            )),
        },
        Some(true) => match next_token {
            Some(token) if !token.is_empty() && Some(token) != consumed => {
                Ok(Some(ProviderContinuation {
                    token: token.to_string(),
                }))
            }
            Some(token) if Some(token) == consumed => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "S3 ListObjectsV2 pagination protocol error: continuation token did not advance",
            )),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "S3 ListObjectsV2 pagination protocol error: missing or empty NextContinuationToken on truncated response",
            )),
        },
        None => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "S3 ListObjectsV2 pagination protocol error: missing IsTruncated field",
        )),
    }
}

/// Pure AWS-response → provider-page mapping for every ListObjectsV2 page.
/// Includes folder-marker dedup by exact evidence only.
fn map_list_objects_v2_page(
    target_id: &str,
    bucket: &str,
    wire_prefix: &str,
    output: &ListObjectsV2Output,
) -> io::Result<ProviderListingPage> {
    let mut entries = Vec::new();

    // Collect exact CommonPrefix values for folder-marker dedup.
    let common_prefixes: Vec<String> = output
        .common_prefixes()
        .iter()
        .filter_map(|cp| cp.prefix())
        .map(|s| s.to_string())
        .collect();

    // Map Contents -> S3ObjectRef (objects).
    for obj in output.contents() {
        let Some(key) = obj.key() else {
            // Missing key => skip unusable record, never invent identity.
            continue;
        };

        // Reject objects outside the requested wire prefix.
        if !key.starts_with(wire_prefix) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "S3 ListObjectsV2 response contained object key outside requested prefix",
            ));
        }

        // FOLDER MARKER DEDUP — exact evidence only.
        // CASE A: current-folder marker (wire prefix != "" AND key == wire prefix AND size == 0).
        let is_current_folder_marker =
            !wire_prefix.is_empty() && key == wire_prefix && obj.size() == Some(0);
        // CASE B: child-folder marker duplicated by CommonPrefixes (size == 0 AND exact CommonPrefixes contains key).
        let is_child_marker_deduped =
            obj.size() == Some(0) && common_prefixes.contains(&key.to_string());

        if is_current_folder_marker || is_child_marker_deduped {
            // Suppress the marker; identity already represented as virtual folder.
            continue;
        }

        // NON-ZERO self-slash object: key == wire_prefix but size > 0.
        // It is real data; presentation would be empty, so fall back to exact key.
        let presentation_name = if !wire_prefix.is_empty() && key == wire_prefix {
            key.to_string()
        } else if let Some(stripped) = key.strip_prefix(wire_prefix) {
            // Strip exact wire prefix for presentation (not operational identity).
            stripped.to_string()
        } else {
            key.to_string()
        };

        // Size: only if present and non-negative.
        let size = obj
            .size()
            .and_then(|s| if s >= 0 { Some(s as u64) } else { None });

        // Last modified: convert DateTime to unix ms if valid.
        let modified_unix_ms = obj
            .last_modified()
            .and_then(|dt| dt.to_millis().ok())
            .and_then(|ms| if ms >= 0 { Some(ms as u64) } else { None });

        entries.push(ListedEntry {
            entry: Entry {
                name: presentation_name,
                kind: EntryKind::File,
                size,
                modified_unix_ms,
            },
            identity: EntryIdentity::S3Object(S3ObjectRef {
                target: target_id.to_string(),
                bucket: bucket.to_string(),
                key: key.to_string(),
            }),
        });
    }

    // Map CommonPrefixes -> S3PrefixRef (virtual folders).
    for cp in output.common_prefixes() {
        let Some(prefix) = cp.prefix() else {
            continue;
        };

        // A returned CommonPrefix with Delimiter="/" MUST end in that delimiter
        // (it groups keys sharing the prefix up to the delimiter). A missing
        // delimiter is a malformed/misconfigured provider response: reject it
        // rather than inventing the missing `/` or transforming the value.
        // This makes the future S3-24 nav conversion formally reversible
        // (provider exact prefix - one delimiter = nav prefix). The error does
        // not echo the prefix value.
        if !prefix.ends_with('/') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "S3 ListObjectsV2 response contained CommonPrefix without delimiter",
            ));
        }

        // Reject CommonPrefix outside requested wire prefix.
        if !prefix.starts_with(wire_prefix) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "S3 ListObjectsV2 response contained CommonPrefix outside requested prefix",
            ));
        }

        // Skip self-navigation (CommonPrefix == wire_prefix => no child).
        if prefix == wire_prefix {
            continue;
        }

        // Presentation: strip exact wire prefix + exactly one trailing "/".
        let presentation_name = if let Some(rest) = prefix.strip_prefix(wire_prefix) {
            if let Some(stripped) = rest.strip_suffix('/') {
                stripped.to_string()
            } else {
                rest.to_string()
            }
        } else {
            prefix.to_string()
        };

        entries.push(ListedEntry {
            entry: Entry {
                name: presentation_name,
                kind: EntryKind::Directory,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::S3Prefix(S3PrefixRef {
                target: target_id.to_string(),
                bucket: bucket.to_string(),
                prefix: prefix.to_string(),
            }),
        });
    }

    Ok(ProviderListingPage {
        entries,
        continuation: None, // handled by next_list_objects_v2_continuation
    })
}
impl VfsOps for S3Fs {
    fn list(&self) -> anyhow::Result<Vec<Entry>> {
        Err(anyhow::anyhow!("S3: not implemented"))
    }
    fn read_head(&self, _path: &str, _lines: usize) -> anyhow::Result<Vec<String>> {
        Err(anyhow::anyhow!("S3: not implemented"))
    }
    fn copy_files(&self, _from: &str, _to: &str, _names: &[String]) -> io::Result<usize> {
        Err(io::Error::other("S3: not implemented"))
    }
    fn move_files(&self, _from: &str, _to: &str, _names: &[String]) -> io::Result<usize> {
        Err(io::Error::other("S3: not implemented"))
    }
    fn delete_files(&self, _dir: &str, _names: &[String]) -> io::Result<usize> {
        Err(io::Error::other("S3: not implemented"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s3_object_key_preserved_exactly() {
        for key in [
            "foo//bar",
            "foo/../bar",
            "foo/./bar",
            "foo/",
            "каталог/файл.txt",
            "日本語/資料.txt",
            "emoji/🧙‍♂️.txt",
        ] {
            let r = S3ObjectRef {
                target: "aws".into(),
                bucket: "b".into(),
                key: key.into(),
            };
            assert_eq!(r.key, key, "object key must stay verbatim");
        }
    }

    #[test]
    fn s3_prefix_preserved_exactly() {
        for prefix in [
            "foo/",
            "foo//bar/",
            "foo/../bar/",
            "日本語/",
            "emoji/🧙‍♂️.txt",
        ] {
            let r = S3PrefixRef {
                target: "aws".into(),
                bucket: "b".into(),
                prefix: prefix.into(),
            };
            assert_eq!(r.prefix, prefix, "prefix must stay verbatim");
        }
    }

    #[test]
    fn s3_bucket_target_identity_preserved() {
        let r = S3BucketRef {
            target: " aws ".into(),
            bucket: "Company-Artifacts".into(),
        };
        assert_eq!(r.target, " aws ");
        assert_eq!(r.bucket, "Company-Artifacts");
    }

    #[test]
    fn s3_refs_are_comparable() {
        assert_eq!(
            S3ObjectRef {
                target: "a".into(),
                bucket: "b".into(),
                key: "k".into(),
            },
            S3ObjectRef {
                target: "a".into(),
                bucket: "b".into(),
                key: "k".into(),
            }
        );
    }

    // ── S3-16: AWS client factory tests ──

    fn mk_target(
        region: Option<&str>,
        profile: Option<&str>,
        endpoint_url: Option<&str>,
        force_path_style: bool,
    ) -> S3TargetConfig {
        S3TargetConfig {
            id: "t".into(),
            name: "test".into(),
            bucket: Some("b".into()),
            region: region.map(|s| s.to_string()),
            profile: profile.map(|s| s.to_string()),
            endpoint_url: endpoint_url.map(|s| s.to_string()),
            force_path_style,
        }
    }

    #[test]
    fn minimal_target_translation() {
        let target = mk_target(None, None, None, false);
        let settings = S3ClientSettings::from_target(&target);
        assert_eq!(settings.region, None);
        assert_eq!(settings.profile, None);
        assert_eq!(settings.endpoint_url, None);
        assert!(!settings.force_path_style);
    }

    #[test]
    fn explicit_region_profile_translation() {
        let target = mk_target(Some("eu-central-1"), Some("release"), None, false);
        let settings = S3ClientSettings::from_target(&target);
        assert_eq!(settings.region.as_deref(), Some("eu-central-1"));
        assert_eq!(settings.profile.as_deref(), Some("release"));
        assert_eq!(settings.endpoint_url, None);
        assert!(!settings.force_path_style);
    }

    #[test]
    fn minio_translation() {
        let target = mk_target(None, None, Some("http://127.0.0.1:9000"), true);
        let settings = S3ClientSettings::from_target(&target);
        assert_eq!(
            settings.endpoint_url.as_deref(),
            Some("http://127.0.0.1:9000")
        );
        assert!(settings.force_path_style);
    }

    #[test]
    fn retry_policy_disabled_synthetic_sdk_config() {
        let target = mk_target(None, None, None, false);
        let settings = S3ClientSettings::from_target(&target);

        // Fully synthetic shared SDK config: no network, no env/profile/IMDS.
        let sdk_config = aws_config::SdkConfig::builder()
            .region(Region::new("us-east-1"))
            .behavior_version(BehaviorVersion::latest())
            .build();
        let config = build_s3_config(&settings, &sdk_config);

        // RetryConfig::disabled() -> max_attempts == 1
        let retry = config.retry_config().expect("retry config must be present");
        assert_eq!(retry.max_attempts(), 1);
    }

    // ── S3-16 correction (Proposal D): ambient endpoint must not bypass
    //    validation; only the validated ARX target endpoint is a custom S3
    //    endpoint. Endpoint clearing happens at the S3 service-builder
    //    boundary (build_s3_config), not by reconstructing SdkConfig. ──

    #[test]
    fn inherited_s3_endpoint_is_cleared() {
        // Ambient endpoint URL carried by the shared SDK config (e.g.
        // AWS_ENDPOINT_URL / AWS_ENDPOINT_URL_S3 / profile endpoint_url).
        let sdk = aws_config::SdkConfig::builder()
            .region(Region::new("us-east-1"))
            .endpoint_url("https://user:password@example.invalid")
            .behavior_version(BehaviorVersion::latest())
            .build();

        // No ARX target endpoint configured.
        let settings = S3ClientSettings {
            region: None,
            profile: None,
            endpoint_url: None,
            force_path_style: false,
        };

        let config = build_s3_config(&settings, &sdk);

        // If the inherited ambient endpoint had leaked through, config would
        // differ from one built with an explicit (validated) target endpoint.
        // Instead the inherited endpoint is cleared, so config is identical to
        // build_s3_config with the SAME settings over a clean SdkConfig.
        let clean = aws_config::SdkConfig::builder()
            .region(Region::new("us-east-1"))
            .behavior_version(BehaviorVersion::latest())
            .build();
        let config_clean = build_s3_config(&settings, &clean);

        // No ambient endpoint survives; the two configs are equivalent.
        assert_eq!(
            format!("{:?}", config),
            format!("{:?}", config_clean),
            "inherited ambient endpoint must be cleared before build"
        );
    }

    #[test]
    fn target_endpoint_is_only_custom_endpoint() {
        let sdk = aws_config::SdkConfig::builder()
            .region(Region::new("us-east-1"))
            .endpoint_url("https://user:password@example.invalid") // ambient, must be cleared
            .behavior_version(BehaviorVersion::latest())
            .build();

        // ARX target supplies the only custom endpoint.
        let settings = S3ClientSettings {
            region: None,
            profile: None,
            endpoint_url: Some("http://127.0.0.1:9000".to_string()),
            force_path_style: true,
        };

        let config = build_s3_config(&settings, &sdk);

        // With a different target endpoint the config must differ from the
        // cleared (None) config above — proving the ARX target endpoint is
        // what became operational, not the ambient one.
        let none_settings = S3ClientSettings {
            region: None,
            profile: None,
            endpoint_url: None,
            force_path_style: true,
        };
        let config_none = build_s3_config(&none_settings, &sdk);

        assert_ne!(
            format!("{:?}", config),
            format!("{:?}", config_none),
            "target endpoint must be applied as the only custom endpoint"
        );

        // Sanity: retry invariant still holds through the boundary.
        assert_eq!(config.retry_config().expect("retry").max_attempts(), 1);
    }

    #[test]
    fn endpoint_userinfo_rejected() {
        let target = mk_target(
            None,
            None,
            Some("https://user:password@example.invalid/"),
            false,
        );
        let err = validate_endpoint(target.endpoint_url.as_deref().unwrap());
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(!msg.contains("password"));
        assert!(!msg.contains("user:"));
    }

    #[test]
    fn endpoint_signed_query_rejected() {
        let target = mk_target(
            None,
            None,
            Some("https://example.invalid/?X-Amz-Signature=SUPERSECRET"),
            false,
        );
        let err = validate_endpoint(target.endpoint_url.as_deref().unwrap());
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(!msg.contains("SUPERSECRET"));
        assert!(!msg.contains("X-Amz-Signature"));
    }

    #[test]
    fn endpoint_signed_query_case_insensitive() {
        let target = mk_target(
            None,
            None,
            Some("https://example.invalid/?x-amz-credential=SECRET"),
            false,
        );
        let err = validate_endpoint(target.endpoint_url.as_deref().unwrap());
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(!msg.contains("SECRET"));
        assert!(!msg.contains("x-amz-credential"));
    }

    #[test]
    fn normal_endpoint_allowed() {
        let target = mk_target(None, None, Some("http://127.0.0.1:9000"), true);
        let err = validate_endpoint(target.endpoint_url.as_deref().unwrap());
        assert!(err.is_ok());
    }

    // ── S3-17: provider lifecycle (no AWS client at construction) ──

    #[test]
    fn new_provider_client_uninitialized() {
        let target = mk_target(Some("eu-central-1"), Some("prod"), None, false);
        let provider = S3Provider::new(target);
        // No aws_config::load() runs in S3Provider::new.
        assert!(provider.client.get().is_none());
    }

    #[test]
    fn providers_have_distinct_client_cells() {
        let a = S3Provider::new(mk_target(Some("eu-central-1"), Some("prod"), None, false));
        let b = S3Provider::new(mk_target(
            None,
            Some("lab"),
            Some("http://127.0.0.1:9000"),
            true,
        ));
        // Distinct storage; not initialized merely for this assertion.
        assert!(a.client.get().is_none());
        assert!(b.client.get().is_none());
        assert!(!std::ptr::eq(&a.client, &b.client));
    }

    #[test]
    fn provider_preserves_target_config() {
        let t = mk_target(
            Some("eu-central-1"),
            Some("prod"),
            Some("http://127.0.0.1:9000"),
            true,
        );
        let provider = S3Provider::new(t.clone());
        assert_eq!(provider.target.id, t.id);
        assert_eq!(provider.target.region, t.region);
        assert_eq!(provider.target.profile, t.profile);
        assert_eq!(provider.target.endpoint_url, t.endpoint_url);
        assert_eq!(provider.target.force_path_style, t.force_path_style);
    }

    #[test]
    fn debug_output_does_not_contain_raw_endpoint() {
        let t = mk_target(None, None, Some("http://127.0.0.1:9000"), true);
        let provider = S3Provider::new(t);
        let dbg = format!("{:?}", provider);
        assert!(!dbg.contains("127.0.0.1"));
        assert!(!dbg.contains("9000"));
        assert!(dbg.contains("client_initialized"));
    }

    // ── S3-18: ListBuckets first-page mapping (offline, pure fixtures) ──

    use aws_sdk_s3::operation::list_buckets::ListBucketsOutput;
    use aws_sdk_s3::operation::list_objects_v2::ListObjectsV2Output;
    use aws_sdk_s3::primitives::ByteStream;
    use aws_sdk_s3::primitives::DateTime;
    use aws_sdk_s3::types::{Bucket, CommonPrefix, Object};

    fn bucket_named(name: &str) -> Bucket {
        Bucket::builder().name(name).build()
    }

    #[test]
    fn map_one_bucket_presentation_and_identity() {
        let out = ListBucketsOutput::builder()
            .buckets(bucket_named("company-artifacts"))
            .build();
        let page = map_list_buckets_page("aws-prod", &out).unwrap();
        assert_eq!(page.entries.len(), 1);
        let le = &page.entries[0];
        assert_eq!(le.entry.name, "company-artifacts");
        assert_eq!(le.entry.kind, crate::vfs::EntryKind::Directory);
        assert_eq!(le.entry.size, None);
        assert_eq!(le.entry.modified_unix_ms, None);
        match &le.identity {
            crate::vfs::EntryIdentity::S3Bucket(b) => {
                assert_eq!(b.target, "aws-prod");
                assert_eq!(b.bucket, "company-artifacts");
            }
            other => panic!("expected S3Bucket identity, got {:?}", other),
        }
    }

    #[test]
    fn map_multiple_buckets_stable_one_to_one() {
        let out = ListBucketsOutput::builder()
            .buckets(bucket_named("a"))
            .buckets(bucket_named("b"))
            .buckets(bucket_named("c"))
            .build();
        let page = map_list_buckets_page("t", &out).unwrap();
        assert_eq!(page.entries.len(), 3);
        let names: Vec<&str> = page.entries.iter().map(|e| e.entry.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
        // every entry carries exact identity with the same target id
        for le in &page.entries {
            match &le.identity {
                crate::vfs::EntryIdentity::S3Bucket(b) => assert_eq!(b.target, "t"),
                other => panic!("expected S3Bucket, got {:?}", other),
            }
        }
    }

    #[test]
    fn map_exact_case_preserved() {
        let out = ListBucketsOutput::builder()
            .buckets(bucket_named("Company-Artifacts"))
            .build();
        let page = map_list_buckets_page("t", &out).unwrap();
        assert_eq!(page.entries[0].entry.name, "Company-Artifacts");
        match &page.entries[0].identity {
            crate::vfs::EntryIdentity::S3Bucket(b) => {
                assert_eq!(b.bucket, "Company-Artifacts")
            }
            other => panic!("expected S3Bucket, got {:?}", other),
        }
    }

    #[test]
    fn map_exact_punctuation_preserved() {
        let out = ListBucketsOutput::builder()
            .buckets(bucket_named("my.bucket-01_example"))
            .build();
        let page = map_list_buckets_page("t", &out).unwrap();
        assert_eq!(page.entries[0].entry.name, "my.bucket-01_example");
    }

    #[test]
    fn map_missing_name_skipped_no_invented_identity() {
        // A bucket record without a name must be skipped, not turned into an
        // empty-string operational identity.
        let out = ListBucketsOutput::builder()
            .buckets(Bucket::builder().build()) // no name set
            .buckets(bucket_named("real-bucket"))
            .build();
        let page = map_list_buckets_page("t", &out).unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].entry.name, "real-bucket");
    }

    #[test]
    fn map_continuation_token_preserved_verbatim() {
        // Continuation preservation is now owned by next_list_buckets_continuation;
        // the opaque token must survive verbatim (no trim/normalize).
        let cont = next_list_buckets_continuation(None, Some("  opaque+/=token 日本語  ")).unwrap();
        assert_eq!(
            cont.as_ref().map(|c| c.token.as_str()),
            Some("  opaque+/=token 日本語  ")
        );
    }

    #[test]
    fn map_target_id_copied_everywhere() {
        let out = ListBucketsOutput::builder()
            .buckets(bucket_named("only"))
            .build();
        let page = map_list_buckets_page("exact-target-id", &out).unwrap();
        match &page.entries[0].identity {
            crate::vfs::EntryIdentity::S3Bucket(b) => assert_eq!(b.target, "exact-target-id"),
            other => panic!("expected S3Bucket, got {:?}", other),
        }
    }

    #[test]
    fn map_no_continuation_when_absent() {
        let out = ListBucketsOutput::builder()
            .buckets(bucket_named("a"))
            .build();
        let page = map_list_buckets_page("t", &out).unwrap();
        assert!(page.continuation.is_none());
    }

    // ── S3-18 correction: pure root-semantics classification (offline) ──

    fn loc(target: &str, bucket: Option<&str>, prefix: &str) -> Location {
        Location::S3 {
            target: target.to_string(),
            bucket: bucket.map(|b| b.to_string()),
            prefix: prefix.to_string(),
        }
    }
    fn bound_target(bucket: &str) -> S3TargetConfig {
        S3TargetConfig {
            id: "t".into(),
            name: "test".into(),
            bucket: Some(bucket.to_string()),
            region: None,
            profile: None,
            endpoint_url: None,
            force_path_style: false,
        }
    }
    fn root_target() -> S3TargetConfig {
        S3TargetConfig {
            id: "t".into(),
            name: "test".into(),
            bucket: None,
            region: None,
            profile: None,
            endpoint_url: None,
            force_path_style: false,
        }
    }

    #[test]
    fn account_target_exact_root_allowed() {
        let p = S3Provider::new(root_target());
        let l = loc("t", None, "");
        let r = p.classify_listing_location(&l);
        assert!(matches!(r, Ok(S3ListingScope::TargetRoot)));
    }

    #[test]
    fn account_target_root_with_prefix_rejected() {
        // Prefixes live inside a bucket; a non-empty root prefix is contradictory
        // and must fail closed (never normalized into "").
        for prefix in ["foo", "foo//bar", "foo/../bar", "/"] {
            let p = S3Provider::new(root_target());
            let l = loc("t", None, prefix);
            let err = p.classify_listing_location(&l).unwrap_err();
            assert!(
                matches!(err.kind(), io::ErrorKind::InvalidInput),
                "prefix {prefix:?} should reject, got {err:?}"
            );
            // Client must stay uninitialized on the rejected path.
            assert!(p.client.get().is_none());
        }
    }

    #[test]
    fn bucket_bound_target_cannot_enter_account_root() {
        let p = S3Provider::new(bound_target("company-artifacts"));
        let l = loc("t", None, "");
        let err = p.classify_listing_location(&l).unwrap_err();
        assert!(matches!(err.kind(), io::ErrorKind::NotFound));
        // Never reaches ListBuckets; client stays uninitialized.
        assert!(p.client.get().is_none());
    }

    #[test]
    fn bucket_bound_exact_bucket_is_not_list_buckets() {
        let p = S3Provider::new(bound_target("company-artifacts"));
        let l = loc("t", Some("company-artifacts"), "");
        let r = p.classify_listing_location(&l);
        match r.unwrap() {
            S3ListingScope::Bucket { bucket, prefix } => {
                assert_eq!(bucket, "company-artifacts");
                assert_eq!(prefix, "");
            }
            _ => panic!("expected Bucket scope"),
        }
        // ListObjectsV2 is S3-20; client stays uninitialized in S3-18.
        assert!(p.client.get().is_none());
    }

    #[test]
    fn bucket_bound_different_bucket_rejected() {
        let p = S3Provider::new(bound_target("company-artifacts"));
        let l = loc("t", Some("other-bucket"), "");
        let err = p.classify_listing_location(&l).unwrap_err();
        assert!(matches!(err.kind(), io::ErrorKind::NotFound));
        assert!(p.client.get().is_none());
    }

    #[test]
    fn account_target_bucket_location_still_classified_bucket() {
        let p = S3Provider::new(root_target());
        let l = loc("t", Some("listed-bucket"), "");
        let r = p.classify_listing_location(&l);
        assert!(matches!(r, Ok(S3ListingScope::Bucket { .. })));
        // No ListBuckets for a bucket scope in S3-18.
        assert!(p.client.get().is_none());
    }

    // ── S3-20: offline ListObjectsV2 first-page mapping matrix ──

    #[test]
    fn wire_prefix_bucket_root_is_empty() {
        assert_eq!(list_objects_wire_prefix(""), "");
    }
    #[test]
    fn wire_prefix_plain_nav_adds_delimiter() {
        assert_eq!(list_objects_wire_prefix("foo"), "foo/");
    }
    #[test]
    fn wire_prefix_literal_trailing_slash_adds_protocol_delimiter() {
        // The existing trailing "/" is literal namespace structure, NOT a
        // protocol delimiter to skip. S3-20R: always append exactly one.
        assert_eq!(list_objects_wire_prefix("foo/"), "foo//");
    }
    #[test]
    fn wire_prefix_repeated_literal_slashes_preserved() {
        assert_eq!(list_objects_wire_prefix("foo//bar"), "foo//bar/");
    }
    #[test]
    fn wire_prefix_nested_appends_delimiter() {
        assert_eq!(list_objects_wire_prefix("foo/bar"), "foo/bar/");
    }
    #[test]
    fn wire_prefix_dotdot_preserved() {
        assert_eq!(list_objects_wire_prefix("foo/../bar"), "foo/../bar/");
    }
    #[test]
    fn wire_prefix_dot_preserved() {
        assert_eq!(list_objects_wire_prefix("foo/./bar"), "foo/./bar/");
    }
    #[test]
    fn wire_prefix_unicode_preserved() {
        assert_eq!(list_objects_wire_prefix("日本語/資料"), "日本語/資料/");
    }

    #[test]
    fn list_objects_request_shape_is_stable_across_pages() {
        let sdk = aws_config::SdkConfig::builder()
            .region(Region::new("us-east-1"))
            .behavior_version(BehaviorVersion::latest())
            .build();
        let settings = S3ClientSettings {
            region: None,
            profile: None,
            endpoint_url: None,
            force_path_style: false,
        };
        let client = Client::from_conf(build_s3_config(&settings, &sdk));
        let opaque = "  opaque+/=token 日本語  ";

        for continuation in [None, Some(opaque)] {
            let input =
                list_objects_v2_request(&client, "Company-Artifacts", "foo//bar/", continuation)
                    .as_input()
                    .clone()
                    .build()
                    .unwrap();

            assert_eq!(input.bucket(), Some("Company-Artifacts"));
            assert_eq!(input.prefix(), Some("foo//bar/"));
            assert_eq!(input.delimiter(), Some("/"));
            assert_eq!(input.max_keys(), Some(1000));
            assert_eq!(input.continuation_token(), continuation);
            assert_eq!(input.start_after(), None);
        }
    }

    fn mk_object(key: &str, size: Option<i64>) -> Object {
        let mut b = Object::builder().key(key);
        if let Some(s) = size {
            b = b.size(s);
        }
        b.build()
    }
    fn mk_object_with_time(key: &str, size: i64, secs: i64) -> Object {
        Object::builder()
            .key(key)
            .size(size)
            .last_modified(DateTime::from_secs(secs))
            .build()
    }
    fn mk_common_prefix(prefix: &str) -> CommonPrefix {
        CommonPrefix::builder().prefix(prefix).build()
    }
    fn mk_list_out(
        contents: Vec<Object>,
        prefixes: Vec<CommonPrefix>,
        is_truncated: Option<bool>,
        token: Option<&str>,
    ) -> ListObjectsV2Output {
        let mut b = ListObjectsV2Output::builder();
        for c in contents {
            b = b.contents(c);
        }
        for p in prefixes {
            b = b.common_prefixes(p);
        }
        if let Some(t) = is_truncated {
            b = b.is_truncated(t);
        }
        if let Some(tok) = token {
            b = b.next_continuation_token(tok);
        }
        b.build()
    }

    #[test]
    fn object_direct_exact_identity_and_relative_presentation() {
        let out = mk_list_out(
            vec![mk_object("foo/bar.txt", Some(10))],
            vec![],
            Some(false),
            None,
        );
        let page = map_list_objects_v2_page("t", "b", "foo/", &out).unwrap();
        assert_eq!(page.entries.len(), 1);
        let e = &page.entries[0];
        assert_eq!(e.entry.name, "bar.txt");
        match &e.identity {
            EntryIdentity::S3Object(o) => {
                assert_eq!(o.target, "t");
                assert_eq!(o.bucket, "b");
                assert_eq!(o.key, "foo/bar.txt");
            }
            other => panic!("expected S3Object, got {:?}", other),
        }
    }

    #[test]
    fn object_awkward_double_slash_identity_exact() {
        let out = mk_list_out(
            vec![mk_object("foo//bar.txt", Some(1))],
            vec![],
            Some(false),
            None,
        );
        let page = map_list_objects_v2_page("t", "b", "foo/", &out).unwrap();
        match &page.entries[0].identity {
            EntryIdentity::S3Object(o) => assert_eq!(o.key, "foo//bar.txt"),
            other => panic!("expected S3Object, got {:?}", other),
        }
        // Presentation is relative; awkward slash preserved in name, not normalized.
        assert_eq!(page.entries[0].entry.name, "/bar.txt");
    }

    #[test]
    fn object_size_preserved() {
        let out = mk_list_out(
            vec![mk_object("foo/x", Some(42))],
            vec![],
            Some(false),
            None,
        );
        let page = map_list_objects_v2_page("t", "b", "foo/", &out).unwrap();
        assert_eq!(page.entries[0].entry.size, Some(42));
    }

    #[test]
    fn object_last_modified_preserved_when_valid() {
        let out = mk_list_out(
            vec![mk_object_with_time("foo/x", 1, 1_700_000_000)],
            vec![],
            Some(false),
            None,
        );
        let page = map_list_objects_v2_page("t", "b", "foo/", &out).unwrap();
        assert_eq!(
            page.entries[0].entry.modified_unix_ms,
            Some(1_700_000_000_000)
        );
    }

    #[test]
    fn object_missing_key_skipped() {
        // AWS object with no key field (key() == None) is unusable; skip it.
        let out = ListObjectsV2Output::builder()
            .contents(Object::builder().size(1).build())
            .is_truncated(false)
            .build();
        let page = map_list_objects_v2_page("t", "b", "foo/", &out).unwrap();
        assert!(page.entries.is_empty());
    }

    #[test]
    fn object_outside_requested_prefix_rejected() {
        let out = mk_list_out(
            vec![mk_object("other/bar.txt", Some(1))],
            vec![],
            Some(false),
            None,
        );
        let err = map_list_objects_v2_page("t", "b", "foo/", &out).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn common_prefix_exact_identity_and_relative_presentation() {
        let out = mk_list_out(
            vec![],
            vec![mk_common_prefix("foo/bar/")],
            Some(false),
            None,
        );
        let page = map_list_objects_v2_page("t", "b", "foo/", &out).unwrap();
        assert_eq!(page.entries.len(), 1);
        let e = &page.entries[0];
        assert_eq!(e.entry.name, "bar");
        match &e.identity {
            EntryIdentity::S3Prefix(p) => {
                assert_eq!(p.target, "t");
                assert_eq!(p.bucket, "b");
                assert_eq!(p.prefix, "foo/bar/");
            }
            other => panic!("expected S3Prefix, got {:?}", other),
        }
    }

    #[test]
    fn common_prefix_outside_requested_prefix_rejected() {
        let out = mk_list_out(
            vec![],
            vec![mk_common_prefix("other/bar/")],
            Some(false),
            None,
        );
        let err = map_list_objects_v2_page("t", "b", "foo/", &out).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn self_common_prefix_not_emitted_as_child() {
        // AWS returns the requested prefix itself as a CommonPrefix -> not a child,
        // skipped (no self-navigation entry).
        let out = mk_list_out(vec![], vec![mk_common_prefix("foo/")], Some(false), None);
        let page = map_list_objects_v2_page("t", "b", "foo/", &out).unwrap();
        assert!(page.entries.is_empty());
    }

    #[test]
    fn current_zero_byte_folder_marker_suppressed() {
        let out = mk_list_out(vec![mk_object("foo/", Some(0))], vec![], Some(false), None);
        let page = map_list_objects_v2_page("t", "b", "foo/", &out).unwrap();
        assert!(page.entries.is_empty());
    }

    #[test]
    fn child_zero_byte_marker_deduped_against_common_prefix() {
        let out = mk_list_out(
            vec![mk_object("foo/bar/", Some(0))],
            vec![mk_common_prefix("foo/bar/")],
            Some(false),
            None,
        );
        let page = map_list_objects_v2_page("t", "b", "foo/", &out).unwrap();
        assert_eq!(page.entries.len(), 1);
        assert!(matches!(
            page.entries[0].identity,
            EntryIdentity::S3Prefix(_)
        ));
    }

    #[test]
    fn nonzero_trailing_slash_object_preserved() {
        let out = mk_list_out(
            vec![mk_object("foo/special/", Some(7))],
            vec![],
            Some(false),
            None,
        );
        let page = map_list_objects_v2_page("t", "b", "foo/", &out).unwrap();
        assert_eq!(page.entries.len(), 1);
        match &page.entries[0].identity {
            EntryIdentity::S3Object(o) => assert_eq!(o.key, "foo/special/"),
            other => panic!("expected S3Object, got {:?}", other),
        }
    }

    #[test]
    fn unmatched_zero_byte_slash_object_preserved() {
        let out = mk_list_out(
            vec![mk_object("foo/special/", Some(0))],
            vec![],
            Some(false),
            None,
        );
        let page = map_list_objects_v2_page("t", "b", "foo/", &out).unwrap();
        assert_eq!(page.entries.len(), 1);
        match &page.entries[0].identity {
            EntryIdentity::S3Object(o) => assert_eq!(o.key, "foo/special/"),
            other => panic!("expected S3Object, got {:?}", other),
        }
    }

    // ── S3-22: adversarial ListObjectsV2 identity regressions (offline) ──

    #[test]
    fn object_adversarial_identity_matrix_preserves_literal_keys() {
        let expected = [
            "foo/../bar.txt",
            "foo/./bar.txt",
            "foo/file name with spaces.txt",
            "foo/каталог/файл.txt",
            "foo/日本語/資料.txt",
            "foo/emoji/🧙‍♂️.txt",
            "foo/empty.bin",
        ];
        let out = mk_list_out(
            expected
                .iter()
                .map(|key| mk_object(key, Some(if *key == "foo/empty.bin" { 0 } else { 1 })))
                .collect(),
            vec![],
            Some(false),
            None,
        );
        let page = map_list_objects_v2_page("t", "b", "foo/", &out).unwrap();

        assert_eq!(page.entries.len(), expected.len());
        for key in expected {
            assert_eq!(
                page.entries
                    .iter()
                    .filter(|entry| {
                        entry.identity
                            == EntryIdentity::S3Object(S3ObjectRef {
                                target: "t".into(),
                                bucket: "b".into(),
                                key: key.into(),
                            })
                    })
                    .count(),
                1,
                "expected exact object identity for {key:?} exactly once"
            );
        }
        let empty = page
            .entries
            .iter()
            .find(|entry| {
                entry.identity
                    == EntryIdentity::S3Object(S3ObjectRef {
                        target: "t".into(),
                        bucket: "b".into(),
                        key: "foo/empty.bin".into(),
                    })
            })
            .unwrap();
        assert_eq!(empty.entry.kind, EntryKind::File);
        assert_eq!(empty.entry.size, Some(0));
    }

    #[test]
    fn common_prefix_adversarial_identity_matrix_preserves_literal_prefixes() {
        let expected = ["foo//nested/", "foo/../nested/", "foo/日本語/"];
        let out = mk_list_out(
            vec![],
            expected
                .iter()
                .map(|prefix| mk_common_prefix(prefix))
                .collect(),
            Some(false),
            None,
        );
        let page = map_list_objects_v2_page("t", "b", "foo/", &out).unwrap();

        assert_eq!(page.entries.len(), expected.len());
        for prefix in expected {
            assert_eq!(
                page.entries
                    .iter()
                    .filter(|entry| {
                        entry.identity
                            == EntryIdentity::S3Prefix(S3PrefixRef {
                                target: "t".into(),
                                bucket: "b".into(),
                                prefix: prefix.into(),
                            })
                    })
                    .count(),
                1,
                "expected exact prefix identity for {prefix:?} exactly once"
            );
        }
    }

    #[test]
    fn adversarial_marker_identity_interactions_are_exact() {
        let out = mk_list_out(
            vec![
                mk_object("foo//nested/", Some(0)),
                mk_object("foo//special/", Some(0)),
                mk_object("foo/../special/", Some(1)),
            ],
            vec![mk_common_prefix("foo//nested/")],
            Some(false),
            None,
        );
        let page = map_list_objects_v2_page("t", "b", "foo/", &out).unwrap();
        let expected = [
            EntryIdentity::S3Prefix(S3PrefixRef {
                target: "t".into(),
                bucket: "b".into(),
                prefix: "foo//nested/".into(),
            }),
            EntryIdentity::S3Object(S3ObjectRef {
                target: "t".into(),
                bucket: "b".into(),
                key: "foo//special/".into(),
            }),
            EntryIdentity::S3Object(S3ObjectRef {
                target: "t".into(),
                bucket: "b".into(),
                key: "foo/../special/".into(),
            }),
        ];

        assert_eq!(page.entries.len(), expected.len());
        for identity in expected {
            assert_eq!(
                page.entries
                    .iter()
                    .filter(|entry| entry.identity == identity)
                    .count(),
                1,
                "expected marker interaction identity exactly once"
            );
        }
    }

    #[test]
    fn mixed_adversarial_page_emits_exact_identity_set() {
        let out = mk_list_out(
            vec![
                mk_object("foo//bar.txt", Some(1)),
                mk_object("foo/../bar.txt", Some(1)),
                mk_object("foo/./bar.txt", Some(1)),
                mk_object("foo/file name.txt", Some(1)),
                mk_object("foo/日本語.txt", Some(1)),
                mk_object("foo/🧙‍♂️.txt", Some(1)),
                mk_object("foo/empty.bin", Some(0)),
                mk_object("foo//nested/", Some(0)),
            ],
            vec![mk_common_prefix("foo//nested/")],
            Some(false),
            None,
        );
        let page = map_list_objects_v2_page("t", "b", "foo/", &out).unwrap();
        let expected = [
            EntryIdentity::S3Object(S3ObjectRef {
                target: "t".into(),
                bucket: "b".into(),
                key: "foo//bar.txt".into(),
            }),
            EntryIdentity::S3Object(S3ObjectRef {
                target: "t".into(),
                bucket: "b".into(),
                key: "foo/../bar.txt".into(),
            }),
            EntryIdentity::S3Object(S3ObjectRef {
                target: "t".into(),
                bucket: "b".into(),
                key: "foo/./bar.txt".into(),
            }),
            EntryIdentity::S3Object(S3ObjectRef {
                target: "t".into(),
                bucket: "b".into(),
                key: "foo/file name.txt".into(),
            }),
            EntryIdentity::S3Object(S3ObjectRef {
                target: "t".into(),
                bucket: "b".into(),
                key: "foo/日本語.txt".into(),
            }),
            EntryIdentity::S3Object(S3ObjectRef {
                target: "t".into(),
                bucket: "b".into(),
                key: "foo/🧙‍♂️.txt".into(),
            }),
            EntryIdentity::S3Object(S3ObjectRef {
                target: "t".into(),
                bucket: "b".into(),
                key: "foo/empty.bin".into(),
            }),
            EntryIdentity::S3Prefix(S3PrefixRef {
                target: "t".into(),
                bucket: "b".into(),
                prefix: "foo//nested/".into(),
            }),
        ];

        assert_eq!(page.entries.len(), expected.len());
        for identity in expected {
            assert_eq!(
                page.entries
                    .iter()
                    .filter(|entry| entry.identity == identity)
                    .count(),
                1,
                "expected identity exactly once"
            );
        }
    }

    #[test]
    fn first_page_not_truncated_is_end() {
        let out = mk_list_out(vec![], vec![], Some(false), None);
        let cont = next_list_objects_v2_continuation(
            None,
            out.is_truncated(),
            out.next_continuation_token(),
        )
        .unwrap();
        assert!(cont.is_none());
    }

    #[test]
    fn next_page_repeated_token_protocol_error() {
        let err = next_list_objects_v2_continuation(
            Some("sensitive-token"),
            Some(true),
            Some("sensitive-token"),
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(!err.to_string().contains("sensitive-token"));
    }

    #[test]
    fn next_page_advancing_token_preserved_verbatim() {
        let returned = "  next+/=token 日本語  ";
        let cont =
            next_list_objects_v2_continuation(Some("consumed-token"), Some(true), Some(returned))
                .unwrap()
                .unwrap();
        assert_eq!(cont.token, returned);
    }

    #[test]
    fn list_objects_final_next_page_is_end() {
        let cont =
            next_list_objects_v2_continuation(Some("consumed-token"), Some(false), None).unwrap();
        assert!(cont.is_none());
    }

    #[test]
    fn first_page_truncated_token_preserved() {
        let out = mk_list_out(vec![], vec![], Some(true), Some("opaque-token"));
        let cont = next_list_objects_v2_continuation(
            None,
            out.is_truncated(),
            out.next_continuation_token(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(cont.token, "opaque-token");
    }

    #[test]
    fn first_page_truncated_missing_token_rejected() {
        let out = mk_list_out(vec![], vec![], Some(true), None);
        let err = next_list_objects_v2_continuation(
            None,
            out.is_truncated(),
            out.next_continuation_token(),
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn first_page_truncated_empty_token_rejected() {
        let out = mk_list_out(vec![], vec![], Some(true), Some(""));
        let err = next_list_objects_v2_continuation(
            None,
            out.is_truncated(),
            out.next_continuation_token(),
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn false_with_next_token_rejected_as_contradictory() {
        let out = mk_list_out(vec![], vec![], Some(false), Some("opaque-token"));
        let err = next_list_objects_v2_continuation(
            None,
            out.is_truncated(),
            out.next_continuation_token(),
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn missing_is_truncated_rejected() {
        let out = mk_list_out(vec![], vec![], None, None);
        let err = next_list_objects_v2_continuation(
            None,
            out.is_truncated(),
            out.next_continuation_token(),
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn wrong_target_id_fails_closed() {
        let p = S3Provider::new(root_target());
        let l = loc("other-target", None, "");
        let err = p.classify_listing_location(&l).unwrap_err();
        assert!(matches!(err.kind(), io::ErrorKind::NotFound));
        assert!(p.client.get().is_none());
    }

    // ── S3-19: ListBuckets pagination protocol (offline) ──

    #[test]
    fn first_page_no_return_token_is_end() {
        assert!(
            next_list_buckets_continuation(None, None)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn first_page_return_token_preserved() {
        let c = next_list_buckets_continuation(None, Some("opaque-token"))
            .unwrap()
            .unwrap();
        assert_eq!(c.token, "opaque-token");
    }

    #[test]
    fn next_page_advancing_token() {
        let c = next_list_buckets_continuation(Some("token-A"), Some("token-B"))
            .unwrap()
            .unwrap();
        assert_eq!(c.token, "token-B");
    }

    #[test]
    fn final_next_page_is_end() {
        assert!(
            next_list_buckets_continuation(Some("token-A"), None)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn repeated_token_protocol_error() {
        let err = next_list_buckets_continuation(Some("token-A"), Some("token-A")).unwrap_err();
        assert!(matches!(err.kind(), io::ErrorKind::InvalidData));
        // No token value echoed into the message.
        assert!(!err.to_string().contains("token-A"));
    }

    #[test]
    fn empty_returned_token_protocol_error() {
        let err = next_list_buckets_continuation(Some("token-A"), Some("")).unwrap_err();
        assert!(matches!(err.kind(), io::ErrorKind::InvalidData));
    }

    #[test]
    fn opaque_input_token_preserved() {
        // Punctuation / non-ASCII / whitespace must survive verbatim.
        let token = "  opaque+/=token 日本語  ";
        let c = next_list_buckets_continuation(None, Some(token))
            .unwrap()
            .unwrap();
        assert_eq!(c.token, token);
    }

    // ── S3-19 regression (NIT from S3-18): bucket-bound via PUBLIC list_page ──

    #[test]
    fn bucket_bound_exact_bucket_classifies_to_list_objects_v2_scope() {
        // S3-20 implements ListObjectsV2 for exact bound bucket. Offline check:
        // the location classifies to Bucket scope (no ListBuckets, no client init).
        let p = S3Provider::new(bound_target("company-artifacts"));
        let l = loc("t", Some("company-artifacts"), "");
        match p.classify_listing_location(&l).unwrap() {
            S3ListingScope::Bucket { bucket, prefix } => {
                assert_eq!(bucket, "company-artifacts");
                assert_eq!(prefix, "");
            }
            other => panic!("expected Bucket scope, got {:?}", other),
        }
        assert!(p.client.get().is_none());
    }

    #[tokio::test]
    async fn bucket_bound_no_account_root_via_list_page() {
        let p = S3Provider::new(bound_target("company-artifacts"));
        let l = Location::S3 {
            target: "t".to_string(),
            bucket: None,
            prefix: String::new(),
        };
        let err = p.list_page(&l, None).await.unwrap_err();
        assert!(matches!(err.kind(), io::ErrorKind::NotFound));
        assert!(p.client.get().is_none());
    }

    #[tokio::test]
    async fn empty_consumed_token_rejected_offline() {
        let p = S3Provider::new(root_target());
        let l = Location::S3 {
            target: "t".to_string(),
            bucket: None,
            prefix: String::new(),
        };
        let cont = ProviderContinuation {
            token: String::new(),
        };
        let err = p.list_page(&l, Some(&cont)).await.unwrap_err();
        assert!(matches!(err.kind(), io::ErrorKind::InvalidData));
        assert!(p.client.get().is_none());
    }

    #[tokio::test]
    async fn list_objects_empty_consumed_token_rejected_offline() {
        let p = S3Provider::new(bound_target("company-artifacts"));
        let l = loc("t", Some("company-artifacts"), "foo");
        let cont = ProviderContinuation {
            token: String::new(),
        };
        let err = p.list_page(&l, Some(&cont)).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(p.client.get().is_none());
    }

    // ── S3-20R: reversible nav/wire delimiter seam ──

    // Roundtrip invariant: for every valid exact provider CommonPrefix P,
    // nav(P) removes exactly one final '/' and wire(nav(P)) == P. This holds
    // even for repeated-delimiter structure. No production nav helper is added;
    // this pins the seam contract only.
    #[test]
    fn wire_nav_roundtrip_preserves_repeated_delimiters() {
        let cases: &[(&str, &str)] = &[
            ("foo/", "foo"),
            ("foo//", "foo/"),
            ("foo///", "foo//"),
            ("foo/bar/", "foo/bar"),
            ("foo//bar/", "foo//bar"),
            ("foo/../bar/", "foo/../bar"),
            ("foo/./bar/", "foo/./bar"),
            ("日本語/資料/", "日本語/資料"),
        ];
        for (exact, nav) in cases {
            let nav_computed = exact
                .strip_suffix('/')
                .expect("valid CommonPrefix delimiter");
            assert_eq!(nav_computed, *nav, "nav of {exact}");
            assert_eq!(
                list_objects_wire_prefix(nav_computed),
                *exact,
                "roundtrip of {exact}"
            );
        }
    }

    #[test]
    fn common_prefix_exact_identity_preserved_with_repeated_delimiter() {
        // wire "foo/" (nav "foo" + one delimiter); CommonPrefix "foo//" is a
        // child of that wire and carries the repeated literal slash.
        let out = mk_list_out(vec![], vec![mk_common_prefix("foo//")], Some(false), None);
        let page = map_list_objects_v2_page("t", "b", "foo/", &out).unwrap();
        let prefix_entries: Vec<&S3PrefixRef> = page
            .entries
            .iter()
            .filter_map(|e| match &e.identity {
                EntryIdentity::S3Prefix(p) => Some(p),
                _ => None,
            })
            .collect();
        assert_eq!(prefix_entries.len(), 1);
        assert_eq!(prefix_entries[0].prefix, "foo//");
    }

    #[test]
    fn common_prefix_missing_delimiter_rejected() {
        // Delimiter="/" => every CommonPrefix MUST end in '/'. "foo/child" is
        // malformed; reject without inventing an identity. wire is a proper
        // parent so the record reaches the delimiter check (not self-skip).
        let out = mk_list_out(
            vec![],
            vec![mk_common_prefix("foo/child")],
            Some(false),
            None,
        );
        let err = map_list_objects_v2_page("t", "b", "foo/", &out).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn common_prefix_awkward_valid_repeated_delimiter_accepted() {
        let out = mk_list_out(
            vec![],
            vec![mk_common_prefix("foo/../child/")],
            Some(false),
            None,
        );
        let page = map_list_objects_v2_page("t", "b", "foo/../", &out).unwrap();
        assert!(page.entries.iter().any(|e| matches!(
            &e.identity,
            EntryIdentity::S3Prefix(p) if p.prefix == "foo/../child/"
        )));
    }

    #[test]
    fn common_prefix_unicode_valid() {
        let out = mk_list_out(
            vec![],
            vec![mk_common_prefix("日本語/資料/")],
            Some(false),
            None,
        );
        let page = map_list_objects_v2_page("t", "b", "日本語/", &out).unwrap();
        assert!(page.entries.iter().any(|e| matches!(
            &e.identity,
            EntryIdentity::S3Prefix(p) if p.prefix == "日本語/資料/"
        )));
    }

    // Repeated-slash current-folder marker must stay suppressed after the
    // unconditional wire-delimiter change (wire == key == CommonPrefix).
    #[test]
    fn repeated_slash_current_folder_marker_suppressed() {
        let out = mk_list_out(
            vec![mk_object("foo//", Some(0))],
            vec![mk_common_prefix("foo//")],
            Some(false),
            None,
        );
        let page = map_list_objects_v2_page("t", "b", "foo//", &out).unwrap();
        // Both the zero-byte self marker and the self CommonPrefix are suppressed.
        assert_eq!(page.entries.len(), 0);
    }

    #[test]
    fn repeated_slash_child_marker_deduped_to_one_prefix() {
        // wire "foo//"; zero-byte child marker key "foo//child/" is deduped
        // against the exact CommonPrefix, leaving exactly one S3PrefixRef.
        let out = mk_list_out(
            vec![mk_object("foo//child/", Some(0))],
            vec![mk_common_prefix("foo//child/")],
            Some(false),
            None,
        );
        let page = map_list_objects_v2_page("t", "b", "foo//", &out).unwrap();
        let prefix_count = page
            .entries
            .iter()
            .filter(|e| matches!(&e.identity, EntryIdentity::S3Prefix(_)))
            .count();
        assert_eq!(prefix_count, 1);
        assert!(page.entries.iter().any(|e| matches!(
            &e.identity,
            EntryIdentity::S3Prefix(p) if p.prefix == "foo//child/"
        )));
    }

    // ── S3-27: bounded GetObject preview via identity seam (offline) ──

    use aws_sdk_s3::error::ErrorMetadata;
    use aws_sdk_s3::primitives::SdkBody;

    /// Build validated request params for a listed S3 object with an exact key.
    /// Never touches `entry.name`; the S3ObjectRef is the sole authority.
    fn preview_params(key: &str) -> io::Result<GetObjectParams> {
        let p = S3Provider::new(root_target());
        let l = loc("t", Some("b"), "");
        let listed = ListedEntry {
            entry: Entry {
                name: "display-name".into(),
                kind: EntryKind::File,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::S3Object(S3ObjectRef {
                target: "t".into(),
                bucket: "b".into(),
                key: key.into(),
            }),
        };
        let refr = p.classify_read_identity(&l, &listed)?;
        build_get_object_params(refr, 1024)
    }

    #[test]
    fn read_preview_preserves_exact_target_bucket_and_keys() {
        for key in [
            "k.txt",
            "foo//bar.txt",
            "foo/../bar.txt",
            "foo/./bar.txt",
            "a b/c d.txt",
            "日本語/資料.txt",
            "emoji/����‍�����.txt",
        ] {
            let params = preview_params(key).unwrap();
            assert_eq!(params.bucket, "b", "bucket preserved for {key}");
            assert_eq!(params.key, key, "key preserved byte-for-byte for {key}");
            assert_eq!(params.range, "bytes=0-1024", "range bounded for {key}");
        }
        // exact target identity preserved (authority == configured target id)
        let p = S3Provider::new(root_target());
        let l = loc("t", Some("b"), "");
        let listed = ListedEntry {
            entry: Entry {
                name: "x".into(),
                kind: EntryKind::File,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::S3Object(S3ObjectRef {
                target: "t".into(),
                bucket: "b".into(),
                key: "k".into(),
            }),
        };
        let refr = p.classify_read_identity(&l, &listed).unwrap();
        assert_eq!(refr.target, "t");
    }

    #[test]
    fn read_preview_uses_ref_key_not_display_name() {
        let p = S3Provider::new(root_target());
        let l = loc("t", Some("b"), "");
        let listed = ListedEntry {
            entry: Entry {
                name: "WRONG-DISPLAY-NAME".into(),
                kind: EntryKind::File,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::S3Object(S3ObjectRef {
                target: "t".into(),
                bucket: "b".into(),
                key: "foo/bar.txt".into(),
            }),
        };
        let refr = p.classify_read_identity(&l, &listed).unwrap();
        let params = build_get_object_params(refr, 256).unwrap();
        assert_eq!(params.key, "foo/bar.txt");
        assert_ne!(params.key, listed.entry.name);
    }

    #[test]
    fn read_preview_target_mismatch_rejected_before_client() {
        let p = S3Provider::new(root_target());
        let l = loc("t", Some("b"), "");
        let listed = ListedEntry {
            entry: Entry {
                name: "x".into(),
                kind: EntryKind::File,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::S3Object(S3ObjectRef {
                target: "other".into(),
                bucket: "b".into(),
                key: "k".into(),
            }),
        };
        assert!(p.classify_read_identity(&l, &listed).is_err());
        assert!(p.client.get().is_none());
    }

    #[test]
    fn read_preview_bucket_mismatch_rejected_before_client() {
        let p = S3Provider::new(root_target());
        let l = loc("t", Some("b"), "");
        let listed = ListedEntry {
            entry: Entry {
                name: "x".into(),
                kind: EntryKind::File,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::S3Object(S3ObjectRef {
                target: "t".into(),
                bucket: "other".into(),
                key: "k".into(),
            }),
        };
        assert!(p.classify_read_identity(&l, &listed).is_err());
        assert!(p.client.get().is_none());
    }

    #[test]
    fn read_preview_bucket_bound_escape_rejected_before_client() {
        let p = S3Provider::new(bound_target("b"));
        let l = loc("t", Some("b"), "");
        let listed = ListedEntry {
            entry: Entry {
                name: "x".into(),
                kind: EntryKind::File,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::S3Object(S3ObjectRef {
                target: "t".into(),
                bucket: "evil".into(),
                key: "k".into(),
            }),
        };
        assert!(p.classify_read_identity(&l, &listed).is_err());
        assert!(p.client.get().is_none());
    }

    #[test]
    fn read_preview_s3prefix_rejected() {
        let p = S3Provider::new(root_target());
        let l = loc("t", Some("b"), "");
        let listed = ListedEntry {
            entry: Entry {
                name: "x".into(),
                kind: EntryKind::Directory,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::S3Prefix(S3PrefixRef {
                target: "t".into(),
                bucket: "b".into(),
                prefix: "p/".into(),
            }),
        };
        let err = p.classify_read_identity(&l, &listed).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
        assert!(p.client.get().is_none());
    }

    #[test]
    fn read_preview_s3bucket_rejected() {
        let p = S3Provider::new(root_target());
        let l = loc("t", Some("b"), "");
        let listed = ListedEntry {
            entry: Entry {
                name: "x".into(),
                kind: EntryKind::Directory,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::S3Bucket(S3BucketRef {
                target: "t".into(),
                bucket: "b".into(),
            }),
        };
        let err = p.classify_read_identity(&l, &listed).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
        assert!(p.client.get().is_none());
    }

    #[test]
    fn read_preview_zero_and_overflow_cap_rejected() {
        let refr = S3ObjectRef {
            target: "t".into(),
            bucket: "b".into(),
            key: "k".into(),
        };
        assert_eq!(
            build_get_object_params(&refr, 0).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            build_get_object_params(&refr, usize::MAX)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn read_preview_range_is_bounded() {
        let refr = S3ObjectRef {
            target: "t".into(),
            bucket: "b".into(),
            key: "k".into(),
        };
        let params = build_get_object_params(&refr, 100).unwrap();
        assert_eq!(params.range, "bytes=0-100");
        assert_eq!(params.probe_len, 101);
        let big = build_get_object_params(&refr, 1_000_000).unwrap();
        assert_eq!(big.range, "bytes=0-1000000");
    }

    #[tokio::test]
    async fn read_preview_body_read_caps_at_probe_len() {
        let big = vec![b'x'; 10_000];
        let reader = ByteStream::new(SdkBody::from(big)).into_async_read();
        let (bytes, truncated) = read_bounded_body(reader, 5).await.unwrap();
        assert_eq!(bytes.len(), 5, "no full-body collect");
        assert!(truncated, "cap reached => more bytes exist");

        let small = vec![b'y'; 3];
        let reader = ByteStream::new(SdkBody::from(small)).into_async_read();
        let (bytes, truncated) = read_bounded_body(reader, 5).await.unwrap();
        assert_eq!(bytes.len(), 3);
        assert!(!truncated);

        let reader = ByteStream::new(SdkBody::from(Vec::<u8>::new())).into_async_read();
        let (bytes, truncated) = read_bounded_body(reader, 5).await.unwrap();
        assert_eq!(bytes.len(), 0);
        assert!(!truncated);
    }

    #[test]
    fn read_preview_error_mapping_is_sanitized() {
        let invalid_range =
            GetObjectError::generic(ErrorMetadata::builder().code("InvalidRange").build());
        let empty = map_get_object_error(&invalid_range).unwrap();
        assert_eq!(empty.bytes, Vec::<u8>::new());
        assert!(!empty.truncated);

        for code in [
            "AccessDenied",
            "NoSuchKey",
            "InvalidObjectState",
            "InternalError",
        ] {
            let err = GetObjectError::generic(ErrorMetadata::builder().code(code).build());
            let io_err = map_get_object_error(&err).unwrap_err();
            let msg = io_err.to_string();
            assert_eq!(msg, "S3 GetObject preview request failed");
            assert!(!msg.contains("X-Amz"), "no signed-query leak for {code}");
            assert!(
                !msg.contains("Authorization"),
                "no auth-header leak for {code}"
            );
            assert!(!msg.contains("secret-key"), "no key leak for {code}");
        }
    }

    // ── S3-28: truthful BoundedRead boundary tests (pure, no AWS) ──

    // Test oracle: mirrors production preview mapping exactly.
    // `probe_len = max_bytes + 1`, local `take(max_bytes)`, isolated constructor.
    async fn preview_object(object: &[u8], max_bytes: usize) -> BoundedRead {
        let probe_len = max_bytes + 1;
        let (buf, truncated) = read_bounded_body(object, probe_len)
            .await
            .expect("read_bounded_body must not fail on pure in-memory readers");
        let bytes: Vec<u8> = buf.into_iter().take(max_bytes).collect();
        s3_bounded_read(bytes, truncated)
    }

    fn binary_payload(len: usize) -> Vec<u8> {
        // Deterministic but arbitrary binary: 0x00, 0xFF, control bytes, non-UTF8
        (0..len as u8)
            .map(|i| i.wrapping_mul(37).wrapping_add(11))
            .collect()
    }

    fn invalid_utf8_payload() -> Vec<u8> {
        // Standalone continuation byte (0x80), partial 3-byte lead (0xE2 0x28),
        // overlong lead (0xC0 0x80), and valid ascii mix
        vec![0xE2, 0x28, 0xFF, 0x80, 0xC0, 0x80, 0x41, 0x42, 0x43]
    }

    const N: usize = 16;

    #[tokio::test]
    async fn bounded_read_boundary_zero_bytes() {
        let r = preview_object(&[], N).await;
        assert_eq!(r.bytes, Vec::<u8>::new());
        assert!(!r.truncated, "real zero-byte object → not truncated");
        assert!(r.bytes.len() <= N);
        assert!(r.unix_mode.is_none() && r.unix_uid.is_none() && r.unix_gid.is_none());
        assert!(
            r.into_revision().is_err(),
            "S3 BoundedRead never usable as edit revision (no POSIX fields)"
        );
    }

    #[tokio::test]
    async fn bounded_read_boundary_one_byte() {
        let obj = binary_payload(1);
        let r = preview_object(&obj, N).await;
        assert_eq!(r.bytes, obj, "single byte preserved exactly");
        assert!(!r.truncated, "1 byte < N → object proved ended");
        assert!(r.bytes.len() <= N);
        assert!(r.unix_mode.is_none() && r.unix_uid.is_none() && r.unix_gid.is_none());
        assert!(r.into_revision().is_err());
    }

    #[tokio::test]
    async fn bounded_read_boundary_n_minus_1() {
        let obj = binary_payload(N - 1);
        let r = preview_object(&obj, N).await;
        assert_eq!(r.bytes, obj, "N-1 bytes preserved exactly");
        assert!(!r.truncated, "N-1 bytes < N → object proved ended");
        assert!(r.bytes.len() <= N);
        assert!(r.unix_mode.is_none() && r.unix_uid.is_none() && r.unix_gid.is_none());
        assert!(r.into_revision().is_err());
    }

    #[tokio::test]
    async fn bounded_read_boundary_exact_n() {
        let obj = binary_payload(N);
        let r = preview_object(&obj, N).await;
        assert_eq!(r.bytes, obj, "exact N bytes preserved exactly");
        assert!(
            !r.truncated,
            "exact N bytes < probe(N+1) → object proved ended"
        );
        assert!(r.bytes.len() <= N);
        assert!(r.unix_mode.is_none() && r.unix_uid.is_none() && r.unix_gid.is_none());
        assert!(r.into_revision().is_err());
    }

    #[tokio::test]
    async fn bounded_read_boundary_n_plus_1() {
        let obj = binary_payload(N + 1);
        let r = preview_object(&obj, N).await;
        assert_eq!(r.bytes, &obj[..N], "first N bytes preserved; N+1st dropped");
        assert!(r.truncated, "N+1 bytes ≥ probe(N+1) → truncated");
        assert_eq!(r.bytes.len(), N);
        assert!(r.unix_mode.is_none() && r.unix_uid.is_none() && r.unix_gid.is_none());
        assert!(r.into_revision().is_err());
    }

    #[tokio::test]
    async fn bounded_read_boundary_large() {
        let obj = binary_payload(1000);
        let r = preview_object(&obj, N).await;
        assert_eq!(r.bytes, &obj[..N], "first N bytes preserved; rest dropped");
        assert!(r.truncated, "large object ≥ probe → truncated");
        assert_eq!(r.bytes.len(), N);
        assert!(r.unix_mode.is_none() && r.unix_uid.is_none() && r.unix_gid.is_none());
        assert!(r.into_revision().is_err());
    }

    #[tokio::test]
    async fn bounded_read_binary_preserved_at_all_boundaries() {
        // Binary data preserved exactly at every boundary length
        for len in [0, 1, N - 1, N, N + 1, 1000] {
            let obj = binary_payload(len);
            let r = preview_object(&obj, N).await;
            let expected_len = std::cmp::min(len, N);
            assert_eq!(
                r.bytes,
                &obj[..expected_len],
                "binary preserved at len={len}"
            );
            assert!(r.unix_mode.is_none() && r.unix_uid.is_none() && r.unix_gid.is_none());
            assert!(r.into_revision().is_err());
        }
    }

    #[tokio::test]
    async fn bounded_read_invalid_utf8_preserved_at_all_boundaries() {
        // Invalid UTF-8 sequences preserved exactly; formatter owns rejection
        let obj = invalid_utf8_payload();
        for len in [0, 1, N - 1, N, N + 1, 1000] {
            let slice = if obj.len() >= len { &obj[..len] } else { &obj };
            let r = preview_object(slice, N).await;
            let expected_len = std::cmp::min(slice.len(), N);
            assert_eq!(
                r.bytes,
                &slice[..expected_len],
                "invalid UTF-8 preserved at len={len}"
            );
            assert!(r.unix_mode.is_none() && r.unix_uid.is_none() && r.unix_gid.is_none());
            assert!(r.into_revision().is_err());
        }
    }

    #[tokio::test]
    async fn bounded_read_error_mapping_zero_byte_consistency() {
        // The InvalidRange (416) path for zero-byte objects matches the probe path:
        // both produce empty bytes, !truncated, no POSIX fields, unusable revision.
        let invalid_range =
            GetObjectError::generic(ErrorMetadata::builder().code("InvalidRange").build());
        let r = map_get_object_error(&invalid_range).unwrap();
        assert_eq!(r.bytes, Vec::<u8>::new());
        assert!(!r.truncated);
        assert!(r.unix_mode.is_none() && r.unix_uid.is_none() && r.unix_gid.is_none());
        assert!(r.into_revision().is_err());
    }
}
