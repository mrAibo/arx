use super::Entry;
use crate::remote::Host;
use anyhow::Context;
use russh::keys;
use russh_sftp::client::SftpSession;
use std::collections::BTreeSet;
use std::future::Future;
use std::io;
use std::sync::Arc;
use tokio::runtime::Handle;

/// SFTP filesystem backend.
pub struct SftpFs;

impl SftpFs {
    pub fn list(host: &Host, remote_path: &str) -> io::Result<Vec<Entry>> {
        let host = host.clone();
        let path = remote_path.to_string();
        let handle = Handle::try_current().map_err(|_| io::Error::other("no tokio runtime"))?;
        handle
            .block_on(async move { list_sftp(&host, &path).await })
            .map_err(|e| io::Error::other(format!("SFTP: {e:#}")))
    }
}

async fn list_sftp(host: &Host, remote_path: &str) -> anyhow::Result<Vec<Entry>> {
    // Resolve through ~/.ssh/config
    let (resolved_host, resolved_port, resolved_user, _key_path, _proxy) =
        crate::remote::ssh_config::resolve_alias(&host.ssh_alias);
    let hostname = if resolved_host != host.ssh_alias {
        resolved_host
    } else {
        host.hostname.clone()
    };
    let port = if resolved_port != 22 {
        resolved_port
    } else {
        host.port
    };
    let user = if resolved_user != std::env::var("USER").unwrap_or_default() {
        resolved_user
    } else {
        host.user.clone()
    };

    let config = Arc::new(russh::client::Config::default());
    let handler = Handler {
        hostname: hostname.clone(),
        port,
    };

    let mut client = russh::client::connect(config, (hostname.as_str(), port), handler)
        .await
        .with_context(|| format!("SSH connect to {hostname}:{port}"))?;

    // Try key auth, fall back to password
    let authed = if let Ok(key_pair) = load_key_pair() {
        matches!(
            client.authenticate_publickey(&user, key_pair).await,
            Ok(russh::client::AuthResult::Success)
        )
    } else if let Ok(pw) = std::env::var("SSH_PASSWORD") {
        matches!(
            client.authenticate_password(&user, &pw).await,
            Ok(russh::client::AuthResult::Success)
        )
    } else {
        false
    };

    anyhow::ensure!(
        authed,
        "SSH auth failed.\n\
         Try: ssh-agent (SSH_AUTH_SOCK), ~/.ssh/id_*, or SSH_PASSWORD env var.\n\
         ponytail: full ssh-agent + ~/.ssh/config resolution deferred — russh agent client needs typed Handler"
    );

    let channel = client.channel_open_session().await?;
    channel.request_subsystem(true, "sftp").await?;
    let sftp = SftpSession::new(channel.into_stream()).await?;

    let read_dir = sftp
        .read_dir(remote_path.to_string())
        .await
        .with_context(|| format!("SFTP read_dir {remote_path}"))?;

    let mut result: Vec<Entry> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for entry in read_dir {
        let name = entry.file_name();
        if !seen.insert(name.clone()) {
            continue;
        }
        let metadata = entry.metadata();
        let kind = if metadata.is_dir() {
            super::EntryKind::Directory
        } else if metadata.is_symlink() {
            super::EntryKind::Symlink
        } else {
            super::EntryKind::File
        };
        let size = Some(metadata.len());
        result.push(Entry { name, kind, size });
    }

    result.sort_by(|a, b| {
        match (
            a.kind == super::EntryKind::Directory,
            b.kind == super::EntryKind::Directory,
        ) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        }
    });

    Ok(result)
}

fn load_key_pair() -> anyhow::Result<russh::keys::PrivateKeyWithHashAlg> {
    let home = dirs::home_dir().context("no HOME")?;
    // Try common key types (ssh-agent is probed by russh internally)
    let key_types = ["id_ed25519", "id_ecdsa", "id_rsa", "id_dsa"];
    let mut data = String::new();
    let mut found = false;
    for kt in &key_types {
        let kp = home.join(".ssh").join(kt);
        if kp.exists() {
            data = std::fs::read_to_string(&kp)?;
            found = true;
            break;
        }
    }
    anyhow::ensure!(
        found,
        "no SSH key found in ~/.ssh/ (tried: {})",
        key_types.join(", ")
    );
    let key = russh::keys::PrivateKey::from_openssh(&data)?;
    Ok(russh::keys::PrivateKeyWithHashAlg::new(
        Arc::new(key),
        Some(russh::keys::HashAlg::Sha256),
    ))
}

struct Handler {
    hostname: String,
    port: u16,
}

impl russh::client::Handler for Handler {
    type Error = anyhow::Error;

    #[allow(clippy::manual_async_fn)]
    fn check_server_key(
        &mut self,
        server_public_key: &keys::PublicKey,
    ) -> impl Future<Output = Result<bool, Self::Error>> + Send {
        let hostname = self.hostname.clone();
        let port = self.port;
        let key = server_public_key.clone();
        async move {
            match keys::check_known_hosts(&hostname, port, &key) {
                Ok(true) => Ok(true),
                Ok(false) => {
                    // TOFU: save new host key automatically
                    let _ = russh::keys::known_hosts::learn_known_hosts(&hostname, port, &key);
                    Ok(true)
                }
                Err(e) => Err(anyhow::anyhow!(
                    "Host key mismatch for {}:{} — possible MITM attack.\nRemove old key from ~/.ssh/known_hosts.\nError: {e}",
                    hostname,
                    port
                )),
            }
        }
    }
}
