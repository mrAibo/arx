use super::{
    BoundedRead, Entry, EntryIdentity, EntryKind, ListedEntry, Location, ProviderContinuation,
    ProviderListingPage,
};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;

/// Archive filesystem backend. Uses system `tar` / `unzip` for listing.
pub struct ArchiveFs;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveMemberRef {
    pub member_path: String,
}

impl ArchiveFs {
    fn validated_member(path: &str) -> io::Result<&str> {
        let safe = !path.is_empty()
            && Path::new(path)
                .components()
                .all(|part| matches!(part, Component::Normal(_)));
        if safe {
            Ok(path)
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "archive member path must be relative and traversal-free",
            ))
        }
    }

    fn read_prefix(archive: &Path, path: &str, max_bytes: usize) -> io::Result<BoundedRead> {
        let member = Self::validated_member(path)?;
        let is_zip = archive
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".zip"));
        let mut command = if is_zip {
            let mut command = std::process::Command::new("unzip");
            command.args(["-p", &archive.to_string_lossy(), member]);
            command
        } else {
            let mut command = std::process::Command::new("tar");
            command.args(["xOf", &archive.to_string_lossy(), "--", member]);
            command
        };
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let mut bytes = Vec::with_capacity(max_bytes.min(64 * 1024) + 1);
        child
            .stdout
            .take()
            .expect("piped archive stdout")
            .take(max_bytes.saturating_add(1) as u64)
            .read_to_end(&mut bytes)?;
        let truncated = bytes.len() > max_bytes;
        bytes.truncate(max_bytes);
        if truncated {
            let _ = child.kill();
        }
        let status = child.wait()?;
        if !truncated && !status.success() {
            return Err(io::Error::other("archive member extraction failed"));
        }
        Ok(BoundedRead {
            bytes,
            truncated,
            unix_mode: None,
            unix_uid: None,
            unix_gid: None,
        })
    }

    /// List entries inside an archive at `inner_path` (empty = root).
    /// Supported: .tar, .tar.gz, .tgz, .tar.bz2, .tar.xz, .zip.
    /// ponytail: shell-based listing; replace with libarchive when perf matters.
    pub fn list(archive: &Path, inner_path: &str) -> io::Result<Vec<Entry>> {
        let name = archive.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let is_zip = name.ends_with(".zip");

        let output = if is_zip {
            std::process::Command::new("unzip")
                .args(["-Z", "-1", &archive.to_string_lossy()])
                .output()
        } else {
            std::process::Command::new("tar")
                .args(["tf", &archive.to_string_lossy()])
                .output()
        };

        let stdout = match output {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
            _ => {
                return Err(io::Error::other(
                    "archive command failed — is tar/unzip installed?",
                ));
            }
        };

        let prefix = if inner_path.is_empty() {
            String::new()
        } else if inner_path.ends_with('/') {
            inner_path.to_string()
        } else {
            format!("{inner_path}/")
        };

        let mut seen = std::collections::BTreeSet::new();
        let mut result = Vec::new();

        for line in stdout.lines() {
            // `unzip -Z -1` and `tar tf` both emit one full member path per
            // line, preserving spaces and Unicode. No column parsing needed.
            let entry_path = line.trim();

            if entry_path.is_empty() || entry_path.ends_with('/') {
                continue; // skip dir markers, we'll infer dirs from paths
            }

            // Filter: only entries under the current inner_path
            if !prefix.is_empty() && !entry_path.starts_with(&prefix) {
                continue;
            }

            let relative = entry_path[prefix.len()..].to_string();

            // Only show immediate children
            if relative.contains('/') && !relative.ends_with('/') {
                // This file is in a subdirectory — skip file, but ensure the subdir exists
                let dir_name = relative.split('/').next().unwrap_or(&relative);
                if !seen.contains(dir_name) {
                    seen.insert(dir_name.to_string());
                    result.push(Entry {
                        name: dir_name.to_string(),
                        kind: EntryKind::Directory,
                        size: None,
                        modified_unix_ms: None,
                    });
                }
                continue;
            }

            if seen.insert(relative.clone()) {
                // Determine if it's a file or directory
                // ponytail: tar doesn't distinguish; assume all are files
                result.push(Entry {
                    name: relative,
                    kind: EntryKind::File,
                    size: None,
                    modified_unix_ms: None,
                });
            }
        }

        // Sort dirs first
        result.sort_by(|a, b| {
            match (
                a.kind == EntryKind::Directory,
                b.kind == EntryKind::Directory,
            ) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            }
        });

        Ok(result)
    }
}
/// Registry-backed archive provider. Each archive file is a distinct provider
/// instance; the path argument is the inner archive path only.
#[derive(Debug)]
pub struct ArchiveProvider {
    pub archive: PathBuf,
}

