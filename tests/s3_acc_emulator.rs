//! AWS-emulator physical acceptance (S3-62E) against Moto/LocalStack.
//!
//! Gated behind ARX_EMULATOR_TEST=1 (Moto server at ARX_EMULATOR_ENDPOINT,
//! default http://localhost:5000). Exercises the SAME production S3Provider
//! Classify results EMULATED PASS — this is an AWS-shaped emulator,
//! NOT a substitute for real-AWS physical acceptance (AWS S3-62A passed
//! physical acceptance in v0.17.0; S3-62E remains emulator-scoped).

mod s3_acceptance;

use arx::transfer::executor::execute_transfer;
use arx::transfer::{S3TransferSpec, TransferIntent, TransferMethod, TransferPlan};
use arx::vfs::{
    ListedEntry, Location, ProviderContinuation, ProviderListingPage, ProviderRegistry, S3ObjectRef,
};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

fn emu_root() -> Location {
    s3_acceptance::bucket_root("emulator", "arxtest")
}

fn scoped(run: &str, sub: &str) -> Location {
    let p = if sub.is_empty() {
        format!("arx-acceptance/{run}")
    } else {
        format!("arx-acceptance/{run}/{sub}")
    };
    Location::S3 {
        target: "emulator".to_string(),
        bucket: Some("arxtest".to_string()),
        prefix: p,
    }
}

async fn list_all(registry: &ProviderRegistry, loc: &Location) -> Vec<ListedEntry> {
    let mut out = Vec::new();
    let mut cont: Option<ProviderContinuation> = None;
    loop {
        let page: ProviderListingPage = registry
            .list_page(loc, cont.as_ref())
            .await
            .expect("list_page");
        out.extend(page.entries);
        match page.continuation {
            Some(c) if !c.token.is_empty() => cont = Some(c),
            _ => break,
        }
    }
    out
}

