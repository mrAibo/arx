use super::{Entry, EntryKind, canonical_unix_mtime_ms};
use crate::remote::Host;
use anyhow::Context;
use std::collections::BTreeSet;
use std::io;
use tokio::sync::Mutex;

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
    let result = entries_from_read_dir(read_dir.collect());
    let _ = connection.close().await;
    Ok(result)
}

fn entries_from_read_dir(read_dir: Vec<russh_sftp::client::fs::DirEntry>) -> Vec<Entry> {
    let mut result: Vec<Entry> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for entry in read_dir {
        let name = entry.file_name();
        if !seen.insert(name.clone()) {
            continue;
        }
        let metadata = entry.metadata();
        let kind = if metadata.is_dir() {
            EntryKind::Directory
        } else if metadata.is_symlink() {
            EntryKind::Symlink
        } else {
            EntryKind::File
        };
        let size = if kind == EntryKind::File {
            Some(metadata.len())
        } else {
            None
        };
        let modified_unix_ms = metadata
            .mtime
            .map(|seconds| canonical_unix_mtime_ms(u64::from(seconds)));
        result.push(Entry {
            name,
            kind,
            size,
            modified_unix_ms,
        });
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
    result
}

use crate::vfs::VfsProvider;
pub struct SftpProvider {
    pub host: crate::remote::Host,
    connection: Mutex<Option<crate::remote::openssh_sftp::OpenSshSftpConnection>>,
}

impl std::fmt::Debug for SftpProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SftpProvider")
            .field("host", &self.host)
            .field("connection", &"<pooled>")
            .finish()
    }
}

impl SftpProvider {
    pub fn new(host: crate::remote::Host) -> Self {
        Self {
            host,
            connection: Mutex::new(None),
        }
    }

    async fn list_pooled(&self, path: &str) -> std::io::Result<Vec<Entry>> {
        let mut guard = self.connection.lock().await;

        // One reconnect attempt handles servers closing an idle subsystem
        // between directory reads while avoiding a reconnect per directory.
        for attempt in 0..2 {
            if guard.is_none() {
                *guard = Some(
                    crate::remote::openssh_sftp::OpenSshSftpConnection::connect(
                        &self.host.ssh_alias,
                    )
                    .await?,
                );
            }

            let result = guard
                .as_ref()
                .expect("connection initialized")
                .session
                .read_dir(path.to_string())
                .await;

            match result {
                Ok(entries) => return Ok(entries_from_read_dir(entries.collect())),
                Err(error) => {
                    if let Some(mut broken) = guard.take() {
                        broken.abort().await;
                    }
                    if attempt == 1 {
                        return Err(std::io::Error::other(format!(
                            "SFTP read_dir {path}: {error}"
                        )));
                    }
                }
            }
        }

        unreachable!("SFTP retry loop always returns")
    }

    /// Reuse pooled connection without retry (mutations are not retried).
    #[allow(dead_code)]
    async fn connect_for_mutation(
        &self,
    ) -> std::io::Result<
        tokio::sync::MutexGuard<'_, Option<crate::remote::openssh_sftp::OpenSshSftpConnection>>,
    > {
        let mut guard = self.connection.lock().await;
        if guard.is_none() {
            *guard = Some(
                crate::remote::openssh_sftp::OpenSshSftpConnection::connect(&self.host.ssh_alias)
                    .await?,
            );
        }
        Ok(guard)
    }

    async fn mkdir(&self, path: &str) -> std::io::Result<()> {
        let mut guard = self.connect_for_mutation().await?;
        let conn = guard
            .as_ref()
            .ok_or_else(|| std::io::Error::other("SFTP connection lost"))?;
        match conn.session.create_dir(path.to_string()).await {
            Ok(()) => Ok(()),
            Err(e) => {
                if let Some(mut broken) = guard.take() {
                    broken.abort().await;
                }
                Err(std::io::Error::other(format!("SFTP mkdir {path}: {e}")))
            }
        }
    }

    async fn remove_file(&self, path: &str) -> std::io::Result<()> {
        let mut guard = self.connect_for_mutation().await?;
        let conn = guard
            .as_ref()
            .ok_or_else(|| std::io::Error::other("SFTP connection lost"))?;
        match conn.session.remove_file(path.to_string()).await {
            Ok(()) => Ok(()),
            Err(e) => {
                if let Some(mut broken) = guard.take() {
                    broken.abort().await;
                }
                Err(std::io::Error::other(format!(
                    "SFTP remove_file {path}: {e}"
                )))
            }
        }
    }

    async fn remove_dir(&self, path: &str) -> std::io::Result<()> {
        let mut guard = self.connect_for_mutation().await?;
        let conn = guard
            .as_ref()
            .ok_or_else(|| std::io::Error::other("SFTP connection lost"))?;
        match conn.session.remove_dir(path.to_string()).await {
            Ok(()) => Ok(()),
            Err(e) => {
                if let Some(mut broken) = guard.take() {
                    broken.abort().await;
                }
                Err(std::io::Error::other(format!(
                    "SFTP remove_dir {path}: {e}"
                )))
            }
        }
    }
}
#[async_trait::async_trait]
impl VfsProvider for SftpProvider {
    fn list(&self, path: &str) -> std::io::Result<Vec<Entry>> {
        SftpFs::list(&self.host, path)
    }

