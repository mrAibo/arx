//! Physical acceptance for S3-55 Phase 8/10 exact-delete against disposable MinIO.
//!
//! Gated behind ARX_MINIO_TEST=1 so default CI never touches network/creds.
//! Run locally with the disposable `arx-minio-test` container:
//!
//!   ARX_MINIO_TEST=1 \
//!   AWS_ACCESS_KEY_ID=minioadmin AWS_SECRET_ACCESS_KEY=minioadmin \
//!   AWS_ENDPOINT_URL=http://localhost:9000 \
//!   cargo test --test s3_delete_minio -- --nocapture
//!
//! Covers: create empty marker -> prove_empty true -> exact delete -> marker gone.

use arx::config::S3TargetConfig;
use arx::vfs::{Location, ProviderRegistry};

fn maybe_skip() -> Option<ProviderRegistry> {
    if std::env::var("ARX_MINIO_TEST").is_err() {
        eprintln!("ARX_MINIO_TEST not set; skipping physical MinIO test");
        return None;
    }
    let endpoint =
        std::env::var("AWS_ENDPOINT_URL").unwrap_or_else(|_| "http://localhost:9000".to_string());

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

fn bucket_root() -> Location {
    Location::S3 {
        target: "minio".to_string(),
        bucket: Some("arxtest".to_string()),
        prefix: "".to_string(),
    }
}

#[tokio::test]
async fn physical_s3_exact_delete_marker_against_minio() {
    let Some(registry) = maybe_skip() else {
        return;
    };
    let uniq = format!("del-folder-{}", std::process::id());
    let marker = format!("{uniq}/");
    let root = bucket_root();

    // 1. create empty prefix marker
    let created = registry
        .create_s3_prefix_marker_at(&root, &uniq)
        .await
        .expect("create empty marker");
    assert_eq!(created.prefix, marker, "marker key must be '<uniq>/'");

    // 2. prove_empty sees the marker
    let is_empty_before = registry
        .prove_empty_s3_prefix_at(&root, &marker)
        .await
        .expect("prove_empty before delete");
    assert!(is_empty_before, "freshly created marker must prove empty");

    // 3. exact delete (ONE DeleteObject)
    registry
        .delete_s3_at(&root, &marker)
        .await
        .expect("exact delete of marker");

    // 4. marker is gone: prove_empty now false (single ListObjectsV2, the
    //    bounded proof — NOT the unimplemented list_location_async descent).
    let is_empty_after = registry
        .prove_empty_s3_prefix_at(&root, &marker)
        .await
        .expect("prove_empty after delete");
    assert!(
        !is_empty_after,
        "marker must be gone after exact delete (prove_empty false)"
    );
}
