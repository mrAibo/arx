//! P8 physical safe-read retry through the REAL TransferQueueRuntime.
//!
//! Uploads a >=1 MiB object to real MinIO, then downloads it through the
//! runtime with the S3 endpoint pointed at a local one-shot TCP fault proxy.
//! The proxy forwards the FIRST GetObject verbatim to upstream but closes the
//! ARX-facing connection early after a small body prefix, causing a REAL
//! transport/body read failure. The SECOND GetObject is a transparent
//! pass-through and succeeds. The production S3 download seam
//! (body.read error -> remove_staged -> cleanup_then_safe -> SafeToRetry)
//! must drive the runtime into RetryWaiting and then a successful second
//! attempt, all under one JobId.
//!
//! Gated behind ARX_MINIO_TEST=1 (same as the other MinIO physical suites).

mod s3_acceptance;

use arx::transfer::executor::execute_transfer;
use arx::transfer::{S3TransferSpec, TransferIntent, TransferMethod, TransferPlan};
use arx::transfer_queue::{PauseGate, TransferQueueConfig};
use arx::transfer_queue_runtime::TransferQueueRuntime;
use arx::vfs::{Location, ProviderRegistry, S3ObjectRef};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

/// One-shot fault proxy: faults the first GET to OUR key, passes the rest
/// through untouched. Counts GETs so we can assert exactly two happened.
struct FaultProxy {
    get_count: AtomicUsize,
    faulted_first: AtomicUsize,
}

