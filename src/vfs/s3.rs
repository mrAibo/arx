//! S3/MinIO VfsProvider stub + AWS client factory (S3-16).
use crate::config::S3TargetConfig;
use crate::vfs::{Entry, VfsOps, VfsProvider};
use aws_config::BehaviorVersion;
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{Builder, retry::RetryConfig};
use std::io;
// ponytail: reqwest already a dependency; use its Url for endpoint validation
// (no new dep). aws_sdk_s3 re-exports its own Region via aws_types.
use aws_config::Region;

pub struct S3Fs;
#[derive(Debug)]
pub struct S3Provider;

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
}
