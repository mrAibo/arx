use std::io::Write as _;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use arx::effects::{Effect, EffectEvent};
use arx::process::ProcessService;
use arx::remote::{Host, validate_ssh_alias};
use arx::vfs::sftp::SftpProvider;
use arx::vfs::{
    CancellationFlag, Location, MAX_REMOTE_EDIT_BYTES, ProviderRegistry, RemoteEditSession,
    VfsProvider, capabilities,
};

struct RemoteFixture {
    host: String,
    base: String,
}

impl Drop for RemoteFixture {
    fn drop(&mut self) {
        let _ = ssh_run(&self.host, &format!("rm -rf -- {}", sh_quote(&self.base)));
    }
}

fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn ssh_run(host: &str, script: &str) -> std::io::Result<()> {
    let status = Command::new("ssh")
        .arg(host)
        .arg(format!("sh -c {}", sh_quote(script)))
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "ssh {host} exited with {status}: {script}"
        )))
    }
}

async fn production_download(
    registry: &ProviderRegistry,
    location: &Location,
    name: &str,
) -> EffectEvent {
    ProcessService::execute_with_registry(
        Effect::DownloadRemoteFile {
            location: location.clone(),
            name: name.to_string(),
            editor: "true".to_string(),
        },
        registry,
    )
    .await
}

async fn production_writeback(
    registry: &ProviderRegistry,
    session: RemoteEditSession,
) -> EffectEvent {
    ProcessService::execute_with_registry(
        Effect::WriteBackRemoteFile {
            session,
            progress: arx::effects::ProgressSlot(None),
        },
        registry,
    )
    .await
}

fn ssh_write(host: &str, path: &str, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    let quoted = sh_quote(path);
    let script = format!("set -eu; umask 077; cat > {quoted}; chmod {mode:o} {quoted}");
    let mut child = Command::new("ssh")
        .arg(host)
        .arg(format!("sh -c {}", sh_quote(&script)))
        .stdin(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or_else(|| std::io::Error::other("ssh stdin unavailable"))?
        .write_all(bytes)?;
    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "ssh write {path} exited with {status}"
        )))
    }
}