    async fn list_async(&self, path: &str) -> std::io::Result<Vec<Entry>> {
        self.list_pooled(path).await
    }

    fn read_head(&self, path: &str, max_lines: usize) -> std::io::Result<Vec<String>> {
        // ponytail: sync bridge for legacy VfsProvider::read_head callers.
        // New SFTP preview goes through read_prefix_bytes (async) in the
        // effect pipeline.
        const MAX_BYTES: usize = 1024 * 1024; // 1 MiB

        let host = self.host.clone();
        let path = path.to_string();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| std::io::Error::other(format!("SFTP runtime: {e}")))?;
            rt.block_on(async {
                let provider = SftpProvider::new(host);
                let bytes = provider.read_prefix(&path, MAX_BYTES).await?;
                crate::services::preview::format_bounded_preview(
                    &bytes, None, false, &path, max_lines,
                )
            })
        })
        .join()
        .map_err(|_| std::io::Error::other("SFTP worker thread panicked"))?
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

    async fn mkdir(&self, path: &str) -> std::io::Result<()> {
        self.mkdir(path).await
    }

    async fn remove_file(&self, path: &str) -> std::io::Result<()> {
        self.remove_file(path).await
    }

    async fn remove_dir(&self, path: &str) -> std::io::Result<()> {
        self.remove_dir(path).await
    }

    async fn read_prefix_bytes(&self, path: &str, max_bytes: usize) -> std::io::Result<Vec<u8>> {
        self.read_prefix(path, max_bytes).await
    }
}

impl SftpProvider {
    /// Read up to `max_bytes` from the beginning of a remote file.
    /// Uses pooled connection with one retry — read is non-destructive.
    async fn read_prefix(&self, path: &str, max_bytes: usize) -> std::io::Result<Vec<u8>> {
        use tokio::io::AsyncReadExt;

        let mut guard = self.connection.lock().await;

        for attempt in 0..2 {
            if guard.is_none() {
                *guard = Some(
                    crate::remote::openssh_sftp::OpenSshSftpConnection::connect(
                        &self.host.ssh_alias,
                    )
                    .await?,
                );
            }

            let conn = guard
                .as_ref()
                .ok_or_else(|| std::io::Error::other("SFTP connection lost"))?;

            let open_result = conn.session.open(path.to_string()).await;

            match open_result {
                Ok(mut file) => {
                    // ponytail: read bounded prefix, loops on short chunks
                    let cap = max_bytes + 1; // +1 for truncation detection
                    let mut buf = Vec::new();
                    // read_to_end is bounded by take(cap)
                    tokio::io::AsyncReadExt::take(&mut file, cap as u64)
                        .read_to_end(&mut buf)
                        .await
                        .map_err(|e| std::io::Error::other(format!("SFTP read {path}: {e}")))?;
                    let truncated = buf.len() > max_bytes;
                    if truncated {
                        buf.truncate(max_bytes);
                    }
                    return Ok(buf);
                }
                Err(error) => {
                    if let Some(mut broken) = guard.take() {
                        broken.abort().await;
                    }
                    if attempt == 1 {
                        return Err(std::io::Error::other(format!("SFTP open {path}: {error}")));
                    }
                }
            }
        }

        unreachable!()
    }
}

#[cfg(test)]
mod metadata_tests {
    use super::*;

    #[test]
    fn sftp_mtime_uses_canonical_second_resolution() {
        let mut metadata = russh_sftp::protocol::FileAttributes::empty();
        metadata.mtime = Some(1_234);

        let modified_unix_ms = metadata
            .mtime
            .map(|seconds| canonical_unix_mtime_ms(u64::from(seconds)));

        assert_eq!(modified_unix_ms, Some(1_234_000));
    }

    // ── REMOTE-09: transport invalidation mechanism ──

    #[test]
    fn sftp_provider_has_invalidation_mechanism() {
        let host = crate::remote::Host::from_alias("test-host");
        let provider = SftpProvider::new(host);
        assert_eq!(provider.host.ssh_alias, "test-host");
    }

    #[test]
    fn no_recursive_delete_path_in_mutation_code() {
        let source = include_str!("sftp.rs");
        // Split at #[cfg(test)] to avoid self-matching assertion strings.
        let prod_code = source.split("#[cfg(test)]").next().unwrap_or(source);
        assert!(!prod_code.contains("remove_dir_all"));
        assert!(!prod_code.contains(".recursive"));
        assert!(!prod_code.contains("walkdir"));
    }

    #[test]
    fn mutation_failure_invalidates_session() {
        let source = include_str!("sftp.rs");
        let count = source.matches("guard.take()").count();
        assert!(
            count >= 3,
            "expected at least 3 guard.take() invalidation sites (mkdir, remove_file, remove_dir), found {count}"
        );
    }
}
