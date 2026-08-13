//! S3/MinIO VfsProvider stub + AWS client factory (S3-16).
use crate::config::S3TargetConfig;
use crate::config::sanitize_diag;
use crate::vfs::{
    Entry, EntryIdentity, EntryKind, ListedEntry, Location, ProviderContinuation,
    ProviderListingPage, VfsOps, VfsProvider,
};
use aws_config::BehaviorVersion;
use aws_config::Region;
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{Builder, retry::RetryConfig};
use aws_sdk_s3::operation::list_buckets::ListBucketsOutput;
use aws_sdk_s3::operation::list_objects_v2::ListObjectsV2Output;
use std::borrow::Cow;
use std::io;

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
                // ListObjectsV2 (S3-20). Continuation consumption is S3-21; an
                // incoming token must fail closed BEFORE the client is built.
                if continuation.is_some() {
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "S3 ListObjectsV2 continuation not supported until S3-21",
                    ));
                }

                // S3-17 lazy per-target lifecycle: only this boundary builds the
                // client. Exactly one bounded ListObjectsV2 .send() per page.
                let client = self.client().await?;
                let wire_prefix = list_objects_wire_prefix(prefix);
                let output = list_objects_v2_page(client, bucket, &wire_prefix).await?;
                let page =
                    map_list_objects_v2_first_page(&self.target.id, bucket, &wire_prefix, &output)?;
                // First-page continuation truth: IsTruncated/NextContinuationToken.
                let continuation = next_list_objects_v2_continuation(
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

/// Bounded page size for every ListObjectsV2 request (first page in S3-20).
// ponytail: stays well under the 1000-key ceiling; S3-21 owns pagination.
const LIST_OBJECTS_PAGE_SIZE: i32 = 1000;

/// Construct the wire prefix for a ListObjectsV2 request.
///
/// This is protocol/navigation construction, NOT filesystem normalization:
/// - nav "" => wire "" (bucket root)
/// - nav ending "/" => preserve exactly
/// - nav non-empty without trailing "/" => append exactly one "/"
///   Never trim, collapse "//", resolve "."/"..", or canonicalize.
fn list_objects_wire_prefix(nav_prefix: &str) -> Cow<'_, str> {
    if nav_prefix.is_empty() {
        Cow::Borrowed("")
    } else if nav_prefix.ends_with('/') {
        Cow::Borrowed(nav_prefix)
    } else {
        Cow::Owned(format!("{nav_prefix}/"))
    }
}

/// One bounded ListObjectsV2 request for the first page.
/// Exactly one `.send()` per invocation. No loop, no paginator helper.
async fn list_objects_v2_page(
    client: &Client,
    bucket: &str,
    wire_prefix: &str,
) -> io::Result<ListObjectsV2Output> {
    let request = client
        .list_objects_v2()
        .bucket(bucket)
        .prefix(wire_prefix)
        .delimiter("/")
        .max_keys(LIST_OBJECTS_PAGE_SIZE);
    request
        .send()
        .await
        .map_err(|e| io::Error::other(format!("S3 ListObjectsV2 failed: {}", e)))
}

/// Pure first-page continuation protocol for ListObjectsV2.
/// ListObjectsV2 exposes IsTruncated + NextContinuationToken.
/// - IsTruncated == false => None
/// - IsTruncated == true AND usable NextContinuationToken => Some(ProviderContinuation)
/// - IsTruncated == true AND token missing/empty => InvalidData (ProtocolError)
/// - Missing IsTruncated => InvalidData
/// - IsTruncated == false BUT NextContinuationToken present => contradictory InvalidData
fn next_list_objects_v2_continuation(
    is_truncated: Option<bool>,
    next_token: Option<&str>,
) -> io::Result<Option<ProviderContinuation>> {
    match is_truncated {
        Some(false) => Ok(None),
        Some(true) => match next_token {
            Some(token) if !token.is_empty() => Ok(Some(ProviderContinuation {
                token: token.to_string(),
            })),
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

/// Pure AWS-response → provider-page mapping for ListObjectsV2 first page.
/// Includes folder-marker dedup by exact evidence only.
fn map_list_objects_v2_first_page(
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
    use aws_sdk_s3::types::Bucket;

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

    #[tokio::test]
    async fn bucket_bound_list_page_initializes_client() {
        // S3-20 implements ListObjectsV2 for exact bound bucket.
        let p = S3Provider::new(bound_target("company-artifacts"));
        let l = Location::S3 {
            target: "t".to_string(),
            bucket: Some("company-artifacts".to_string()),
            prefix: String::new(),
        };
        // ListObjectsV2 is now implemented (S3-20); client initializes.
        // Without AWS creds it fails, but NOT with Unsupported.
        let err = p.list_page(&l, None).await.unwrap_err();
        assert!(
            !matches!(err.kind(), io::ErrorKind::Unsupported),
            "Exact bound bucket should route to ListObjectsV2, not return Unsupported"
        );
        // Client should have been initialized (lazy init happened).
        assert!(p.client.get().is_some());
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
}
