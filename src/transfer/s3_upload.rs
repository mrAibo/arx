//! S3 → Local upload core (S3-34/35).
//!
//! Invariants enforced here:
//! - exact `S3ObjectRef` bucket/key authority; no name-based reconstruction
//! - single PutObject, no multipart, no hidden retry (SDK retries stay disabled)
//! - `BASIC_TRANSFER_MAX_BYTES` guard before any PutObject
//! - explicit overwrite policy via HeadObject preflight, fail closed
//! - cancellation truth: no fake-abort semantics for a single PutObject
//! - diagnostics are sanitized: no key, credentials, signed query, or auth header

use crate::transfer::S3TransferSpec;
use crate::vfs::S3ObjectRef;
use crate::vfs::s3::S3Provider;
use aws_sdk_s3::error::ProvideErrorMetadata;
use aws_sdk_s3::operation::head_object::HeadObjectError;
use aws_sdk_s3::primitives::ByteStream;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Temporary single-put ceiling for the basic transfer pack. NOT an S3 service
/// limit; S3-43 may later replace/redefine threshold logic.
pub const BASIC_TRANSFER_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// Explicit frozen overwrite policy. Basic user-facing transfer defaults to
/// `Forbid`; `Confirmed` is only reachable through a separately wired explicit
/// confirmation flow (not in this pack's UI surface).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum S3OverwritePolicy {
    Forbid,
    Confirmed,
}

/// Result of an upload: bytes physically written.
pub type UploadOutcome = u64;

/// Pure outcome of the overwrite preflight, mapped from the HeadObject call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HeadState {
    /// Object is absent (NoSuchKey) — PutObject is permitted.
    Missing,
    /// Object exists — policy decides whether PutObject is allowed.
    Exists,
    /// HeadObject denied (AccessDenied) — factual failure, never treated as missing.
    AccessDenied,
    /// Any other error (network, throttling, unknown) — factual failure.
    Unknown,
}

/// Pure overwrite-policy verdict, computed from `HeadState` + policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OverwriteVerdict {
    Put,
    Conflict,
    Denied,
    Unknown,
}

/// Upload a single regular local file to the exact S3 destination ref.
///
/// S3-34/35 core. Validates locally, asserts exact identity, enforces the
/// single-put size ceiling, runs a HeadObject preflight for the overwrite
/// policy, then issues ONE physical PutObject. No multipart, no recursive,
/// no hidden retry (SDK retries stay disabled by the client factory).
#[allow(dead_code)]
pub(crate) async fn upload_one(
    provider: &S3Provider,
    spec: &S3TransferSpec,
    policy: S3OverwritePolicy,
    cancel: Arc<AtomicBool>,
) -> io::Result<UploadOutcome> {
    // 1. Extract the UploadOne payload.
    let (local_source, destination) = match spec {
        S3TransferSpec::UploadOne {
            local_source,
            destination,
        } => (local_source, destination),
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "S3TransferSpec is not UploadOne",
            ));
        }
    };

    // 2. Pre-request cancellation: zero mutation, no client, no network.
    if cancel.load(Ordering::Relaxed) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "upload cancelled before request",
        ));
    }

    // 3. Validate the local source (exists + regular file) and read its size.
    //    Pure local syscall — never a mutation and never reaches AWS.
    let meta = tokio::fs::metadata(local_source).await.map_err(|e| {
        io::Error::new(
            e.kind(),
            format!("S3 upload local source not readable: {}", e),
        )
    })?;
    if !meta.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "S3 upload local source is not a regular file",
        ));
    }
    let file_size = meta.len();

    // 4. Fail-closed identity validation BEFORE any AWS client/auth/network work.
    //    - exact target id match
    //    - if the provider is bucket-bound, the destination bucket must match
    validate_upload_identity(&provider.target.id, &provider.target.bucket, destination)?;

    // 5. Temporary single-put limit: refuse anything above the ceiling BEFORE PutObject.
    check_single_put_size(file_size)?;

    // 6. Get the (shared, lazily built) AWS client — no second client is created.
    let client = provider.client().await?;

    // 7. Overwrite preflight: safe HeadObject read, then a pure policy decision.
    let head = client
        .head_object()
        .bucket(&destination.bucket)
        .key(&destination.key)
        .send()
        .await;
    let state = match head {
        Ok(_) => HeadState::Exists,
        Err(sdk_err) => classify_head_error(&sdk_err.into_service_error()),
    };
    match decide_overwrite(policy, state) {
        OverwriteVerdict::Put => {}
        OverwriteVerdict::Conflict => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "S3 object already exists (overwrite forbidden)",
            ));
        }
        OverwriteVerdict::Denied => {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "S3 PutObject preflight denied (access denied)",
            ));
        }
        OverwriteVerdict::Unknown => {
            return Err(io::Error::other("S3 PutObject preflight failed"));
        }
    }

    // 8. Final pre-request cancellation check: if cancelled now, no PutObject
    //    has been issued and the HeadObject above was a read-only probe, so the
    //    operation is still zero-mutation.
    if cancel.load(Ordering::Relaxed) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "upload cancelled before request",
        ));
    }

    // 9. Exactly one PutObject, to the exact destination bucket/key. The body
    //     is a streaming ByteStream (no full-file allocation).
    let body = ByteStream::from_path(local_source).await.map_err(|e| {
        io::Error::other(format!("S3 upload failed to open local source stream: {e}"))
    })?;
    let put = client
        .put_object()
        .bucket(&destination.bucket)
        .key(&destination.key)
        .body(body)
        .send()
        .await;

    match put {
        Ok(_) => Ok(file_size),
        // Static, sanitized diagnostic: no key, credential, or signed query leaks.
        Err(_) => Err(io::Error::other("S3 PutObject upload failed")),
    }
}