#[async_trait::async_trait]
impl super::VfsProvider for ArchiveProvider {
    fn list(&self, path: &str) -> io::Result<Vec<Entry>> {
        ArchiveFs::list(&self.archive, path)
    }

    async fn list_async(&self, path: &str) -> io::Result<Vec<Entry>> {
        let archive = self.archive.clone();
        let path = path.to_string();
        tokio::task::spawn_blocking(move || ArchiveFs::list(&archive, &path))
            .await
            .map_err(|error| io::Error::other(format!("archive worker failed: {error}")))?
    }

    async fn list_page(
        &self,
        location: &Location,
        continuation: Option<&ProviderContinuation>,
    ) -> io::Result<ProviderListingPage> {
        if continuation.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "archive listing is not paginated",
            ));
        }
        let Location::Archive { inner_path, .. } = location else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "archive provider requires an archive location",
            ));
        };
        let prefix = inner_path.trim_matches('/');
        let entries = self.list_async(inner_path).await?;
        Ok(ProviderListingPage {
            entries: entries
                .into_iter()
                .map(|entry| {
                    let member_path = if prefix.is_empty() {
                        entry.name.clone()
                    } else {
                        format!("{prefix}/{}", entry.name)
                    };
                    ListedEntry {
                        entry,
                        identity: EntryIdentity::ArchiveMember(ArchiveMemberRef { member_path }),
                    }
                })
                .collect(),
            continuation: None,
        })
    }

    async fn read_prefix_bytes(&self, path: &str, max_bytes: usize) -> io::Result<BoundedRead> {
        let archive = self.archive.clone();
        let path = path.to_string();
        tokio::task::spawn_blocking(move || ArchiveFs::read_prefix(&archive, &path, max_bytes))
            .await
            .map_err(|error| io::Error::other(format!("archive worker failed: {error}")))?
    }

    async fn read_listed_prefix_bytes(
        &self,
        _location: &Location,
        listed: &ListedEntry,
        max_bytes: usize,
    ) -> io::Result<BoundedRead> {
        let EntryIdentity::ArchiveMember(member) = &listed.identity else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "archive preview requires exact member identity",
            ));
        };
        self.read_prefix_bytes(&member.member_path, max_bytes).await
    }

    fn read_head(&self, _path: &str, _lines: usize) -> io::Result<Vec<String>> {
        Err(io::Error::other("archive read_head not implemented"))
    }

    fn copy_files(&self, _src: &str, _dst: &str, _names: &[String]) -> io::Result<usize> {
        Err(io::Error::other(
            "archive copy is handled by transfer/extract services",
        ))
    }

    fn move_files(&self, _src: &str, _dst: &str, _names: &[String]) -> io::Result<usize> {
        Err(io::Error::other("archive move is unsupported"))
    }

    fn delete_files(&self, _dir: &str, _names: &[String]) -> io::Result<usize> {
        Err(io::Error::other("archive delete is unsupported"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::VfsProvider;

    #[tokio::test]
    async fn bounded_preview_reads_archive_member_and_reports_truncation() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("README.md");
        std::fs::write(&source, b"archive preview content").unwrap();
        let archive = temp.path().join("fixture.tar");
        assert!(
            std::process::Command::new("tar")
                .args(["cf", archive.to_str().unwrap(), "README.md"])
                .current_dir(temp.path())
                .status()
                .unwrap()
                .success()
        );

        let provider = ArchiveProvider { archive };
        let read = provider.read_prefix_bytes("README.md", 7).await.unwrap();
        assert_eq!(read.bytes, b"archive");
        assert!(read.truncated);
    }

    #[tokio::test]
    async fn zip_list_preserves_member_names_with_spaces() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("file with spaces.txt");
        std::fs::write(&source, b"payload").unwrap();
        let archive = temp.path().join("fixture.zip");
        assert!(
            std::process::Command::new("zip")
                .args([archive.to_str().unwrap(), "file with spaces.txt"])
                .current_dir(temp.path())
                .status()
                .unwrap()
                .success()
        );

        let entries = ArchiveFs::list(&archive, "").unwrap();
        assert!(entries
            .iter()
            .any(|e| e.name == "file with spaces.txt"));
    }
}
