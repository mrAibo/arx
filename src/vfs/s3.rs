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
pub struct S3ClientSettings {
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
// ponytail: single deterministic helper so the retry-invariant is observable
// and testable without performing real AWS calls
#[allow(dead_code)]
pub(crate) fn build_s3_config(
    settings: &S3ClientSettings,
    sdk_config: &aws_config::SdkConfig,
) -> aws_sdk_s3::Config {
    let mut builder = Builder::from(sdk_config);
    if let Some(region) = &settings.region {
        builder = builder.region(Region::new(region.clone()));
    }
    if let Some(profile) = &settings.profile {
        // profile already applied on the shared config loader; this is a no-op
        // guard documenting that profile is not an ARX-owned credential store
        let _ = profile;
    }
    if let Some(endpoint) = &settings.endpoint_url {
        builder = builder.endpoint_url(endpoint.clone());
    }
    builder = builder.force_path_style(settings.force_path_style);
    builder = builder.retry_config(RetryConfig::disabled());
    builder.build()
}

/// Construct an `aws_sdk_s3::Client` for exactly one `S3TargetConfig`.
///
/// Uses the official AWS SDK credential/config chain (no manual chain, no
/// static credentials). Creates NO requests and performs NO listing.
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
    fn retry_policy_disabled() {
        let target = mk_target(None, None, None, false);
        let settings = S3ClientSettings::from_target(&target);

        // Build a minimal SDK config in-process (no network) to verify the
        // service config is constructed with retries disabled.
        let sdk_config = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(aws_config::defaults(BehaviorVersion::latest()).load());
        let config = build_s3_config(&settings, &sdk_config);

        // RetryConfig::disabled() -> max_attempts == 1
        let retry = config.retry_config().expect("retry config must be present");
        assert_eq!(retry.max_attempts(), 1);
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
