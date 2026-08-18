//! Complete real AWS S3 physical acceptance (S3-62A..65A / S3-68 target).
//!
//! Gated behind ARX_AWS_ACCEPTANCE=1. Runs against a DISPOSABLE bucket you
//! created with temporary STS credentials. Uses the SAME production
//! S3Provider/runtime as MinIO/Moto — never an AWS-special executor.
//!
//! Classification: PHYSICAL PASS for real AWS. This gate confirmed
//! ARX_S3 as SUPPORTED in v0.17.0 (immutable SHA b5f0ee6, 20/20 physical).
//!
//! Covers (per DESIGN_S3 acceptance matrix):
//!   S3-62  basic operations + >1000 pagination
//!   S3-64  real multipart + cancel/abort/no-orphan
//!   S3-65  session-token SDK chain, no-ListBuckets least-privilege, controlled
//!          denial matrix, wrong region, missing bucket, object-disappears
//!
//! When ARX_AWS_ACCEPTANCE=1 is NOT set, every test returns early. Cargo records
//! these as PASSED tests (CARGO_RESULT = PASS), but their PHYSICAL_CLASSIFICATION
//! is NOT_RUN — they never touched a real bucket. Do not read a skipped early-return
//! as physical evidence.

mod s3_acceptance;

use arx::transfer::executor::{TransferProgress, execute_transfer};
use arx::transfer::{S3TransferSpec, TransferIntent, TransferMethod, TransferPlan};
use arx::vfs::{
    ListedEntry, Location, ProviderContinuation, ProviderListingPage, ProviderRegistry, S3ObjectRef,
};
use aws_config::Region;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

fn region_for_test() -> String {
    std::env::var("ARX_AWS_REGION")
        .ok()
        .filter(|r| !r.trim().is_empty())
        .unwrap_or_else(|| "us-east-1".to_string())
}

