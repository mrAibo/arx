//! S3-64E / S3-64 multipart acceptance (emulator + MinIO).
//!
//! Exercises the multipart upload path (>64 MiB => Create/UploadPart/Complete,
//! with cancel/abort truth). Uses the SAME production transfer runtime as AWS.
//!
//! Gated: ARX_EMULATOR_TEST=1 (Moto :5000) and/or ARX_MINIO_TEST=1.

mod s3_acceptance;

use arx::transfer::executor::execute_transfer;
use arx::transfer::{S3TransferSpec, TransferIntent, TransferMethod, TransferPlan};
use arx::vfs::{ListedEntry, Location, ProviderContinuation, ProviderRegistry, S3ObjectRef};
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

// Single-put ceiling in s3_upload is 64 MiB; go above to force multipart.
const BIG: u64 = 65 * 1024 * 1024;

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

fn make_big_file(path: &std::path::Path, seed: u64) {
    let mut f = std::fs::File::create(path).expect("create big temp");
    let mut written = 0u64;
    let mut block = vec![0u8; 256 * 1024];
    while written < BIG {
        // deterministic pseudo-random block
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

async fn list_names(reg: &ProviderRegistry, loc: &Location) -> Vec<String> {
    let mut out = Vec::new();
    let mut cont: Option<ProviderContinuation> = None;
    loop {
        let page = reg.list_page(loc, cont.as_ref()).await.expect("list_page");
        out.extend(
            page.entries
                .iter()
                .map(|e: &ListedEntry| e.entry.name.clone()),
        );
        match page.continuation {
            Some(c) if !c.token.is_empty() => cont = Some(c),
            _ => break,
        }
    }
    out
}

async fn run_multipart_roundtrip(reg: &ProviderRegistry, target: &str) {
    let run = s3_acceptance::run_id();
    let key = format!("arx-acceptance/{run}/big.bin");
    let k_up = key.clone();
    let k_dl = key.clone();
    let tmp = std::env::temp_dir().join(format!("arx-big-{}-{}", std::process::id(), hex(&key)));
    make_big_file(&tmp, 0x9E3779B9);
    let plan = TransferPlan {
        source: Location::Local(std::env::temp_dir()),
        destination: root(target),
        intent: TransferIntent::Copy,
        method: TransferMethod::S3,
        s3_spec: Some(S3TransferSpec::UploadOne {
            local_source: tmp.clone(),
            destination: S3ObjectRef {
                target: target.to_string(),
                bucket: "arxtest".to_string(),
                key: key.clone(),
            },
        }),
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let out = execute_transfer(&plan, &[k_up], reg, cancel, |_| {})
        .await
        .expect("multipart upload");
    assert_eq!(out.completed, 1, "multipart upload completed");
    // download + byte-exact compare (streaming, not full memory)
    let dl = std::env::temp_dir().join(format!("arx-big-dl-{}-{}", std::process::id(), hex(&key)));
    let dplan = TransferPlan {
        source: root(target),
        destination: Location::Local(std::env::temp_dir()),
        intent: TransferIntent::Copy,
        method: TransferMethod::S3,
        s3_spec: Some(S3TransferSpec::DownloadOne {
            source: S3ObjectRef {
                target: target.to_string(),
                bucket: "arxtest".to_string(),
                key: key.clone(),
            },
            local_destination: dl.clone(),
        }),
    };
    let cancel2 = Arc::new(AtomicBool::new(false));
    let dout = execute_transfer(&dplan, &[k_dl], reg, cancel2, |_| {})
        .await
        .expect("multipart download");
    assert_eq!(dout.completed, 1);
    assert_eq!(
        std::fs::metadata(&dl).unwrap().len(),
        BIG,
        "downloaded size == uploaded size"
    );
    // streaming byte compare
    let mut a = std::fs::File::open(&tmp).unwrap();
    let mut b = std::fs::File::open(&dl).unwrap();
    let (mut ra, mut rb) = ([0u8; 65536], [0u8; 65536]);
    let mut pos = 0u64;
    loop {
        let na = std::io::Read::read(&mut a, &mut ra).unwrap();
        let nb = std::io::Read::read(&mut b, &mut rb).unwrap();
        assert_eq!(na, nb, "byte lengths match at {pos}");
        if na == 0usize {
            break;
        }
        assert_eq!(&ra[..na], &rb[..nb], "bytes match at {pos}");
        pos += na as u64;
    }
    let _ = std::fs::remove_file(&tmp);
    let _ = std::fs::remove_file(&dl);
    reg.delete_s3_at(&root(target), &key)
        .await
        .expect("cleanup big");
}

async fn run_multipart_cancel_before(reg: &ProviderRegistry, target: &str) {
    let run = s3_acceptance::run_id();
    let key = format!("arx-acceptance/{run}/cancelled.bin");
    let k_can = key.clone();
    let tmp = std::env::temp_dir().join(format!("arx-can-{}-{}", std::process::id(), hex(&key)));
    make_big_file(&tmp, 0x1234ABCD);
    let plan = TransferPlan {
        source: Location::Local(std::env::temp_dir()),
        destination: root(target),
        intent: TransferIntent::Copy,
        method: TransferMethod::S3,
        s3_spec: Some(S3TransferSpec::UploadOne {
            local_source: tmp.clone(),
            destination: S3ObjectRef {
                target: target.to_string(),
                bucket: "arxtest".to_string(),
                key: key.clone(),
            },
        }),
    };
    // cancel already set BEFORE execute => must not create a usable object
    let cancel = Arc::new(AtomicBool::new(true));
    let res = execute_transfer(&plan, &[k_can], reg, cancel, |_| {}).await;
    let _ = std::fs::remove_file(&tmp);
    // Any error is acceptable when cancelled before start; the key point is that
    // no usable object is created. (ARX may classify this as a generic cancelled
    // error rather than TransferExecutionError::Cancelled — both are correct.)
    assert!(
        res.is_err(),
        "upload must not complete when cancelled before start"
    );
    // object must not be present (abort cleaned up any partial multipart)
    let names = list_names(reg, &scoped(&run, target)).await;
    assert!(
        !names.iter().any(|n| n == "cancelled.bin"),
        "cancelled upload left no object"
    );
}

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{:02x}", b)).collect()
}

#[tokio::test]
async fn emulator_multipart() {
    let Some(reg) = s3_acceptance::maybe_skip_emulator() else {
        return;
    };
    run_multipart_roundtrip(&reg, "emulator").await;
    run_multipart_cancel_before(&reg, "emulator").await;
}

#[tokio::test]
async fn minio_multipart() {
    let Some(reg) = s3_acceptance::maybe_skip_minio() else {
        return;
    };
    run_multipart_roundtrip(&reg, "minio").await;
    run_multipart_cancel_before(&reg, "minio").await;
}
