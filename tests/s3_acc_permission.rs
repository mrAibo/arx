//! S3-65E / S3-65 permission-matrix acceptance (emulator, Moto).
//!
//! Moto's IAM/STS is NOT full AWS IAM, so the full session-token + fine-grained
//! permission matrix stays PARKED_ENV (S3-65A). Here we verify the ARX-side
//! fail-closed guards that DO NOT depend on provider IAM:
//! bucket-escape rejection (target bound to bucket X cannot touch bucket Y),
//! and unknown/empty bucket list is rejected, never widens to ListAllMyBuckets.
//! These run against Moto (:5000).

mod s3_acceptance;

use arx::vfs::Location;

#[tokio::test]
async fn emulator_permission_fail_closed() {
    let Some(reg) = s3_acceptance::maybe_skip_emulator() else {
        return;
    };
    // Bucket-bound target "emulator" is bound to bucket "arxtest" (config).
    // A request scoped to a DIFFERENT bucket must be rejected fail-closed,
    // never silently widened or escaped.
    let wrong_bucket = Location::S3 {
        target: "emulator".to_string(),
        bucket: Some("nonexistent-bucket-xyz".to_string()),
        prefix: String::new(),
    };
    let res = reg.list_page(&wrong_bucket, None).await;
    assert!(
        res.is_err(),
        "wrong/unknown bucket list rejected fail-closed (no escape)"
    );

    // Target root (bucket = None) for a bucket-bound target must NOT trigger
    // ListAllMyBuckets; it must be rejected (bucket creation/list-all out of scope).
    let root = Location::S3 {
        target: "emulator".to_string(),
        bucket: None,
        prefix: String::new(),
    };
    let res_root = reg.list_page(&root, None).await;
    assert!(
        res_root.is_err(),
        "target-root ListBuckets rejected for bucket-bound target"
    );
}