async fn assert_no_transaction_artifacts(
    provider: &SftpProvider,
    base: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let entries = provider.list_async(base).await?;
    assert!(
        entries.iter().all(|entry| {
            !entry.name.contains(".arx-part-")
                && !entry.name.contains(".arx-txn-")
                && !entry.name.contains(".arx-read-")
        }),
        "transaction artifacts remain: {entries:?}"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires ARX_SFTP_SMOKE_HOST pointing at a disposable SSH/SFTP host"]
async fn remote_edit_sftp_smoke() -> Result<(), Box<dyn std::error::Error>> {
    let host_alias = std::env::var("ARX_SFTP_SMOKE_HOST")?;
    validate_ssh_alias(&host_alias)?;
    let token = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let base = format!("/tmp/arx-demo/arx-remote-edit-smoke-{token}");
    let quoted_base = sh_quote(&base);
    ssh_run(
        &host_alias,
        &format!("set -eu; mkdir -p {quoted_base}; chmod 700 {quoted_base}"),
    )?;
    let _fixture = RemoteFixture {
        host: host_alias.clone(),
        base: base.clone(),
    };
    let provider = SftpProvider::new(Host::from_alias(&host_alias));
    let cancellation = CancellationFlag::default();

    let small_path = format!("{base}/small.txt");
    ssh_write(&host_alias, &small_path, b"old\n", 0o600)?;
    let fresh_metadata = provider.metadata(&small_path).await?;
    assert!(fresh_metadata.is_regular);
    assert_eq!(fresh_metadata.len, 4);
    assert_eq!(fresh_metadata.unix_mode.unwrap() & 0o7777, 0o600);
    let original_uid = fresh_metadata.unix_uid.expect("OpenSSH SFTP uid");
    let original_gid = fresh_metadata.unix_gid.expect("OpenSSH SFTP gid");
    let small_revision = provider
        .read_all_capped(&small_path, 1024)
        .await?
        .into_revision()?;
    assert_eq!(small_revision.bytes(), b"old\n");
    provider
        .write_file_bytes_if_unchanged(&small_path, b"new\n", &small_revision, &cancellation, None)
        .await?;
    assert_eq!(
        provider.read_all_capped(&small_path, 1024).await?.bytes,
        b"new\n"
    );
    let edited_metadata = provider.metadata(&small_path).await?;
    assert_eq!(edited_metadata.unix_mode.unwrap() & 0o7777, 0o600);
    assert_eq!(edited_metadata.unix_uid, Some(original_uid));
    assert_eq!(edited_metadata.unix_gid, Some(original_gid));

    let secondary_group =
        std::env::var("ARX_SFTP_SMOKE_GROUP").unwrap_or_else(|_| "sys".to_string());
    let group_path = format!("{base}/group-owned.txt");
    ssh_write(&host_alias, &group_path, b"group old\n", 0o660)?;
    ssh_run(
        &host_alias,
        &format!(
            "chgrp -- {} {}; chmod 660 -- {}",
            sh_quote(&secondary_group),
            sh_quote(&group_path),
            sh_quote(&group_path)
        ),
    )?;
    let group_revision = provider
        .read_all_capped(&group_path, 1024)
        .await?
        .into_revision()?;
    assert_ne!(group_revision.unix_gid(), original_gid);
    provider
        .write_file_bytes_if_unchanged(
            &group_path,
            b"group new\n",
            &group_revision,
            &cancellation,
            None,
        )
        .await?;
    let group_metadata = provider.metadata(&group_path).await?;
    assert_eq!(group_metadata.unix_uid, Some(original_uid));
    assert_eq!(group_metadata.unix_gid, Some(group_revision.unix_gid()));
    assert_eq!(group_metadata.unix_mode.unwrap() & 0o7777, 0o660);

    let executable_path = format!("{base}/script.sh");
    ssh_write(&host_alias, &executable_path, b"#!/bin/sh\nexit 0\n", 0o755)?;
    let executable_revision = provider
        .read_all_capped(&executable_path, 1024)
        .await?
        .into_revision()?;
    provider
        .write_file_bytes_if_unchanged(
            &executable_path,
            b"#!/bin/sh\nprintf ok\n",
            &executable_revision,
            &cancellation,
            None,
        )
        .await?;
    assert_eq!(
        provider
            .metadata(&executable_path)
            .await?
            .unix_mode
            .unwrap()
            & 0o7777,
        0o755
    );

    let large_path = format!("{base}/large.txt");
    let large_original = vec![b'a'; 1_200_000];
    let large_edited = vec![b'b'; 1_300_000];
    ssh_write(&host_alias, &large_path, &large_original, 0o644)?;
    let large_revision = provider
        .read_all_capped(&large_path, 2_000_000)
        .await?
        .into_revision()?;
    provider
        .write_file_bytes_if_unchanged(
            &large_path,
            &large_edited,
            &large_revision,
            &cancellation,
            None,
        )
        .await?;
    let large_read = provider.read_all_capped(&large_path, 2_000_000).await?;
    assert!(!large_read.truncated);
    assert_eq!(large_read.bytes, large_edited);
    assert_eq!(
        provider.metadata(&large_path).await?.unix_mode.unwrap() & 0o7777,
        0o644
    );

    let exact_path = format!("{base}/exact-limit.txt");
    let exact = vec![b'x'; MAX_REMOTE_EDIT_BYTES];
    ssh_write(&host_alias, &exact_path, &exact, 0o600)?;
    let exact_result = provider
        .read_all_capped(&exact_path, MAX_REMOTE_EDIT_BYTES)
        .await?;
    assert!(!exact_result.truncated);
    assert_eq!(exact_result.bytes, exact);

    let oversized_path = format!("{base}/oversized.txt");
    let oversized = vec![b'y'; MAX_REMOTE_EDIT_BYTES + 1];
    ssh_write(&host_alias, &oversized_path, &oversized, 0o600)?;
    let oversized_result = provider
        .read_all_capped(&oversized_path, MAX_REMOTE_EDIT_BYTES)
        .await?;
    assert!(oversized_result.truncated);
    assert_eq!(oversized_result.bytes.len(), MAX_REMOTE_EDIT_BYTES);

    let conflict_path = format!("{base}/same-size-conflict.txt");
    ssh_write(&host_alias, &conflict_path, b"AAAA", 0o600)?;
    let conflict_revision = provider
        .read_all_capped(&conflict_path, 16)
        .await?
        .into_revision()?;
    ssh_write(&host_alias, &conflict_path, b"BBBB", 0o600)?;
    let conflict = provider
        .write_file_bytes_if_unchanged(
            &conflict_path,
            b"LOCAL",
            &conflict_revision,
            &cancellation,
            None,
        )
        .await
        .expect_err("same-size remote mutation must conflict");
    assert_eq!(conflict.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(
        provider.read_all_capped(&conflict_path, 16).await?.bytes,
        b"BBBB"
    );

    let zero_path = format!("{base}/zero-conflict.txt");
    ssh_write(&host_alias, &zero_path, b"", 0o600)?;
    let zero_revision = provider
        .read_all_capped(&zero_path, 16)
        .await?
        .into_revision()?;
    ssh_write(&host_alias, &zero_path, b"x", 0o600)?;
    let zero_conflict = provider
        .write_file_bytes_if_unchanged(&zero_path, b"local", &zero_revision, &cancellation, None)
        .await
        .expect_err("zero-byte frozen revision must not be a sentinel");
    assert_eq!(zero_conflict.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(provider.read_all_capped(&zero_path, 16).await?.bytes, b"x");

    let mode_path = format!("{base}/mode-conflict.txt");
    ssh_write(&host_alias, &mode_path, b"MODE", 0o600)?;
    let mode_revision = provider
        .read_all_capped(&mode_path, 16)
        .await?
        .into_revision()?;
    ssh_run(
        &host_alias,
        &format!("chmod 644 -- {}", sh_quote(&mode_path)),
    )?;
    let mode_conflict = provider
        .write_file_bytes_if_unchanged(&mode_path, b"local", &mode_revision, &cancellation, None)
        .await
        .expect_err("concurrent chmod must conflict");
    assert_eq!(mode_conflict.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(
        provider.read_all_capped(&mode_path, 16).await?.bytes,
        b"MODE"
    );
    assert_eq!(
        provider.metadata(&mode_path).await?.unix_mode.unwrap() & 0o7777,
        0o644
    );

    let symlink_target = format!("{base}/symlink-target.txt");
    let symlink_path = format!("{base}/symlink.txt");
    ssh_write(&host_alias, &symlink_target, b"target", 0o600)?;
    ssh_run(
        &host_alias,
        &format!(
            "ln -s -- {} {}",
            sh_quote(&symlink_target),
            sh_quote(&symlink_path)
        ),
    )?;
    assert!(!provider.metadata(&symlink_path).await?.is_regular);
    let symlink_error = provider
        .read_all_capped(&symlink_path, MAX_REMOTE_EDIT_BYTES)
        .await
        .expect_err("remote edit must not follow symlinks");
    assert_eq!(symlink_error.kind(), std::io::ErrorKind::InvalidInput);

    let empty_name = "empty.txt";
    let unicode_name = "unicode.txt";
    let nul_name = "nul.txt";
    let invalid_utf8_name = "invalid-utf8.txt";
    ssh_run(
        &host_alias,
        &format!(": > {}", sh_quote(&format!("{base}/{empty_name}"))),
    )?;
    ssh_run(
        &host_alias,
        &format!(
            "printf 'Привет, 世界 🌍\\n' > {}",
            sh_quote(&format!("{base}/{unicode_name}"))
        ),
    )?;
    ssh_run(
        &host_alias,
        &format!(
            "printf 'a\\000b' > {}",
            sh_quote(&format!("{base}/{nul_name}"))
        ),
    )?;
    ssh_run(
        &host_alias,
        &format!(
            "printf '\\377' > {}",
            sh_quote(&format!("{base}/{invalid_utf8_name}"))
        ),
    )?;

    let registry = ProviderRegistry::new();
    registry.insert_sftp(
        &host_alias,
        Box::new(SftpProvider::new(Host::from_alias(&host_alias))),
        capabilities::SFTP_CAPABILITIES,
    );
    let location = Location::Sftp {
        host: host_alias.clone(),
        path: base.clone(),
    };
    let EffectEvent::Downloaded {
        session: empty_session,
    } = production_download(&registry, &location, empty_name).await
    else {
        panic!("production download must accept an empty file");
    };
    assert!(empty_session.revision.bytes().is_empty());
    assert!(matches!(
        production_writeback(&registry, empty_session).await,
        EffectEvent::NoChange { name } if name == empty_name
    ));

    let EffectEvent::Downloaded {
        session: unicode_session,
    } = production_download(&registry, &location, unicode_name).await
    else {
        panic!("production download must accept Unicode UTF-8");
    };
    assert_eq!(
        unicode_session.revision.bytes(),
        "Привет, 世界 🌍\n".as_bytes()
    );
    tokio::fs::write(
        unicode_session.temp_dir.path().join("working"),
        "Изменено, 世界 🌍\n",
    )
    .await?;
    assert!(matches!(
        production_writeback(&registry, unicode_session).await,
        EffectEvent::WrittenBack { name } if name == unicode_name
    ));
    assert_eq!(
        provider
            .read_all_capped(&format!("{base}/{unicode_name}"), 1024)
            .await?
            .bytes,
        "Изменено, 世界 🌍\n".as_bytes()
    );

    assert!(matches!(
        production_download(&registry, &location, "exact-limit.txt").await,
        EffectEvent::Downloaded { .. }
    ));
    assert!(matches!(
        production_download(&registry, &location, "oversized.txt").await,
        EffectEvent::Failed { error, .. } if error.contains("too large")
    ));

    let EffectEvent::Downloaded {
        session: oversized_edit_session,
    } = production_download(&registry, &location, empty_name).await
    else {
        panic!("production download must create an editable empty session");
    };
    tokio::fs::write(
        oversized_edit_session.temp_dir.path().join("working"),
        vec![b'z'; MAX_REMOTE_EDIT_BYTES + 1],
    )
    .await?;
    assert!(matches!(
        production_writeback(&registry, oversized_edit_session).await,
        EffectEvent::Failed { error, .. } if error.contains("remote edit limit")
    ));
    assert!(
        provider
            .read_all_capped(&format!("{base}/{empty_name}"), 16)
            .await?
            .bytes
            .is_empty()
    );

    let production_conflict_name = "production-conflict.txt";
    let production_conflict_path = format!("{base}/{production_conflict_name}");
    ssh_write(&host_alias, &production_conflict_path, b"AAAA", 0o600)?;
    let EffectEvent::Downloaded {
        session: production_conflict_session,
    } = production_download(&registry, &location, production_conflict_name).await
    else {
        panic!("production conflict fixture must download");
    };
    tokio::fs::write(
        production_conflict_session.temp_dir.path().join("working"),
        b"LOCAL",
    )
    .await?;
    ssh_write(&host_alias, &production_conflict_path, b"BBBB", 0o600)?;
    assert!(matches!(
        production_writeback(&registry, production_conflict_session).await,
        EffectEvent::RemoteConflict { name, .. } if name == production_conflict_name
    ));
    assert_eq!(
        provider
            .read_all_capped(&production_conflict_path, 16)
            .await?
            .bytes,
        b"BBBB"
    );

    assert!(matches!(
        production_download(&registry, &location, nul_name).await,
        EffectEvent::Failed { error, .. } if error.contains("NUL")
    ));
    assert!(matches!(
        production_download(&registry, &location, invalid_utf8_name).await,
        EffectEvent::Failed { error, .. } if error.contains("valid UTF-8")
    ));

    assert_no_transaction_artifacts(&provider, &base).await?;
    Ok(())
}
