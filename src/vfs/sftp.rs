use super::Entry;
use crate::remote::Host;
use anyhow::Context;
use std::collections::BTreeSet;
use std::io;

/// SFTP filesystem backend.
pub struct SftpFs;

impl SftpFs {
    pub fn list(host: &Host, remote_path: &str) -> io::Result<Vec<Entry>> {
        let host = host.clone();
        let path = remote_path.to_string();

        // Transitional sync bridge for legacy Location::list() call sites.
        // Never call Handle::block_on() from the async TUI runtime: Tokio
        // rejects nested blocking and can panic. Keep the legacy API isolated
        // on its own runtime thread until directory loading is async end-to-end.
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| io::Error::other(format!("SFTP runtime: {error}")))?;
            runtime
                .block_on(list_sftp(&host, &path))
                .map_err(|error| io::Error::other(format!("SFTP: {error:#}")))
        })
        .join()
        .map_err(|_| io::Error::other("SFTP worker thread panicked"))?
    }
}

async fn list_sftp(host: &Host, remote_path: &str) -> anyhow::Result<Vec<Entry>> {
    let connection = crate::remote::openssh_sftp::OpenSshSftpConnection::connect(&host.ssh_alias)
        .await
        .with_context(|| format!("OpenSSH SFTP connect to {}", host.ssh_alias))?;

    let read_dir = connection
        .session
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
        result.push(Entry {
            name,
            kind,
            size: Some(metadata.len()),
        });
    }

    let _ = connection.close().await;

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

use crate::vfs::VfsProvider;

#[derive(Debug)]
pub struct SftpProvider {
    pub host: crate::remote::Host,
}

#[async_trait::async_trait]
impl VfsProvider for SftpProvider {
    fn list(&self, path: &str) -> std::io::Result<Vec<Entry>> {
        SftpFs::list(&self.host, path)
    }

    async fn list_async(&self, path: &str) -> std::io::Result<Vec<Entry>> {
        list_sftp(&self.host, path)
            .await
            .map_err(|error| std::io::Error::other(format!("SFTP: {error:#}")))
    }

    fn read_head(&self, _path: &str, _lines: usize) -> std::io::Result<Vec<String>> {
        Err(std::io::Error::other("SFTP read_head not supported"))
    }

    fn copy_files(&self, _src: &str, _dst: &str, _names: &[String]) -> std::io::Result<usize> {
        Err(std::io::Error::other("SFTP copy via transfer planner"))
    }

    fn move_files(&self, _src: &str, _dst: &str, _names: &[String]) -> std::io::Result<usize> {
        Err(std::io::Error::other("SFTP move via transfer planner"))
    }

    fn delete_files(&self, _dir: &str, _names: &[String]) -> std::io::Result<usize> {
        Err(std::io::Error::other("SFTP delete via transfer planner"))
    }
}
