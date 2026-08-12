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
    // ponytail: called by S3-18+ operations; intentionally unused in S3-17.
    #[allow(dead_code)]
    pub(crate) async fn client(&self) -> io::Result<&aws_sdk_s3::Client> {
        self.client
            .get_or_try_init(|| client_for_target(&self.target))
            .await
            .map_err(|e| io::Error::new(e.kind(), e.to_string()))
    }
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
        // S3-19 owns continuation consumption / next-page fetching.
        if continuation.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "S3 ListBuckets continuation not supported until S3-19",
            ));
        }

        // Must be exactly this provider's S3 target root (account/target root).
        let (target, bucket) = match location {
            Location::S3 { target, bucket, .. } => (target, bucket),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "S3Provider::list_page requires Location::S3",
                ));
            }
        };
        if target != &self.target.id {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("S3 target mismatch: {}", sanitize_diag(target)),
            ));
        }
        // Bucket-bound listing is ListObjectsV2 — out of scope (S3-20).
        if bucket.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "S3 bucket object listing requires S3-20 (ListObjectsV2)",
            ));
        }

        // S3-17 lazy per-target lifecycle: only this boundary builds the client.
        let client = self.client().await?;
        let output = list_buckets_first_page(client).await?;
        map_list_buckets_first_page(&self.target.id, &output)
    }
}

/// Bounded page size for the first (and, in S3-18, only) ListBuckets request.
// ponytail: stays well under the 10k quota ceiling; S3-19 owns pagination.
const LIST_BUCKETS_PAGE_SIZE: i32 = 1000;

/// One bounded, unpaginated ListBuckets request. The only `.send()` permitted
/// in S3-18 production code. No continuation token (S3-19 consumes those).
async fn list_buckets_first_page(client: &Client) -> io::Result<ListBucketsOutput> {
    client
        .list_buckets()
        .max_buckets(LIST_BUCKETS_PAGE_SIZE)
        .send()
        .await
        .map_err(|e| io::Error::other(format!("S3 ListBuckets failed: {}", e)))
}

/// Pure AWS-response → provider-page mapping. No network, no SDK config dump.
/// Every usable bucket name becomes exactly one `ListedEntry` with an exact
/// `S3BucketRef` identity. Skips name-less records; never invents one.
fn map_list_buckets_first_page(
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

    let continuation = output.continuation_token().map(|t| ProviderContinuation {
        token: t.to_string(),
    });
    Ok(ProviderListingPage {
        entries,
        continuation,
    })
}

// Old VfsOps stub kept for compat
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
        let page = map_list_buckets_first_page("aws-prod", &out).unwrap();
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
        let page = map_list_buckets_first_page("t", &out).unwrap();
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
        let page = map_list_buckets_first_page("t", &out).unwrap();
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
        let page = map_list_buckets_first_page("t", &out).unwrap();
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
        let page = map_list_buckets_first_page("t", &out).unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].entry.name, "real-bucket");
    }

    #[test]
    fn map_continuation_token_preserved_verbatim() {
        let out = ListBucketsOutput::builder()
            .buckets(bucket_named("a"))
            .continuation_token("  opaque+/=token 日本語  ")
            .build();
        let page = map_list_buckets_first_page("t", &out).unwrap();
        assert_eq!(
            page.continuation.as_ref().map(|c| c.token.as_str()),
            Some("  opaque+/=token 日本語  ")
        );
    }

    #[test]
    fn map_target_id_copied_everywhere() {
        let out = ListBucketsOutput::builder()
            .buckets(bucket_named("only"))
            .build();
        let page = map_list_buckets_first_page("exact-target-id", &out).unwrap();
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
        let page = map_list_buckets_first_page("t", &out).unwrap();
        assert!(page.continuation.is_none());
    }
}