fn aws_bucket() -> String {
    std::env::var("ARX_AWS_BUCKET").unwrap_or_else(|_| "arx-acceptance".to_string())
}
fn aws_root() -> Location {
    Location::S3 {
        target: "aws-bucket".to_string(),
        bucket: Some(aws_bucket()),
        prefix: "".to_string(),
    }
}
fn scoped(run: &str, sub: &str) -> Location {
    let p = if sub.is_empty() {
        format!("arx-acceptance/{run}")
    } else {
        format!("arx-acceptance/{run}/{sub}")
    };
    Location::S3 {
        target: "aws-bucket".to_string(),
        bucket: Some(aws_bucket()),
        prefix: p,
    }
}
fn hexify(s: &str) -> String {
    s.bytes().map(|b| format!("{:02x}", b)).collect::<String>()
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
    let tmp = std::env::temp_dir().join(format!(
        "arx-acc-{}-{}.bin",
        std::process::id(),
        hexify(key)
    ));
    std::fs::write(&tmp, data).expect("write temp fixture");
    let spec = S3TransferSpec::UploadOne {
        local_source: tmp.clone(),
        destination: S3ObjectRef {
            target: "aws".to_string(),
            bucket: aws_bucket(),
            key: key.to_string(),
        },
    };
    let plan = TransferPlan {
        source: Location::Local(std::env::temp_dir()),
        destination: aws_root(),
        intent: TransferIntent::Copy,
        method: TransferMethod::S3,
        s3_spec: Some(spec),
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let outcome = execute_transfer(&plan, &[key.to_string()], registry, cancel, |_| {})
        .await
        .expect("upload");
    assert_eq!(outcome.completed, 1, "exactly one object uploaded");
    let _ = std::fs::remove_file(&tmp);
}
async fn download_bytes(registry: &ProviderRegistry, key: &str) -> Vec<u8> {
    let tmp = std::env::temp_dir().join(format!(
        "arx-acc-dl-{}-{}.bin",
        std::process::id(),
        hexify(key)
    ));
    let spec = S3TransferSpec::DownloadOne {
        source: S3ObjectRef {
            target: "aws".to_string(),
            bucket: aws_bucket(),
            key: key.to_string(),
        },
        local_destination: tmp.clone(),
    };
    let plan = TransferPlan {
        source: aws_root(),
        destination: Location::Local(std::env::temp_dir()),
        intent: TransferIntent::Copy,
        method: TransferMethod::S3,
        s3_spec: Some(spec),
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let outcome = execute_transfer(&plan, &[key.to_string()], registry, cancel, |_| {})
        .await
        .expect("download");
    assert_eq!(outcome.completed, 1, "exactly one object downloaded");
    let data = std::fs::read(&tmp).expect("read downloaded fixture");
    let _ = std::fs::remove_file(&tmp);
    data
}

// ───────────────────────── BASIC 6 (S3-62 smoke) ─────────────────────────

#[tokio::test]
async fn aws_connect_and_bucket_bound() {
    let Some(reg) = s3_acceptance::maybe_skip_aws() else {
        return;
    };
    let _page = reg
        .list_page(&aws_root(), None)
        .await
        .expect("bucket root list");
}

#[tokio::test]
async fn aws_account_root_list_buckets() {
    // S3-62 target-root acceptance: ARX ListBuckets (bucket=None) must physically
    // see the disposable acceptance bucket under the full-role identity.
    let Some(reg) = s3_acceptance::maybe_skip_aws() else {
        return;
    };
    let bucket = aws_bucket();
    let res = reg
        .list_page(
            &Location::S3 {
                target: "aws".to_string(),
                bucket: None,
                prefix: "".to_string(),
            },
            None,
        )
        .await;
    let page = res.expect("account-root ListBuckets must succeed under full role");
    assert!(
        page.entries.iter().any(|e| e.entry.name == bucket),
        "disposable acceptance bucket must be visible via ListBuckets"
    );
}
#[tokio::test]
async fn aws_basic_upload_download_roundtrip() {
    let Some(reg) = s3_acceptance::maybe_skip_aws() else {
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
    reg.delete_s3_at(&aws_root(), &key).await.expect("cleanup");
}
#[tokio::test]
async fn aws_prefix_navigation() {
    let Some(reg) = s3_acceptance::maybe_skip_aws() else {
        return;
    };
    let run = s3_acceptance::run_id();
    let child_key = format!("arx-acceptance/{run}/prefix-a/file.txt");
    upload_bytes(&reg, &child_key, b"hello aws").await;
    let parent = list_all(&reg, &scoped(&run, "")).await;
    assert!(
        parent.iter().any(|e| e.entry.name == "prefix-a"),
        "nested prefix visible"
    );
    let sub = list_all(&reg, &scoped(&run, "prefix-a")).await;
    assert!(
        sub.iter().any(|e| e.entry.name == "file.txt"),
        "child visible under prefix"
    );
    reg.delete_s3_at(&aws_root(), &child_key)
        .await
        .expect("cleanup child");
}
#[tokio::test]
async fn aws_incremental_pagination_1005() {
    let Some(reg) = s3_acceptance::maybe_skip_aws() else {
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
        reg.delete_s3_at(&aws_root(), &key).await.expect("cleanup");
    }
}
#[tokio::test]
async fn aws_unicode_identity_and_bytes() {
    let Some(reg) = s3_acceptance::maybe_skip_aws() else {
        return;
    };
    let run = s3_acceptance::run_id();
    let key = format!("arx-acceptance/{run}/日本語/каталог/🧙‍♂️.txt");
    let payload = s3_acceptance::deterministic_bytes(0xC0FFEE, 256);
    upload_bytes(&reg, &key, &payload).await;
    let got = download_bytes(&reg, &key).await;
    assert!(s3_acceptance::byte_eq(&got, &payload), "unicode byte-exact");
    reg.delete_s3_at(&aws_root(), &key)
        .await
        .expect("cleanup unicode");
}
#[tokio::test]
async fn aws_zero_byte_and_folder_marker() {
    let Some(reg) = s3_acceptance::maybe_skip_aws() else {
        return;
    };
    let run = s3_acceptance::run_id();
    let zb = format!("arx-acceptance/{run}/zero.bin");
    upload_bytes(&reg, &zb, &[]).await;
    let listed = list_all(&reg, &scoped(&run, "")).await;
    let z = listed
        .iter()
        .find(|e| e.entry.name == "zero.bin")
        .expect("zero-byte listed");
    assert_eq!(z.entry.size, Some(0), "size=0, not mistaken for prefix");
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
        "fresh marker empty"
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
    reg.delete_s3_at(&aws_root(), &zb)
        .await
        .expect("cleanup zero-byte");
}

// ───────────────────────── A — S3-64 MULTIPART ─────────────────────────

const BIG: u64 = 65 * 1024 * 1024; // >64 MiB single-put ceiling => multipart
fn make_big_file(path: &std::path::Path, seed: u64) {
    let mut f = std::fs::File::create(path).expect("create big temp");
    let mut written = 0u64;
    let mut block = vec![0u8; 256 * 1024];
    while written < BIG {
        let n = (seed.wrapping_mul(2654435761).wrapping_add(written)) as u32;
        for (i, b) in block.iter_mut().enumerate() {
            *b = ((n >> (i % 24)) ^ (i as u32)) as u8;
        }
        let take = std::cmp::min(block.len() as u64, BIG - written) as usize;
        f.write_all(&block[..take]).expect("write block");
        written += take as u64;
    }
    f.flush().unwrap();
    drop(f);
}
#[tokio::test]
async fn aws_multipart_upload_roundtrip() {
    let Some(reg) = s3_acceptance::maybe_skip_aws() else {
        return;
    };
    let run = s3_acceptance::run_id();
    let key = format!("arx-acceptance/{run}/big.bin");
    let tmp = std::env::temp_dir().join(format!(
        "arx-big-{}-{}.bin",
        std::process::id(),
        hexify(&key)
    ));
    make_big_file(&tmp, 0x9E3779B9);
    let plan = TransferPlan {
        source: Location::Local(std::env::temp_dir()),
        destination: aws_root(),
        intent: TransferIntent::Copy,
        method: TransferMethod::S3,
        s3_spec: Some(S3TransferSpec::UploadOne {
            local_source: tmp.clone(),
            destination: S3ObjectRef {
                target: "aws".to_string(),
                bucket: aws_bucket(),
                key: key.clone(),
            },
        }),
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let out = execute_transfer(&plan, std::slice::from_ref(&key), &reg, cancel, |_| {})
        .await
        .expect("multipart upload");
    assert_eq!(out.completed, 1, "multipart upload completed");
    let dl = std::env::temp_dir().join(format!(
        "arx-big-dl-{}-{}.bin",
        std::process::id(),
        hexify(&key)
    ));
    let dplan = TransferPlan {
        source: aws_root(),
        destination: Location::Local(std::env::temp_dir()),
        intent: TransferIntent::Copy,
        method: TransferMethod::S3,
        s3_spec: Some(S3TransferSpec::DownloadOne {
            source: S3ObjectRef {
                target: "aws".to_string(),
                bucket: aws_bucket(),
                key: key.clone(),
            },
            local_destination: dl.clone(),
        }),
    };
    let cancel2 = Arc::new(AtomicBool::new(false));
    let dout = execute_transfer(&dplan, std::slice::from_ref(&key), &reg, cancel2, |_| {})
        .await
        .expect("multipart download");
    assert_eq!(dout.completed, 1);
    assert_eq!(
        std::fs::metadata(&dl).unwrap().len(),
        BIG,
        "downloaded size == uploaded size"
    );
    let mut a = std::fs::File::open(&tmp).unwrap();
    let mut b = std::fs::File::open(&dl).unwrap();
    let (mut ra, mut rb) = ([0u8; 65536], [0u8; 65536]);
    let mut pos = 0u64;
    loop {
        let na = std::io::Read::read(&mut a, &mut ra).unwrap();
        let nb = std::io::Read::read(&mut b, &mut rb).unwrap();
        assert_eq!(na, nb, "byte lengths match at {pos}");
        if na == 0 {
            break;
        }
        assert_eq!(&ra[..na], &rb[..nb], "bytes match at {pos}");
        pos += na as u64;
    }
    let _ = std::fs::remove_file(&tmp);
    let _ = std::fs::remove_file(&dl);
    reg.delete_s3_at(&aws_root(), &key)
        .await
        .expect("cleanup big");
}

// ───────────────────────── B — S3-64 CANCEL / ABORT ─────────────────────────

#[tokio::test]
async fn aws_multipart_cancel_aborts_without_completed_object() {
    let Some(reg) = s3_acceptance::maybe_skip_aws() else {
        return;
    };
    let run = s3_acceptance::run_id();
    let key = format!("arx-acceptance/{run}/cancelled.bin");
    let tmp = std::env::temp_dir().join(format!(
        "arx-can-{}-{}.bin",
        std::process::id(),
        hexify(&key)
    ));
    make_big_file(&tmp, 0x1234ABCD);
    let plan = TransferPlan {
        source: Location::Local(std::env::temp_dir()),
        destination: aws_root(),
        intent: TransferIntent::Copy,
        method: TransferMethod::S3,
        s3_spec: Some(S3TransferSpec::UploadOne {
            local_source: tmp.clone(),
            destination: S3ObjectRef {
                target: "aws".to_string(),
                bucket: aws_bucket(),
                key: key.clone(),
            },
        }),
    };
    // cancel set BEFORE execute => no usable object may appear
    let cancel = Arc::new(AtomicBool::new(true));
    let res = execute_transfer(&plan, std::slice::from_ref(&key), &reg, cancel, |_| {}).await;
    let _ = std::fs::remove_file(&tmp);
    assert!(
        res.is_err(),
        "upload must not complete when cancelled before start"
    );
    let names = list_all(&reg, &scoped(&run, "")).await;
    assert!(
        !names.iter().any(|n| n.entry.name == "cancelled.bin"),
        "cancelled upload left no object"
    );
}

/// S3-64 real contract: cancel AFTER >=1 accepted part must attempt AbortMultipartUpload,
/// must NOT call CompleteMultipartUpload, and must leave no completed object.
/// Deterministic: cancellation is triggered from the progress hook after the first part
/// succeeds — not via timing/sleep. The executor's per-iteration cancel check then aborts
/// before scheduling the next part.
#[tokio::test]
async fn aws_multipart_cancel_after_first_part_aborts() {
    let Some(reg) = s3_acceptance::maybe_skip_aws() else {
        return;
    };
    let run = s3_acceptance::run_id();
    let key = format!("arx-acceptance/{run}/cancel-after-part.bin");
    let tmp = std::env::temp_dir().join(format!(
        "arx-cap-{}-{}.bin",
        std::process::id(),
        hexify(&key)
    ));
    make_big_file(&tmp, 0xCAFE1234);
    let plan = TransferPlan {
        source: Location::Local(std::env::temp_dir()),
        destination: aws_root(),
        intent: TransferIntent::Copy,
        method: TransferMethod::S3,
        s3_spec: Some(S3TransferSpec::UploadOne {
            local_source: tmp.clone(),
            destination: S3ObjectRef {
                target: "aws-bucket".to_string(),
                bucket: aws_bucket(),
                key: key.clone(),
            },
        }),
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_hook = cancel.clone();
    // deterministic: once >=1 part completed, raise cancellation before the next part.
    let on_progress = move |p: TransferProgress| {
        if p.completed >= 1 {
            cancel_hook.store(true, Ordering::Relaxed);
        }
    };
    let res = execute_transfer(&plan, std::slice::from_ref(&key), &reg, cancel, on_progress).await;
    let _ = std::fs::remove_file(&tmp);
    // Abort path returns Err (not a silent completed object).
    assert!(res.is_err(), "cancel after first part must not complete");
    // No completed object may exist.
    let names = list_all(&reg, &scoped(&run, "")).await;
    assert!(
        !names
            .iter()
            .any(|n| n.entry.name == "cancel-after-part.bin"),
        "completed destination object absent after cancel/abort"
    );
    // S3-64 physical orphan query: full role has s3:ListBucketMultipartUploads.
    // Verify NO active multipart upload remains for the exact key.
    // Must use the explicit temporary arx-full profile (no ambient/default fallback).
    use aws_sdk_s3::Client as S3Client;
    let full_profile = std::env::var("ARX_AWS_FULL_PROFILE")
        .expect("ARX_AWS_FULL_PROFILE required for orphan verifier");
    let shared = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .profile_name(full_profile)
        .region(Region::new(region_for_test()))
        .load()
        .await;
    let client = S3Client::new(&shared);
    let uploads = client
        .list_multipart_uploads()
        .bucket(aws_bucket())
        .prefix(format!("arx-acceptance/{run}/"))
        .send()
        .await
        .expect("ListBucketMultipartUploads must succeed under full role");
    let orphan = uploads
        .uploads()
        .iter()
        .any(|u| u.key().map(|k| k == key).unwrap_or(false));
    assert!(
        !orphan,
        "no active multipart upload remains for exact key after abort"
    );
}

// ───────────────────────── E2 — EXPLICIT PUT DENIAL ─────────────────────────

#[tokio::test]
async fn aws_denial_put_object() {
    let Some(full) = s3_acceptance::maybe_skip_aws() else {
        return;
    };
    let Some(deny) = build_denial_registry("ARX_AWS_DENY_PUT_PROFILE", "PutObject").await else {
        return;
    };
    let run = s3_acceptance::run_id();
    let key = format!("arx-acceptance/{run}/denied-put.bin");
    let tmp = std::env::temp_dir().join(format!(
        "arx-dput-{}-{}.bin",
        std::process::id(),
        hexify(&key)
    ));
    std::fs::write(&tmp, b"denied put payload").unwrap();
    let plan = TransferPlan {
        source: Location::Local(std::env::temp_dir()),
        destination: Location::S3 {
            target: "aws-deny".to_string(),
            bucket: Some(aws_bucket()),
            prefix: "".to_string(),
        },
        intent: TransferIntent::Copy,
        method: TransferMethod::S3,
        s3_spec: Some(S3TransferSpec::UploadOne {
            local_source: tmp.clone(),
            destination: S3ObjectRef {
                target: "aws-deny".to_string(),
                bucket: aws_bucket(),
                key: key.clone(),
            },
        }),
    };
    let c = Arc::new(AtomicBool::new(false));
    let res = execute_transfer(&plan, std::slice::from_ref(&key), &deny, c, |_| {}).await;
    let _ = std::fs::remove_file(&tmp);
    assert!(res.is_err(), "PutObject denied must fail, no false success");
    // Verify absence via FULL identity (exact run-prefix/object identity),
    // not via the deny listing which may itself be denied.
    let listed = list_all(&full, &scoped(&run, "")).await;
    assert!(
        listed.iter().all(|e| e.entry.name != "denied-put.bin"),
        "denied put created no object (verified via full identity)"
    );
    let diag = format!("{res:?}");
    s3_acceptance::assert_no_secret_leak(&diag);
}

// ───────────────────────── E3 — EXPLICIT DELETE DENIAL ─────────────────────────

#[tokio::test]
async fn aws_denial_delete_object() {
    // Create via FULL acceptance identity, delete via DENY-DELETE identity.
    let Some(full) = s3_acceptance::maybe_skip_aws() else {
        return;
    };
    let Some(deny) = build_denial_registry("ARX_AWS_DENY_DEL_PROFILE", "DeleteObject").await else {
        return;
    };
    let run = s3_acceptance::run_id();
    let key = format!("arx-acceptance/{run}/denied-del.bin");
    // create with full identity
    upload_bytes(&full, &key, b"to be deleted by denied role").await;
    // delete via denial identity — exact deny location (target + bucket + key)
    let deny_loc = Location::S3 {
        target: "aws-deny".to_string(),
        bucket: Some(aws_bucket()),
        prefix: "".to_string(),
    };
    let del_res = deny.delete_s3_at(&deny_loc, &key).await;
    assert!(
        del_res.is_err(),
        "DeleteObject denied must fail, no false success"
    );
    // object still exists afterward (confirmed via full identity)
    let listed = list_all(&full, &scoped(&run, "")).await;
    assert!(
        listed.iter().any(|e| e.entry.name == "denied-del.bin"),
        "object still exists after denied delete"
    );
    let diag = format!("{del_res:?}");
    s3_acceptance::assert_no_secret_leak(&diag);
    // cleanup with full identity
    full.delete_s3_at(&aws_root(), &key)
        .await
        .expect("cleanup denied-del");
}

// ───────────────────────── C — S3-65 SESSION TOKEN ─────────────────────────

#[tokio::test]
async fn aws_session_credentials_work_through_sdk_chain() {
    let Some(reg) = s3_acceptance::maybe_skip_aws() else {
        return;
    };
    // Explicitly require a temporary AssumeRole session token in the arx-full profile.
    // Verify non-empty token in the shared credentials file WITHOUT printing it.
    let creds_file = std::env::var("AWS_SHARED_CREDENTIALS_FILE")
        .expect("AWS_SHARED_CREDENTIALS_FILE must be set by setup script");
    let full_profile = std::env::var("ARX_AWS_FULL_PROFILE")
        .expect("ARX_AWS_FULL_PROFILE must be set for AWS acceptance");
    let content = std::fs::read_to_string(&creds_file).expect("read shared credentials");
    // find the [full_profile] section and ensure aws_session_token is present + non-empty
    let has_token = content
        .lines()
        .skip_while(|l| l.trim() != format!("[{full_profile}]"))
        .skip(1)
        .take_while(|l| !l.trim_start().starts_with('['))
        .any(|l| {
            let (k, v) = l.split_once('=').unwrap_or(("", ""));
            k.trim() == "aws_session_token" && !v.trim().is_empty()
        });
    assert!(
        has_token,
        "arx-full profile must contain a non-empty aws_session_token"
    );
    let run = s3_acceptance::run_id();
    let key = format!("arx-acceptance/{run}/session.txt");
    upload_bytes(&reg, &key, b"via session token").await;
    let got = download_bytes(&reg, &key).await;
    assert_eq!(&got, b"via session token");
    // diagnostics must not contain the session token
    let diag = format!("{:?}", reg);
    s3_acceptance::assert_no_secret_leak(&diag);
    reg.delete_s3_at(&aws_root(), &key)
        .await
        .expect("cleanup session");
}

// ───────────────────────── D — S3-65 NO-LISTBUCKETS ROLE ─────────────────────────

#[tokio::test]
async fn aws_bucket_bound_target_works_without_list_all_my_buckets() {
    // Uses ARX_AWS_NOLB_PROFILE (a role with ListBucket on the bucket but NO s3:ListAllMyBuckets).
    let profile = match std::env::var("ARX_AWS_NOLB_PROFILE") {
        Ok(p) if !p.trim().is_empty() => p,
        _ => {
            eprintln!("ARX_AWS_NOLB_PROFILE not set; skipping no-ListBuckets role test");
            return;
        }
    };
    let cfg = s3_acceptance::aws_target_with_profile("aws-nolb", &profile);
    let registry = ProviderRegistry::new();
    registry.register_s3_targets(&[cfg]);
    // account root ListBuckets must fail factually
    let root_res = registry
        .list_page(
            &Location::S3 {
                target: "aws-nolb".to_string(),
                bucket: None,
                prefix: "".to_string(),
            },
            None,
        )
        .await;
    assert!(root_res.is_err(), "account-root ListBuckets must be denied");
    // but bucket-bound target lists + reads
    let run = s3_acceptance::run_id();
    let root = Location::S3 {
        target: "aws-nolb".to_string(),
        bucket: Some(aws_bucket()),
        prefix: "".to_string(),
    };
    let key = format!("arx-acceptance/{run}/nolb.txt");
    let tmp = std::env::temp_dir().join(format!(
        "arx-nolb-{}-{}.bin",
        std::process::id(),
        hexify(&key)
    ));
    std::fs::write(&tmp, b"bucket bound").unwrap();
    let up = TransferPlan {
        source: Location::Local(std::env::temp_dir()),
        destination: root.clone(),
        intent: TransferIntent::Copy,
        method: TransferMethod::S3,
        s3_spec: Some(S3TransferSpec::UploadOne {
            local_source: tmp.clone(),
            destination: S3ObjectRef {
                target: "aws-nolb".to_string(),
                bucket: aws_bucket(),
                key: key.clone(),
            },
        }),
    };
    let c = Arc::new(AtomicBool::new(false));
    execute_transfer(&up, std::slice::from_ref(&key), &registry, c, |_| {})
        .await
        .expect("bucket-bound upload");
    let _ = std::fs::remove_file(&tmp);
    let _page = registry
        .list_page(&root, None)
        .await
        .expect("bucket-bound list works");
    let dl = download_bytes_in(&registry, "aws-nolb", &key).await;
    assert_eq!(&dl, b"bucket bound");
    registry
        .delete_s3_at(&root, &key)
        .await
        .expect("cleanup nolb");
}
async fn download_bytes_in(registry: &ProviderRegistry, target: &str, key: &str) -> Vec<u8> {
    let tmp = std::env::temp_dir().join(format!(
        "arx-dl2-{}-{}.bin",
        std::process::id(),
        hexify(key)
    ));
    let spec = S3TransferSpec::DownloadOne {
        source: S3ObjectRef {
            target: target.to_string(),
            bucket: aws_bucket(),
            key: key.to_string(),
        },
        local_destination: tmp.clone(),
    };
    let plan = TransferPlan {
        source: Location::S3 {
            target: target.to_string(),
            bucket: Some(aws_bucket()),
            prefix: "".to_string(),
        },
        destination: Location::Local(std::env::temp_dir()),
        intent: TransferIntent::Copy,
        method: TransferMethod::S3,
        s3_spec: Some(spec),
    };
    let c = Arc::new(AtomicBool::new(false));
    execute_transfer(&plan, &[key.to_string()], registry, c, |_| {})
        .await
        .expect("download");
    let data = std::fs::read(&tmp).expect("read");
    let _ = std::fs::remove_file(&tmp);
    data
}

// ───────────────────────── E — S3-65 CONTROLLED DENIAL MATRIX ─────────────────────────

async fn build_denial_registry(profile_env: &str, profile_label: &str) -> Option<ProviderRegistry> {
    let profile = match std::env::var(profile_env) {
        Ok(p) if !p.trim().is_empty() => p,
        _ => {
            eprintln!("{profile_env} not set; skipping {profile_label} denial test");
            return None;
        }
    };
    let cfg = s3_acceptance::aws_target_with_profile("aws-deny", &profile);
    let registry = ProviderRegistry::new();
    registry.register_s3_targets(&[cfg]);
    Some(registry)
}
#[tokio::test]
async fn aws_denial_list_objects() {
    let Some(reg) = build_denial_registry("ARX_AWS_DENY_LIST_PROFILE", "ListObjects").await else {
        return;
    };
    let res = reg
        .list_page(
            &Location::S3 {
                target: "aws-deny".to_string(),
                bucket: Some(aws_bucket()),
                prefix: "arx-acceptance/x".to_string(),
            },
            None,
        )
        .await;
    assert!(res.is_err(), "ListObjects denied must fail");
}
#[tokio::test]
async fn aws_denial_get_object() {
    // GetObject denial: full identity creates the fixture, deny-Get registry
    // attempts a REAL bounded read; listing success is NOT evidence for Get denial.
    let Some(full) = s3_acceptance::maybe_skip_aws() else {
        return;
    };
    let Some(deny) = build_denial_registry("ARX_AWS_DENY_GET_PROFILE", "GetObject").await else {
        return;
    };
    let run = s3_acceptance::run_id();
    let key = format!("arx-acceptance/{run}/denied-get.bin");
    // fixture via full identity
    upload_bytes(&full, &key, b"secret-get-fixture").await;
    // freeze exact ref + attempt bounded read through deny-Get registry
    let tmp = std::env::temp_dir().join(format!(
        "arx-dg-{}-{}.bin",
        std::process::id(),
        hexify(&key)
    ));
    let spec = S3TransferSpec::DownloadOne {
        source: S3ObjectRef {
            target: "aws-deny".to_string(),
            bucket: aws_bucket(),
            key: key.clone(),
        },
        local_destination: tmp.clone(),
    };
    let plan = TransferPlan {
        source: Location::S3 {
            target: "aws-deny".to_string(),
            bucket: Some(aws_bucket()),
            prefix: "".to_string(),
        },
        destination: Location::Local(std::env::temp_dir()),
        intent: TransferIntent::Copy,
        method: TransferMethod::S3,
        s3_spec: Some(spec),
    };
    let c = Arc::new(AtomicBool::new(false));
    let get_res = execute_transfer(&plan, std::slice::from_ref(&key), &deny, c, |_| {}).await;
    assert!(
        get_res.is_err(),
        "GetObject denied must fail, no false bytes / zero-byte success"
    );
    // sanity: full identity CAN read it (fixture existed)
    let got = download_bytes(&full, &key).await;
    assert_eq!(
        &got, b"secret-get-fixture",
        "fixture readable by full identity"
    );
    let diag = format!("{get_res:?}");
    s3_acceptance::assert_no_secret_leak(&diag);
    // cleanup with full identity
    full.delete_s3_at(&aws_root(), &key)
        .await
        .expect("cleanup denied-get");
}

// ───────────────────────── F — WRONG REGION ─────────────────────────

#[tokio::test]
async fn aws_wrong_region_factual_behavior() {
    let Some(_) = s3_acceptance::maybe_skip_aws() else {
        return;
    };
    let cfg = s3_acceptance::aws_target_wrong_region();
    let registry = ProviderRegistry::new();
    registry.register_s3_targets(&[cfg]);
    let res = registry
        .list_page(
            &Location::S3 {
                target: "aws-wrong-region".to_string(),
                bucket: Some(aws_bucket()),
                prefix: "arx-acceptance/x".to_string(),
            },
            None,
        )
        .await;
    let diag = format!("{res:?}");
    s3_acceptance::assert_no_secret_leak(&diag);
    eprintln!(
        "WRONG_REGION observed: {}",
        if res.is_ok() {
            "ok (SDK redirect)"
        } else {
            "explicit region error"
        }
    );
}

// ───────────────────────── G — BUCKET MISSING ─────────────────────────

#[tokio::test]
async fn aws_missing_bucket_factual_error() {
    let Some(reg) = s3_acceptance::maybe_skip_aws() else {
        return;
    };
    let run = s3_acceptance::run_id();
    let missing = format!("arx-acceptance-missing-{run}");
    let res = reg
        .list_page(
            &Location::S3 {
                target: "aws".to_string(),
                bucket: Some(missing.clone()),
                prefix: "".to_string(),
            },
            None,
        )
        .await;
    assert!(
        res.is_err(),
        "nonexistent bucket must error, not empty/success"
    );
}

// ───────────────────────── H — OBJECT DISAPPEARS MID-OP ─────────────────────────

#[tokio::test]
async fn aws_object_disappears_mid_op() {
    let Some(reg) = s3_acceptance::maybe_skip_aws() else {
        return;
    };
    let run = s3_acceptance::run_id();
    let key = format!("arx-acceptance/{run}/vanish.txt");
    upload_bytes(&reg, &key, b"will vanish").await;
    let frozen = S3ObjectRef {
        target: "aws".to_string(),
        bucket: aws_bucket(),
        key: key.clone(),
    };
    reg.delete_s3_at(&aws_root(), &key)
        .await
        .expect("fixture remove");
    let tmp = std::env::temp_dir().join(format!(
        "arx-vanish-{}-{}.bin",
        std::process::id(),
        hexify(&key)
    ));
    let spec = S3TransferSpec::DownloadOne {
        source: frozen.clone(),
        local_destination: tmp.clone(),
    };
    let plan = TransferPlan {
        source: aws_root(),
        destination: Location::Local(std::env::temp_dir()),
        intent: TransferIntent::Copy,
        method: TransferMethod::S3,
        s3_spec: Some(spec),
    };
    let c = Arc::new(AtomicBool::new(false));
    let res = execute_transfer(&plan, std::slice::from_ref(&key), &reg, c, |_| {}).await;
    assert!(
        res.is_err(),
        "download of vanished object must error, not zero-byte success"
    );
    assert!(!tmp.exists(), "no final destination created");
}

// ───────────────────────── I — INVALID CREDS (isolated) ─────────────────────────

#[tokio::test]
async fn aws_invalid_credentials_no_fallback() {
    match std::env::var("ARX_AWS_INVALID_CREDS") {
        Ok(v) if !v.trim().is_empty() => {}
        _ => {
            eprintln!("ARX_AWS_INVALID_CREDS not set; skipping invalid-creds test");
            return;
        }
    }
    // Genuinely reach the invalid-credential SDK path: use the registered
    // `aws-invalid` target (not aws_root()/aws-bucket).
    let cfg = s3_acceptance::aws_target_with_profile(
        "aws-invalid",
        &std::env::var("ARX_AWS_INVALID_PROFILE").unwrap_or_else(|_| "invalid".to_string()),
    );
    let registry = ProviderRegistry::new();
    registry.register_s3_targets(&[cfg]);
    let invalid_loc = Location::S3 {
        target: "aws-invalid".to_string(),
        bucket: Some(aws_bucket()),
        prefix: "arx-acceptance/invalid".to_string(),
    };
    let res = registry.list_page(&invalid_loc, None).await;
    assert!(
        res.is_err(),
        "invalid creds must fail, no fallback to ambient identity"
    );
}