/// Convenience: resolve the upload destination path for a `UploadOne` spec.
#[allow(dead_code)]
pub(crate) fn upload_local_source(spec: &S3TransferSpec) -> Option<PathBuf> {
    match spec {
        S3TransferSpec::UploadOne { local_source, .. } => Some(local_source.clone()),
        _ => None,
    }
}

/// Pure identity gate: exact target id match, and (if bucket-bound) exact
/// bucket match. Runs before any AWS work.
pub(crate) fn validate_upload_identity(
    provider_target_id: &str,
    provider_bucket: &Option<String>,
    dest: &S3ObjectRef,
) -> io::Result<()> {
    if dest.target != provider_target_id {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "S3 target mismatch: destination target id does not match provider",
        ));
    }
    if let Some(bound) = provider_bucket
        && bound != &dest.bucket
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "S3 bucket escape rejected: destination bucket is outside the bound target",
        ));
    }
    Ok(())
}

/// Pure single-put size guard. Refuse anything above `BASIC_TRANSFER_MAX_BYTES`
/// BEFORE any PutObject (S3-43 may later introduce multipart).
pub(crate) fn check_single_put_size(size: u64) -> io::Result<()> {
    if size > BASIC_TRANSFER_MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "file requires multipart S3 upload",
        ));
    }
    Ok(())
}

/// Pure: map a HeadObject service error into a `HeadState`. The object-absent
/// code is `NoSuchKey` on AWS; MinIO (S3-compatible) returns `NotFound`. Both
/// mean the object is missing. `AccessDenied` is NEVER inferred as missing;
/// everything else is `Unknown` (factual failure).
pub(crate) fn classify_head_error(svc: &HeadObjectError) -> HeadState {
    match svc.code() {
        Some("NoSuchKey") | Some("NotFound") => HeadState::Missing,
        Some("AccessDenied") => HeadState::AccessDenied,
        _ => HeadState::Unknown,
    }
}

