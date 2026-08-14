//! Shared physical-acceptance harness for ARX S3 provider/runtime paths.
//!
//! Gated by opt-in env vars so default CI never touches network/creds:
//!   ARX_EMULATOR_TEST=1  -> AWS-emulated endpoint (Moto :5000 / LocalStack)
//!   ARX_MINIO_TEST=1      -> disposable MinIO (arx-minio-test :9000)
//!
//! Results are classified factually: PASS / FAIL / NOT_RUN(reason). We never
//! convert NOT_RUN into PASS, and we never weaken assertions to make a case pass.

use arx::config::S3TargetConfig;
use arx::vfs::{Location, ProviderRegistry};

/// Opt-in gate. Returns the registry only when the env var is set; otherwise
/// the caller should `return` (test recorded as skipped, not passed).
pub fn maybe_skip_emulator() -> Option<ProviderRegistry> {
    if std::env::var("ARX_EMULATOR_TEST").is_err() {
        eprintln!("ARX_EMULATOR_TEST not set; skipping AWS-emulator acceptance");
        return None;
    }
    let endpoint = std::env::var("ARX_EMULATOR_ENDPOINT")
        .unwrap_or_else(|_| "http://localhost:5000".to_string());
    let registry = ProviderRegistry::new();
    registry.register_s3_targets(&[S3TargetConfig {
        id: "emulator".to_string(),
        name: "emulator".to_string(),
        bucket: Some("arxtest".to_string()),
        region: Some("us-east-1".to_string()),
        profile: None,
        endpoint_url: Some(endpoint),
        force_path_style: true,
    }]);
    Some(registry)
}

pub fn maybe_skip_minio() -> Option<ProviderRegistry> {
    if std::env::var("ARX_MINIO_TEST").is_err() {
        eprintln!("ARX_MINIO_TEST not set; skipping MinIO acceptance");
        return None;
    }
    let endpoint = std::env::var("AWS_ENDPOINT_URL")
        .unwrap_or_else(|_| "http://localhost:9000".to_string());
    let registry = ProviderRegistry::new();
    registry.register_s3_targets(&[S3TargetConfig {
        id: "minio".to_string(),
        name: "minio".to_string(),
        bucket: Some("arxtest".to_string()),
        region: Some("us-east-1".to_string()),
        profile: None,
        endpoint_url: Some(endpoint),
        force_path_style: true,
    }]);
    Some(registry)
}

/// Bucket-root location for the configured target.
pub fn bucket_root(target: &str, bucket: &str) -> Location {
    Location::S3 {
        target: target.to_string(),
        bucket: Some(bucket.to_string()),
        prefix: "".to_string(),
    }
}

/// Unique per-process run id so parallel/again runs never collide on fixtures.
pub fn run_id() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    )
}

/// Disposable fixture prefix: `arx-acceptance/<run-id>/`.
pub fn disposable_prefix(run: &str) -> String {
    format!("arx-acceptance/{run}/")
}

/// Deterministic pseudo-random bytes (xorshift) for reproducible fixtures.
pub fn deterministic_bytes(seed: u64, len: usize) -> Vec<u8> {
    let mut state = seed ^ 0x9E3779B97F4A7C15;
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.push((state & 0xFF) as u8);
    }
    out
}

/// Streaming-equivalent comparison (we already hold both in memory for tests).
pub fn byte_eq(a: &[u8], b: &[u8]) -> bool {
    a == b
}

/// Sanitization probe: fail if a string leaks credential-shaped material.
/// We assert the ABSENCE of secrets in any diagnostic we would print.
pub fn assert_no_secret_leak(s: &str) {
    let forbidden = [
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
        "X-Amz-Signature",
        "Authorization:",
        "x-amz-security-token",
    ];
    for f in forbidden {
        assert!(
            !s.contains(f),
            "diagnostic leaks forbidden token fragment: {f}"
        );
    }
    assert!(
        !s.contains("X-Amz-Algorithm"),
        "diagnostic leaks signed-URL query"
    );
}

/// Run a future under a timeout guard so a stalled endpoint cannot hang CI.
#[cfg(feature = "tokio")]
pub async fn with_timeout<F, T>(dur: std::time::Duration, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    match tokio::time::timeout(dur, fut).await {
        Ok(v) => v,
        Err(_) => panic!("acceptance operation timed out after {dur:?}"),
    }
}