async fn proxy_connection(proxy: Arc<FaultProxy>, upstream: String, mut client: TcpStream) {
    // Read the client's request line to decide fault vs pass-through.
    let mut peek = [0u8; 4096];
    let n = match client.peek(&mut peek).await {
        Ok(n) if n > 0 => n,
        _ => {
            return;
        }
    };
    let head = String::from_utf8_lossy(&peek[..n.min(peek.len())]);
    let is_get = head.starts_with("GET ");

    if is_get {
        let count = proxy.get_count.fetch_add(1, Ordering::SeqCst) + 1;
        let fault_this = count == 1; // fault exactly the first GET

        // Connect upstream and forward the exact request bytes unchanged.
        let mut up = match TcpStream::connect(&upstream).await {
            Ok(u) => u,
            Err(_) => return,
        };
        let mut req = vec![0u8; n];
        let _ = client.read_exact(&mut req).await;
        let _ = up.write_all(&req).await;

        if !fault_this {
            // Transparent pass-through: full duplex until both ends EOF.
            let (mut cr, mut cw) = client.split();
            let (mut ur, mut uw) = up.split();
            let _ = tokio::join!(
                tokio::io::copy(&mut ur, &mut cw),
                tokio::io::copy(&mut cr, &mut uw)
            );
            return;
        }

        // Fault path: read upstream status line + headers, forward them,
        // then forward only a small body prefix and close the client early.
        proxy.faulted_first.fetch_add(1, Ordering::SeqCst);
        let mut up_buf = [0u8; 4096];
        let mut resp_head = Vec::new();
        loop {
            match up.read(&mut up_buf).await {
                Ok(0) => break,
                Ok(m) => {
                    resp_head.extend_from_slice(&up_buf[..m]);
                    if resp_head.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = client.write_all(&resp_head).await;
        // Forward a small body prefix, then drop -> client sees truncated body.
        let mut body_prefix = [0u8; 8192];
        let bn = up.read(&mut body_prefix).await.unwrap_or(0);
        if bn > 0 {
            let _ = client.write_all(&body_prefix[..bn]).await;
        }
        // Drop both ends: early close forces a real body read failure.
        let _ = client.flush().await;
        drop(client);
        drop(up);
        return;
    }

    // Non-GET (e.g. connection setup noise): just pass through.
    if let Ok(mut up) = TcpStream::connect(&upstream).await {
        let (mut cr, mut cw) = client.split();
        let (mut ur, mut uw) = up.split();
        let _ = tokio::join!(
            tokio::io::copy(&mut ur, &mut cw),
            tokio::io::copy(&mut cr, &mut uw)
        );
    }
}

fn hexify(s: &str) -> String {
    s.bytes().map(|b| format!("{:02x}", b)).collect()
}

fn start_proxy(upstream: String) -> (u16, Arc<FaultProxy>) {
    let proxy = Arc::new(FaultProxy {
        get_count: AtomicUsize::new(0),
        faulted_first: AtomicUsize::new(0),
    });
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let listener = TcpListener::from_std(listener).unwrap();
    let proxy2 = proxy.clone();
    tokio::spawn(async move {
        while let Ok((client, _)) = listener.accept().await {
            let p = proxy2.clone();
            let up = upstream.clone();
            tokio::spawn(proxy_connection(p, up, client));
        }
    });
    (port, proxy)
}

async fn upload_object(registry: &ProviderRegistry, key: &str, payload: &[u8]) {
    // Upload via the production S3 transfer path (a normal, non-faulted target).
    let tmp = std::env::temp_dir().join(format!(
        "arx-p8-up-{}-{}.bin",
        std::process::id(),
        hexify(key)
    ));
    std::fs::write(&tmp, payload).unwrap();
    let spec = S3TransferSpec::UploadOne {
        local_source: tmp.clone(),
        destination: S3ObjectRef {
            target: "minio".to_string(),
            bucket: "arxtest".to_string(),
            key: key.to_string(),
        },
    };
    let plan = TransferPlan {
        source: Location::Local(tmp.clone()),
        destination: arx::vfs::Location::S3 {
            target: "minio".to_string(),
            bucket: Some("arxtest".to_string()),
            prefix: String::new(),
        },
        intent: TransferIntent::Copy,
        method: TransferMethod::S3,
        s3_spec: Some(spec),
        webdav_spec: None,
    };
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let outcome = execute_transfer(
        &plan,
        &[key.to_string()],
        registry,
        cancel,
        PauseGate::disabled(),
        |_| {},
    )
    .await
    .expect("upload via transfer");
    assert_eq!(outcome.completed, 1, "exactly one object uploaded");
    let _ = std::fs::remove_file(&tmp);
}

#[tokio::test]
async fn p8_s3_safe_read_retry_through_runtime() {
    let Some(reg) = s3_acceptance::maybe_skip_minio() else {
        return;
    };

    // Proxy target: same MinIO, but through a local fault proxy.
    let (proxy_port, proxy) = start_proxy("127.0.0.1:9000".to_string());
    let registry = ProviderRegistry::new();
    registry.register_s3_targets(&[arx::config::S3TargetConfig {
        id: "minio-proxy".to_string(),
        name: "minio-proxy".to_string(),
        bucket: Some("arxtest".to_string()),
        region: Some("us-east-1".to_string()),
        profile: None,
        endpoint_url: Some(format!("http://127.0.0.1:{proxy_port}")),
        force_path_style: true,
    }]);

    // Upload a >=1 MiB object to real MinIO (via the non-proxied target).
    let run = s3_acceptance::run_id();
    let key = format!("arx-acceptance/{run}/p8-large.bin");
    let payload = vec![0x4b; 2 * 1024 * 1024]; // 2 MiB
    upload_object(&reg, &key, &payload).await;

    // Build a DownloadOne plan through the proxy target.
    let dest_dir = std::env::temp_dir().join(format!("arx-p8-dl-{}-{}", std::process::id(), run));
    std::fs::create_dir_all(&dest_dir).unwrap();
    let final_path = dest_dir.join("p8-large.bin");
    let spec = S3TransferSpec::DownloadOne {
        source: S3ObjectRef {
            target: "minio-proxy".to_string(),
            bucket: "arxtest".to_string(),
            key: key.clone(),
        },
        local_destination: final_path.clone(),
    };
    let plan = TransferPlan {
        source: arx::vfs::Location::S3 {
            target: "minio-proxy".to_string(),
            bucket: Some("arxtest".to_string()),
            prefix: String::new(),
        },
        destination: Location::Local(dest_dir.clone()),
        intent: TransferIntent::Copy,
        method: TransferMethod::S3,
        s3_spec: Some(spec),
        webdav_spec: None,
    };

    let manager = arx::jobs::JobManager::new();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let config = TransferQueueConfig::new(1).unwrap();
    let runtime = TransferQueueRuntime::new(manager, tx, registry, config);

    let id = runtime.enqueue(plan, vec![key.clone()]).unwrap();

    // Observe the JobManager lifecycle via events.
    let mut saw_running = 0usize;
    let mut saw_retry_waiting = false;
    let mut saw_completed = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let observed_stage_at_retry: Arc<Mutex<Option<bool>>> = Arc::new(Mutex::new(None));
    loop {
        if let Ok(ev) = rx.try_recv() {
            match ev {
                arx::jobs::JobEvent::Running { id: eid, .. } => {
                    if eid == id {
                        saw_running += 1;
                    }
                }
                arx::jobs::JobEvent::RetryWaiting { id: eid } => {
                    if eid == id {
                        saw_retry_waiting = true;
                        // Stage artifact must be gone before retry.
                        let stage_present = std::fs::read_dir(&dest_dir)
                            .map(|mut d| {
                                d.any(|e| {
                                    let n = e.unwrap().file_name().to_string_lossy().to_string();
                                    n.contains(".arx-part")
                                })
                            })
                            .unwrap_or(false);
                        *observed_stage_at_retry.lock().unwrap() = Some(!stage_present);
                    }
                }
                arx::jobs::JobEvent::Completed { id: eid, .. } if eid == id => {
                    saw_completed = true;
                }
                _ => {}
            }
        }
        if saw_completed {
            break;
        }
        if tokio::time::Instant::now() > deadline {
            panic!("timed out waiting for completion");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Drain any remaining events.
    while let Ok(ev) = rx.try_recv() {
        match ev {
            arx::jobs::JobEvent::RetryWaiting { id: eid } if eid == id => {
                saw_retry_waiting = true;
            }
            _ => {}
        }
    }

    let records = runtime.manager().snapshot();
    let job = runtime.manager().get(&id).expect("job record exists");
    let final_progress = match &job.progress {
        arx::jobs::JobProgress::Generic(arx::jobs::Progress::Bytes { done, total, .. }) => {
            (*done, *total)
        }
        other => panic!("unexpected progress: {other:?}"),
    };

    // Assert no staged artifact remains after success.
    let stage_leftover = std::fs::read_dir(&dest_dir)
        .map(|mut d| {
            d.any(|e| {
                let n = e.unwrap().file_name().to_string_lossy().to_string();
                n.contains(".arx-part")
            })
        })
        .unwrap_or(false);

    let final_bytes = std::fs::read(&final_path).expect("final file readable");

    println!(
        "P8 GET_COUNT={} faulted_first={} running_events={} retry_waiting={} completed={} same_job_id={} records={} stage_clean_at_retry={:?} stage_leftover={} final_progress={:?} final_bytes={} expected={}",
        proxy.get_count.load(Ordering::SeqCst),
        proxy.faulted_first.load(Ordering::SeqCst),
        saw_running,
        saw_retry_waiting,
        saw_completed,
        id,
        records.len(),
        observed_stage_at_retry.lock().unwrap(),
        stage_leftover,
        final_progress,
        final_bytes.len(),
        payload.len(),
    );

    // Required facts.
    assert_eq!(
        proxy.get_count.load(Ordering::SeqCst),
        2,
        "exactly two GetObject"
    );
    assert_eq!(
        proxy.faulted_first.load(Ordering::SeqCst),
        1,
        "first GET faulted"
    );
    assert!(saw_running >= 2, "saw Running for both attempts");
    assert!(saw_retry_waiting, "saw RetryWaiting exactly once path");
    assert!(saw_completed, "saw Completed");
    assert_eq!(records.len(), 1, "exactly one JobManager transfer record");
    assert_eq!(job.id, id, "same JobId throughout");
    assert_eq!(
        *observed_stage_at_retry.lock().unwrap(),
        Some(true),
        "staged .arx-part removed before retry"
    );
    assert!(!stage_leftover, "no staged artifact after success");
    assert_eq!(
        final_progress.0,
        final_progress.1.unwrap(),
        "progress exact (done==total)"
    );
    assert_eq!(
        job.progress.percent(),
        Some(100),
        "percent 100 where total known"
    );
    assert_eq!(final_bytes.len(), payload.len(), "final bytes byte-exact");
    assert_eq!(final_bytes, payload, "final content byte-exact");

    runtime.shutdown().await;
}
