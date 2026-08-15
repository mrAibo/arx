//! S3-72/73/74 — regression: S3 capability flip did not regress other providers,
//! and S3 stays bucket-bound (no ListBuckets escape). Factual, no network.

use arx::vfs::capabilities::builtin_capabilities;
use arx::vfs::{Capability, ProviderId};

#[test]
fn regression_s3_flip_did_not_regress_others() {
    // Local: core file ops retained after S3 flip (regression guard).
    let local = builtin_capabilities(ProviderId::Local);
    for c in [
        Capability::List,
        Capability::Read,
        Capability::Write,
        Capability::Mkdir,
        Capability::Delete,
        Capability::Copy,
        Capability::Move,
    ] {
        assert!(local.supports(c), "Local must retain {c:?} after S3 flip");
    }
    // Sftp: core remote ops retained after S3 flip.
    let sftp = builtin_capabilities(ProviderId::Sftp);
    for c in [
        Capability::List,
        Capability::Read,
        Capability::Write,
        Capability::Mkdir,
        Capability::Delete,
    ] {
        assert!(sftp.supports(c), "Sftp must retain {c:?} after S3 flip");
    }
    // S3: exactly the MVP set, never widened to Copy/Move/Rename/Symlink/Chmod/ServerSideCopy.
    let s3 = builtin_capabilities(ProviderId::S3);
    for c in [
        Capability::List,
        Capability::Read,
        Capability::Write,
        Capability::Mkdir,
        Capability::Delete,
    ] {
        assert!(s3.supports(c), "S3 must keep {c:?}");
    }
    for c in [
        Capability::Copy,
        Capability::Move,
        Capability::Rename,
        Capability::Symlink,
        Capability::Chmod,
        Capability::ServerSideCopy,
    ] {
        assert!(
            !s3.supports(c),
            "S3 must NOT gain {c:?} (no regression-widening)"
        );
    }
}
