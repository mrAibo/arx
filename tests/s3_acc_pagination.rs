//! S3-63E / S3-63 pagination acceptance (emulator + MinIO).
//!
//! Verifies real continuation behavior: page1 bounded, continuation present,
//! page2 accepted, continuation cleared truthfully, no dup/missing, one
//! next-page request at a time (no eager all-pages enumeration).
//!
//! Gated: ARX_EMULATOR_TEST=1 (Moto :5000) and/or ARX_MINIO_TEST=1.

mod s3_acceptance;

use arx::transfer::executor::execute_transfer;
use arx::transfer::{S3TransferSpec, TransferIntent, TransferMethod, TransferPlan};
use arx::vfs::{
    Location, ProviderContinuation, ProviderListingPage, ProviderRegistry, S3ObjectRef,
};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

const COUNT: u32 = 1005;

fn root(target: &str) -> Location {
    s3_acceptance::bucket_root(target, "arxtest")
}

fn scoped(run: &str, target: &str) -> Location {
    Location::S3 {
        target: target.to_string(),
        bucket: Some("arxtest".to_string()),
        prefix: format!("arx-acceptance/{run}"),
    }
}

async fn upload(reg: &ProviderRegistry, target: &str, key: &str, data: &[u8]) {
    let tmp = std::env::temp_dir().join(format!("arx-pg-{}-{}", std::process::id(), hex(key)));
    std::fs::write(&tmp, data).unwrap();
    let plan = TransferPlan {
        source: Location::Local(std::env::temp_dir()),
        destination: root(target),
        intent: TransferIntent::Copy,
        method: TransferMethod::S3,
        archive_spec: None,
        s3_spec: Some(S3TransferSpec::UploadOne {
            local_source: tmp.clone(),
            destination: S3ObjectRef {
                target: target.to_string(),
                bucket: "arxtest".to_string(),
                key: key.to_string(),
            },
        }),
        webdav_spec: None,
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let out = execute_transfer(
        &plan,
        &[key.to_string()],
        reg,
        cancel,
        arx::transfer_queue::PauseGate::disabled(),
        |_| {},
    )
    .await
    .expect("upload");
    assert_eq!(out.completed, 1);
    let _ = std::fs::remove_file(&tmp);
}

async fn delete_all(reg: &ProviderRegistry, target: &str, run: &str) {
    for i in 0..COUNT {
        let key = format!("arx-acceptance/{run}/item-{i:04}");
        reg.delete_s3_at(&root(target), &key)
            .await
            .expect("cleanup");
    }
}

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{:02x}", b)).collect()
}

async fn run_pagination(reg: &ProviderRegistry, target: &str) {
    let run = s3_acceptance::run_id();
    for i in 0..COUNT {
        let key = format!("arx-acceptance/{run}/item-{i:04}");
        upload(reg, target, &key, &[i as u8]).await;
    }
    // Explicit two-page walk; assert bounded first page + truthful continuation.
    let page1: ProviderListingPage = reg
        .list_page(&scoped(&run, target), None)
        .await
        .expect("page1");
    let names1: Vec<String> = page1.entries.iter().map(|e| e.entry.name.clone()).collect();
    assert!(names1.len() < COUNT as usize, "page1 is bounded (< total)");
    assert!(!names1.is_empty(), "page1 non-empty");
    let cont1 = page1
        .continuation
        .expect("continuation present after page1");
    assert!(!cont1.token.is_empty(), "continuation token non-empty");
    // one next-page request
    let page2: ProviderListingPage = reg
        .list_page(
            &scoped(&run, target),
            Some(&ProviderContinuation {
                token: cont1.token.clone(),
            }),
        )
        .await
        .expect("page2");
    let names2: Vec<String> = page2.entries.iter().map(|e| e.entry.name.clone()).collect();
    assert!(!names2.is_empty(), "page2 non-empty");
    // continuation cleared truthfully (no third page)
    assert!(
        page2.continuation.is_none() || page2.continuation.as_ref().unwrap().token.is_empty(),
        "continuation cleared/empty after final page"
    );
    // combine + verify no dup / no missing
    let mut all = names1.clone();
    all.extend(names2);
    let items: Vec<&String> = all.iter().filter(|n| n.starts_with("item-")).collect();
    assert_eq!(
        items.len() as u32,
        COUNT,
        "no missing, exact count across pages"
    );
    let mut sorted = items.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len() as u32, COUNT, "no duplicate identity");
    delete_all(reg, target, &run).await;
}

#[tokio::test]
async fn emulator_pagination() {
    let Some(reg) = s3_acceptance::maybe_skip_emulator() else {
        return;
    };
    run_pagination(&reg, "emulator").await;
}

#[tokio::test]
async fn minio_pagination() {
    let Some(reg) = s3_acceptance::maybe_skip_minio() else {
        return;
    };
    run_pagination(&reg, "minio").await;
}
