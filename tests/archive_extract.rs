use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use arx::transfer::archive_extract::extract_one;
use arx::transfer::{
    ArchiveTransferSpec, ExecutorAvailability, TransferIntent, TransferMethod, TransferPlanner,
    TransferRequest,
};
use arx::transfer_queue::PauseGate;
use arx::vfs::archive::ArchiveMemberRef;
use arx::vfs::{CapabilitySet, Location, ProviderId};

fn archive(path: &Path) -> Location {
    Location::Archive {
        archive: path.to_path_buf(),
        inner_path: String::new(),
    }
}

fn request(
    source: Location,
    destination: Location,
    spec: Option<ArchiveTransferSpec>,
) -> TransferRequest {
    let source_provider = match &source {
        Location::Archive { .. } => ProviderId::Archive,
        Location::Local(_) => ProviderId::Local,
        other => panic!("archive_extract test helper: unsupported source {other:?}"),
    };
    let destination_provider = match &destination {
        Location::Archive { .. } => ProviderId::Archive,
        Location::Local(_) => ProviderId::Local,
        other => panic!("archive_extract test helper: unsupported destination {other:?}"),
    };
    TransferRequest {
        source,
        destination,
        source_provider,
        destination_provider,
        source_capabilities: CapabilitySet::NONE,
        destination_capabilities: CapabilitySet::NONE,
        intent: TransferIntent::Copy,
        executors: ExecutorAvailability::local(),
        delete_extraneous: false,
        archive_spec: spec,
        s3_spec: None,
        webdav_spec: None,
    }
}

fn make_tar(source_dir: &Path, archive: &Path, member: &str, gzip: bool) {
    let mut command = std::process::Command::new("tar");
    command.current_dir(source_dir);
    command.arg(if gzip { "czf" } else { "cf" });
    command.arg(archive).arg("--").arg(member);
    assert!(command.status().unwrap().success());
}

#[test]
fn planner_only_accepts_archive_to_local_copy_with_frozen_spec() {
    let source = archive(Path::new("/tmp/source.tar"));
    let destination = Location::Local(PathBuf::from("/tmp/out"));
    let spec = ArchiveTransferSpec {
        source: ArchiveMemberRef {
            member_path: "nested/exact.txt".into(),
        },
        local_destination: PathBuf::from("/tmp/out/exact.txt"),
    };

    let plan = TransferPlanner::plan(request(
        source.clone(),
        destination.clone(),
        Some(spec.clone()),
    ))
    .unwrap();
    assert_eq!(plan.method, TransferMethod::Archive);
    assert_eq!(plan.archive_spec, Some(spec));

    assert!(TransferPlanner::plan(request(source.clone(), destination.clone(), None)).is_err());
    let mut move_request = request(source, destination, plan.archive_spec.clone());
    move_request.intent = TransferIntent::Move;
    assert!(TransferPlanner::plan(move_request).is_err());

    let local_to_archive = request(
        Location::Local(PathBuf::from("/tmp/in")),
        archive(Path::new("/tmp/source.tar")),
        plan.archive_spec,
    );
    assert!(TransferPlanner::plan(local_to_archive).is_err());
}

#[tokio::test]
async fn traversal_is_rejected_before_archive_access() {
    let temp = tempfile::tempdir().unwrap();
    let spec = ArchiveTransferSpec {
        source: ArchiveMemberRef {
            member_path: "../escape".into(),
        },
        local_destination: temp.path().join("escape"),
    };
    let error = extract_one(
        Path::new("/definitely/missing.tar"),
        &spec,
        Arc::new(AtomicBool::new(false)),
        PauseGate::disabled(),
        |_| {},
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert!(!temp.path().join("escape").exists());
}

#[tokio::test]
async fn extracts_unicode_and_spaces_from_tar_gz() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    std::fs::create_dir_all(source.join("資料 folder")).unwrap();
    std::fs::create_dir(&destination).unwrap();
    let member = "資料 folder/report ü.txt";
    std::fs::write(source.join(member), b"exact archive payload").unwrap();
    let archive = temp.path().join("fixture.tar.gz");
    make_tar(&source, &archive, member, true);
    let output = destination.join("report ü.txt");
    let spec = ArchiveTransferSpec {
        source: ArchiveMemberRef {
            member_path: member.into(),
        },
        local_destination: output.clone(),
    };

    extract_one(
        &archive,
        &spec,
        Arc::new(AtomicBool::new(false)),
        PauseGate::disabled(),
        |_| {},
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read(output).unwrap(), b"exact archive payload");
}

#[tokio::test]
async fn noclobber_preserves_existing_destination() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir(&destination).unwrap();
    std::fs::write(source.join("same.txt"), b"new").unwrap();
    let archive = temp.path().join("fixture.tar");
    make_tar(&source, &archive, "same.txt", false);
    let output = destination.join("same.txt");
    std::fs::write(&output, b"old").unwrap();
    let spec = ArchiveTransferSpec {
        source: ArchiveMemberRef {
            member_path: "same.txt".into(),
        },
        local_destination: output.clone(),
    };

    assert!(
        extract_one(
            &archive,
            &spec,
            Arc::new(AtomicBool::new(false)),
            PauseGate::disabled(),
            |_| {},
        )
        .await
        .is_err()
    );
    assert_eq!(std::fs::read(output).unwrap(), b"old");
}