async fn upload_bytes(registry: &ProviderRegistry, key: &str, data: &[u8]) {
    let tmp = std::env::temp_dir().join(format!("arx-acc-{}-{}", std::process::id(), hexify(key)));
    std::fs::write(&tmp, data).expect("write temp fixture");
    let spec = S3TransferSpec::UploadOne {
        local_source: tmp.clone(),
        destination: S3ObjectRef {
            target: "emulator".to_string(),
            bucket: "arxtest".to_string(),
            key: key.to_string(),
        },
    };
    let plan = TransferPlan {
        source: Location::Local(std::env::temp_dir()),
        destination: emu_root(),
        intent: TransferIntent::Copy,
        method: TransferMethod::S3,
        s3_spec: Some(spec),
        webdav_spec: None,
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let outcome = execute_transfer(
        &plan,
        &[key.to_string()],
        registry,
        cancel,
        arx::transfer_queue::PauseGate::disabled(),
        |_| {},
    )
    .await
    .expect("upload via transfer");
    assert_eq!(outcome.completed, 1, "exactly one object uploaded");
    let _ = std::fs::remove_file(&tmp);
}

async fn download_bytes(registry: &ProviderRegistry, key: &str) -> Vec<u8> {
    let tmp =
        std::env::temp_dir().join(format!("arx-acc-dl-{}-{}", std::process::id(), hexify(key)));
    let spec = S3TransferSpec::DownloadOne {
        source: S3ObjectRef {
            target: "emulator".to_string(),
            bucket: "arxtest".to_string(),
            key: key.to_string(),
        },
        local_destination: tmp.clone(),
    };
    let plan = TransferPlan {
        source: emu_root(),
        destination: Location::Local(std::env::temp_dir()),
        intent: TransferIntent::Copy,
        method: TransferMethod::S3,
        s3_spec: Some(spec),
        webdav_spec: None,
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let outcome = execute_transfer(
        &plan,
        &[key.to_string()],
        registry,
        cancel,
        arx::transfer_queue::PauseGate::disabled(),
        |_| {},
    )
    .await
    .expect("download via transfer");
    assert_eq!(outcome.completed, 1, "exactly one object downloaded");
    let data = std::fs::read(&tmp).expect("read downloaded fixture");
    let _ = std::fs::remove_file(&tmp);
    data
}

fn hexify(s: &str) -> String {
    s.bytes().map(|b| format!("{:02x}", b)).collect::<String>()
}

#[tokio::test]
async fn emulator_connect_and_bucket_bound() {
    let Some(reg) = s3_acceptance::maybe_skip_emulator() else {
        return;
    };
    let _page = reg
        .list_page(&emu_root(), None)
        .await
        .expect("bucket root list");
}

#[tokio::test]
async fn emulator_prefix_navigation() {
    let Some(reg) = s3_acceptance::maybe_skip_emulator() else {
        return;
    };
    let run = s3_acceptance::run_id();
    let child_key = format!("arx-acceptance/{run}/prefix-a/file.txt");
    upload_bytes(&reg, &child_key, b"hello emulator").await;
    let parent = list_all(&reg, &scoped(&run, "")).await;
    assert!(
        parent.iter().any(|e| e.entry.name == "prefix-a"),
        "nested prefix visible as folder"
    );
    let sub = list_all(&reg, &scoped(&run, "prefix-a")).await;
    assert!(
        sub.iter().any(|e| e.entry.name == "file.txt"),
        "child visible under exact prefix"
    );
    reg.delete_s3_at(&emu_root(), &child_key)
        .await
        .expect("cleanup child");
}

#[tokio::test]
async fn emulator_unicode_identity_and_bytes() {
    let Some(reg) = s3_acceptance::maybe_skip_emulator() else {
        return;
    };
    let run = s3_acceptance::run_id();
    let key = format!("arx-acceptance/{run}/日本語/каталог/🧙‍♂️.txt");
    let payload = s3_acceptance::deterministic_bytes(0xC0FFEE, 256);
    upload_bytes(&reg, &key, &payload).await;
    let listed = list_all(&reg, &scoped(&run, "")).await;
    assert!(
        listed.iter().any(|e| e.entry.name.contains("日本語")),
        "unicode prefix listed with exact identity"
    );
    let got = download_bytes(&reg, &key).await;
    assert!(
        s3_acceptance::byte_eq(&got, &payload),
        "unicode object byte-exact"
    );
    reg.delete_s3_at(&emu_root(), &key)
        .await
        .expect("cleanup unicode");
}

#[tokio::test]
async fn emulator_zero_byte_and_folder_marker() {
    let Some(reg) = s3_acceptance::maybe_skip_emulator() else {
        return;
    };
    let run = s3_acceptance::run_id();
    let zb = format!("arx-acceptance/{run}/zero.bin");
    upload_bytes(&reg, &zb, &[]).await;
    let listed = list_all(&reg, &scoped(&run, "")).await;
    let z = listed
        .iter()
        .find(|e| e.entry.name == "zero.bin")
        .expect("zero-byte object listed");
    assert_eq!(
        z.entry.size,
        Some(0),
        "S3ObjectRef size=0, not mistaken for prefix"
    );
    let marker_name = format!("{run}-folder");
    reg.create_s3_prefix_marker_at(&scoped(&run, ""), &marker_name)
        .await
        .expect("create marker");
    let marker_key = format!("arx-acceptance/{run}/{marker_name}/");
    let marker_loc = scoped(&run, "");
    assert!(
        reg.prove_empty_s3_prefix_at(&marker_loc, &marker_key)
            .await
            .expect("prove"),
        "fresh marker is empty"
    );
    let sub = list_all(&reg, &marker_loc).await;
    assert!(
        sub.iter().any(|e| e.entry.name == marker_name),
        "empty marker visible as prefix"
    );
    reg.delete_s3_at(&marker_loc, &marker_key)
        .await
        .expect("delete marker");
    assert!(
        !reg.prove_empty_s3_prefix_at(&marker_loc, &marker_key)
            .await
            .expect("prove after"),
        "marker gone"
    );
    reg.delete_s3_at(&emu_root(), &zb)
        .await
        .expect("cleanup zero-byte");
}

#[tokio::test]
async fn emulator_incremental_pagination() {
    let Some(reg) = s3_acceptance::maybe_skip_emulator() else {
        return;
    };
    let run = s3_acceptance::run_id();
    let count = 1005u32;
    for i in 0..count {
        let key = format!("arx-acceptance/{run}/item-{i:04}");
        upload_bytes(&reg, &key, &[i as u8]).await;
    }
    let listed = list_all(&reg, &scoped(&run, "")).await;
    let item_names: Vec<&str> = listed
        .iter()
        .filter(|e| e.entry.name.starts_with("item-"))
        .map(|e| e.entry.name.as_str())
        .collect();
    assert_eq!(item_names.len() as u32, count, "no missing, exact count");
    let mut sorted = item_names.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len() as u32, count, "no duplicate identity");
    for i in 0..count {
        let key = format!("arx-acceptance/{run}/item-{i:04}");
        reg.delete_s3_at(&emu_root(), &key).await.expect("cleanup");
    }
}

#[tokio::test]
async fn emulator_small_upload_download_roundtrip() {
    let Some(reg) = s3_acceptance::maybe_skip_emulator() else {
        return;
    };
    let run = s3_acceptance::run_id();
    let key = format!("arx-acceptance/{run}/roundtrip.bin");
    let payload = s3_acceptance::deterministic_bytes(0x1234, 4096);
    upload_bytes(&reg, &key, &payload).await;
    let got = download_bytes(&reg, &key).await;
    assert!(
        s3_acceptance::byte_eq(&got, &payload),
        "byte-exact roundtrip"
    );
    reg.delete_s3_at(&emu_root(), &key).await.expect("cleanup");
}