/// Pure overwrite-policy decision. Missing => always Put. Exists => Put only
/// when `Confirmed`, else Conflict. AccessDenied/Unknown => factual failure.
pub(crate) fn decide_overwrite(policy: S3OverwritePolicy, state: HeadState) -> OverwriteVerdict {
    match (state, policy) {
        (HeadState::Missing, _) => OverwriteVerdict::Put,
        (HeadState::Exists, S3OverwritePolicy::Forbid) => OverwriteVerdict::Conflict,
        (HeadState::Exists, S3OverwritePolicy::Confirmed) => OverwriteVerdict::Put,
        (HeadState::AccessDenied, _) => OverwriteVerdict::Denied,
        (HeadState::Unknown, _) => OverwriteVerdict::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::S3TargetConfig;
    use crate::transfer::s3_upload_destination_ref;
    use crate::vfs::s3::S3Provider;
    use aws_sdk_s3::error::ErrorMetadata;
    use std::sync::atomic::AtomicBool;
    use tempfile::tempdir;

    // ── BASIC_TRANSFER_MAX_BYTES boundary (metadata only, no huge alloc) ──

    #[test]
    fn single_put_size_boundary() {
        // Exactly the ceiling is allowed; one byte over is not.
        assert!(check_single_put_size(BASIC_TRANSFER_MAX_BYTES).is_ok());
        assert!(check_single_put_size(BASIC_TRANSFER_MAX_BYTES + 1).is_err());
        assert!(check_single_put_size(0).is_ok());
    }

    #[test]
    fn single_put_size_error_kind() {
        let e = check_single_put_size(BASIC_TRANSFER_MAX_BYTES + 1).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::Unsupported);
    }

    // ── Overwrite policy branches (pure helpers, real logic) ──

    fn head_state_for(code: &str) -> HeadState {
        classify_head_error(&HeadObjectError::generic(
            ErrorMetadata::builder().code(code).build(),
        ))
    }

    #[test]
    fn missing_object_permits_put_under_both_policies() {
        let missing = head_state_for("NoSuchKey");
        assert_eq!(missing, HeadState::Missing);
        assert_eq!(
            decide_overwrite(S3OverwritePolicy::Forbid, missing),
            OverwriteVerdict::Put
        );
        assert_eq!(
            decide_overwrite(S3OverwritePolicy::Confirmed, missing),
            OverwriteVerdict::Put
        );
    }

    #[test]
    fn existing_object_conflicts_under_forbid() {
        let exists = HeadState::Exists;
        assert_eq!(
            decide_overwrite(S3OverwritePolicy::Forbid, exists),
            OverwriteVerdict::Conflict
        );
    }

    #[test]
    fn existing_object_puts_under_confirmed() {
        let exists = HeadState::Exists;
        assert_eq!(
            decide_overwrite(S3OverwritePolicy::Confirmed, exists),
            OverwriteVerdict::Put
        );
    }

    #[test]
    fn access_denied_never_inferred_missing() {
        let denied = head_state_for("AccessDenied");
        assert_eq!(denied, HeadState::AccessDenied);
        assert_eq!(
            decide_overwrite(S3OverwritePolicy::Forbid, denied),
            OverwriteVerdict::Denied
        );
        assert_eq!(
            decide_overwrite(S3OverwritePolicy::Confirmed, denied),
            OverwriteVerdict::Denied
        );
    }

    #[test]
    fn unknown_head_error_fails_factually() {
        // A network/timeout/throttling error has no code -> Unknown.
        let unknown = head_state_for("InternalError");
        assert_eq!(unknown, HeadState::Unknown);
        assert_eq!(
            decide_overwrite(S3OverwritePolicy::Forbid, unknown),
            OverwriteVerdict::Unknown
        );
        assert_eq!(
            decide_overwrite(S3OverwritePolicy::Confirmed, unknown),
            OverwriteVerdict::Unknown
        );
    }

    // ── Identity: target/bucket mismatch rejected before any client ──

    fn bound_provider() -> S3Provider {
        S3Provider::new(S3TargetConfig {
            id: "tgt".into(),
            name: "tgt".into(),
            bucket: Some("bk".into()),
            region: None,
            profile: None,
            endpoint_url: None,
            force_path_style: false,
        })
    }

    #[test]
    fn identity_target_mismatch_rejected() {
        let p = bound_provider();
        let dest = S3ObjectRef {
            target: "other".into(),
            bucket: "bk".into(),
            key: "a.txt".into(),
        };
        let e = validate_upload_identity(&p.target.id, &p.target.bucket, &dest).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn identity_bucket_mismatch_rejected() {
        let p = bound_provider();
        let dest = S3ObjectRef {
            target: "tgt".into(),
            bucket: "other-bk".into(),
            key: "a.txt".into(),
        };
        let e = validate_upload_identity(&p.target.id, &p.target.bucket, &dest).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn identity_match_accepted() {
        let p = bound_provider();
        let dest = S3ObjectRef {
            target: "tgt".into(),
            bucket: "bk".into(),
            key: "a.txt".into(),
        };
        assert!(validate_upload_identity(&p.target.id, &p.target.bucket, &dest).is_ok());
    }

    #[test]
    fn unbound_provider_accepts_any_bucket() {
        let p = S3Provider::new(S3TargetConfig {
            id: "tgt".into(),
            name: "tgt".into(),
            bucket: None,
            region: None,
            profile: None,
            endpoint_url: None,
            force_path_style: false,
        });
        let dest = S3ObjectRef {
            target: "tgt".into(),
            bucket: "any-bk".into(),
            key: "a.txt".into(),
        };
        assert!(validate_upload_identity(&p.target.id, &p.target.bucket, &dest).is_ok());
    }

    // ── S3 keys are opaque: no filesystem-traversal semantics (S3-42S) ──
    //
    // The LOCAL child filename is validated at the construction boundary by
    // `validate_child_name` (via `s3_upload_destination_ref`). The assembled
    // S3 key is an opaque string, so the navigation prefix is never rescanned
    // for "."/".."/"//" — those are legal key contents.

    #[test]
    fn opaque_key_accepts_dotdot_segment() {
        let dest = s3_upload_destination_ref("tgt", "bk", "foo/../bar", "a.txt").unwrap();
        assert_eq!(dest.key, "foo/../bar/a.txt");
    }

    #[test]
    fn opaque_key_accepts_dot_segment() {
        let dest = s3_upload_destination_ref("tgt", "bk", "foo/./bar", "a.txt").unwrap();
        assert_eq!(dest.key, "foo/./bar/a.txt");
    }

    #[test]
    fn opaque_key_accepts_double_slash() {
        let dest = s3_upload_destination_ref("tgt", "bk", "foo//bar", "a.txt").unwrap();
        assert_eq!(dest.key, "foo//bar/a.txt");
    }

    #[test]
    fn opaque_key_accepts_unicode() {
        let dest = s3_upload_destination_ref("tgt", "bk", "日本語/../資料", "file.txt").unwrap();
        assert_eq!(dest.key, "日本語/../資料/file.txt");
    }

    // Local child-name boundary still rejects filesystem-unsafe names.
    #[test]
    fn local_child_dotdot_rejected() {
        assert!(s3_upload_destination_ref("tgt", "bk", "p", "..").is_err());
    }

    #[test]
    fn local_child_slash_rejected() {
        assert!(s3_upload_destination_ref("tgt", "bk", "p", "foo/bar").is_err());
    }

    // Exact destination ref unchanged verbatim (target/bucket/key as built).
    #[test]
    fn exact_destination_ref_unchanged() {
        let dest = s3_upload_destination_ref("tgt", "bk", "pre/fix", "name.txt").unwrap();
        assert_eq!(dest.target, "tgt");
        assert_eq!(dest.bucket, "bk");
        assert_eq!(dest.key, "pre/fix/name.txt");
    }

    // ── Cancellation: pre-request => Cancelled, no client/network ──

    #[tokio::test]
    async fn pre_request_cancellation_returns_interrupted() {
        let p = bound_provider();
        let dir = tempdir().unwrap();
        let src = dir.path().join("a.txt");
        tokio::fs::write(&src, b"data").await.unwrap();
        let dest = S3ObjectRef {
            target: "tgt".into(),
            bucket: "bk".into(),
            key: "a.txt".into(),
        };
        let spec = S3TransferSpec::UploadOne {
            local_source: src,
            destination: dest,
        };
        let cancel = Arc::new(AtomicBool::new(true));
        let e = upload_one(&p, &spec, S3OverwritePolicy::Forbid, cancel)
            .await
            .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::Interrupted);
    }

    // ── Non-regular / missing local source rejected before client ──

    #[tokio::test]
    async fn missing_local_source_rejected() {
        let p = bound_provider();
        let src = PathBuf::from("/nonexistent/arx/upload/path.txt");
        let dest = S3ObjectRef {
            target: "tgt".into(),
            bucket: "bk".into(),
            key: "a.txt".into(),
        };
        let spec = S3TransferSpec::UploadOne {
            local_source: src,
            destination: dest,
        };
        // With cancel clear, the missing local source is rejected at the local
        // stat step (before any client/network).
        let cancel = Arc::new(AtomicBool::new(false));
        let e = upload_one(&p, &spec, S3OverwritePolicy::Forbid, cancel)
            .await
            .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
    }
}
