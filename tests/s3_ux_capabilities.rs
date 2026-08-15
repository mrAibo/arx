//! S3-69/70/71 — UX capability contract (factual, no network).
//!
//! Verifies the S3 capability set the UI is built on top of:
//! available = {List, Read, Write, Mkdir, Delete}; NOT available =
//! {Copy, Move, Rename, Symlink, Chmod, ServerSideCopy}. The TUI availability
//! layer (`action_availability`) delegates to exactly these capabilities, so a
//! correct set here is what keeps Copy/Move/Rename disabled in the S3 UI.
//!
//! This is a contract test (no endpoint needed); it gates the UX hardening
//! claims for S3-69/70/71 without asserting on live provider IAM.

use arx::vfs::capabilities::builtin_capabilities;
use arx::vfs::{Capability, ProviderId};

#[test]
fn s3_capability_contract_ui_facing() {
    let caps = builtin_capabilities(ProviderId::S3);

    // Available in the S3 UI.
    assert!(
        caps.supports(Capability::List),
        "S3: List available (UI lists)"
    );
    assert!(
        caps.supports(Capability::Read),
        "S3: Read available (F3 preview)"
    );
    assert!(
        caps.supports(Capability::Write),
        "S3: Write available (upload)"
    );
    assert!(
        caps.supports(Capability::Mkdir),
        "S3: Mkdir available (folder marker)"
    );
    assert!(
        caps.supports(Capability::Delete),
        "S3: Delete available (F8)"
    );

    // Explicitly NOT available — UI must keep these disabled on S3.
    assert!(!caps.supports(Capability::Copy), "S3: Copy disabled in UI");
    assert!(!caps.supports(Capability::Move), "S3: Move disabled in UI");
    assert!(
        !caps.supports(Capability::Rename),
        "S3: Rename disabled in UI"
    );
    assert!(
        !caps.supports(Capability::Symlink),
        "S3: Symlink disabled in UI"
    );
    assert!(
        !caps.supports(Capability::Chmod),
        "S3: Chmod disabled in UI"
    );
    assert!(
        !caps.supports(Capability::ServerSideCopy),
        "S3: ServerSideCopy disabled in UI"
    );
}
