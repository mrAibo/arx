//! Physical #264 acceptance against disposable MinIO.
//!
//! Fixture setup may use the existing transfer path, while every assertion of
//! inspector behavior goes through the production S3 Inspector core and the
//! same concrete S3Provider/client used by listing and transfers.

mod s3_acceptance;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use arx::s3_inspector::{
    S3EvidenceSource, S3InspectionScope, S3ScanOutcome, inspect_object, scan_scope,
};
use arx::transfer::executor::execute_transfer;
use arx::transfer::{S3TransferSpec, TransferIntent, TransferMethod, TransferPlan};
use arx::vfs::s3::{S3ObjectRef, S3PrefixRef};
use arx::vfs::{Location, ProviderRegistry};

fn minio_root() -> Location {
    s3_acceptance::bucket_root("minio", "arxtest")
}

async fn upload_bytes(registry: &ProviderRegistry, key: &str, data: &[u8]) {
    let tmp = std::env::temp_dir().join(format!(
        "arx-inspector-{}-{}",
        std::process::id(),
        key.bytes()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    ));
    std::fs::write(&tmp, data).expect("write inspector fixture");
    let plan = TransferPlan {
        source: Location::Local(std::env::temp_dir()),
        destination: minio_root(),
        intent: TransferIntent::Copy,
        method: TransferMethod::S3,
        archive_spec: None,
        s3_spec: Some(S3TransferSpec::UploadOne {
            local_source: tmp.clone(),
            destination: S3ObjectRef {
                target: "minio".into(),
                bucket: "arxtest".into(),
                key: key.into(),
            },
        }),
        webdav_spec: None,
    };
    let outcome = execute_transfer(
        &plan,
        &[key.to_string()],
        registry,
        Arc::new(AtomicBool::new(false)),
        arx::transfer_queue::PauseGate::disabled(),
        |_| {},
    )
    .await
    .expect("upload inspector fixture through production transfer path");
    assert_eq!(outcome.completed, 1);
    let _ = std::fs::remove_file(tmp);
}

#[tokio::test]
async fn minio_object_and_prefix_inspector_are_live_bounded_and_exact() {
    let Some(registry) = s3_acceptance::maybe_skip_minio() else {
        return;
    };
    let run = s3_acceptance::run_id();
    let root = format!("arx-acceptance/{run}/inspector/");
    let key_a = format!("{root}a.bin");
    let key_b = format!("{root}child/b.bin");
    let key_c = format!("{root}child/c.bin");
    upload_bytes(&registry, &key_a, b"abc").await;
    upload_bytes(&registry, &key_b, b"12345").await;
    upload_bytes(&registry, &key_c, b"1234567").await;

    let provider = registry
        .s3_provider_for_transfer("minio")
        .expect("same concrete MinIO provider");
    let object = inspect_object(
        Arc::clone(&provider),
        S3ObjectRef {
            target: "minio".into(),
            bucket: "arxtest".into(),
            key: key_c.clone(),
        },
        Arc::new(AtomicBool::new(false)),
    )
    .await
    .expect("HeadObject inspector");
    assert_eq!(object.evidence, S3EvidenceSource::LiveScan);
    assert_eq!(object.target, "minio");
    assert_eq!(object.bucket, "arxtest");
    assert_eq!(object.key, key_c);
    assert_eq!(object.size, Some(7));
    assert!(object.etag.is_some(), "MinIO must prove an ETag");

    let mut progress = Vec::new();
    let outcome = scan_scope(
        provider,
        S3InspectionScope::Prefix(S3PrefixRef {
            target: "minio".into(),
            bucket: "arxtest".into(),
            prefix: root.clone(),
        }),
        Arc::new(AtomicBool::new(false)),
        |update| progress.push(update),
    )
    .await
    .expect("prefix LiveScan");
    let S3ScanOutcome::Complete(scan) = outcome else {
        panic!("physical MinIO scan must complete");
    };
    assert_eq!(scan.evidence, S3EvidenceSource::LiveScan);
    assert!(scan.complete);
    assert!(!scan.cancelled);
    assert_eq!(scan.object_count, 3);
    assert_eq!(scan.total_logical_bytes, 15);
    assert_eq!(scan.objects_without_size, 0);
    assert_eq!(scan.largest_objects[0].size, 7);
    assert_eq!(scan.largest_objects[0].key, key_c);
    assert_eq!(scan.largest_prefixes.source, S3EvidenceSource::LiveScan);
    let prefixes = scan
        .largest_prefixes
        .value
        .expect("bounded prefix analytics");
    assert!(
        prefixes.iter().any(|prefix| {
            prefix.prefix == format!("{root}child/") && prefix.logical_bytes == 12
        })
    );
    assert!(!progress.is_empty());
    assert_eq!(progress.last().unwrap().objects_seen, 3);
    assert_eq!(progress.last().unwrap().logical_bytes_seen, 15);

    let cancelled = scan_scope(
        registry
            .s3_provider_for_transfer("minio")
            .expect("same provider for cancellation"),
        S3InspectionScope::Prefix(S3PrefixRef {
            target: "minio".into(),
            bucket: "arxtest".into(),
            prefix: root,
        }),
        Arc::new(AtomicBool::new(true)),
        |_| {},
    )
    .await
    .expect("pre-page cancellation is a typed outcome");
    let S3ScanOutcome::Cancelled(cancelled) = cancelled else {
        panic!("pre-page cancellation must not become failure");
    };
    assert_eq!(cancelled.pages_seen, 0);
    assert_eq!(cancelled.object_count, 0);

    for key in [&key_a, &key_b, &key_c] {
        registry
            .delete_s3_at(&minio_root(), key)
            .await
            .expect("cleanup inspector fixture");
    }
}
