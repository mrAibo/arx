use super::{Entry, EntryKind};
use std::io;
use std::path::Path;

/// Archive filesystem backend. Uses system `tar` / `unzip` for listing.
pub struct ArchiveFs;

impl ArchiveFs {
    /// List entries inside an archive at `inner_path` (empty = root).
    /// Supported: .tar, .tar.gz, .tgz, .tar.bz2, .tar.xz, .zip.
    /// ponytail: shell-based listing; replace with libarchive when perf matters.
    pub fn list(archive: &Path, inner_path: &str) -> io::Result<Vec<Entry>> {
        let name = archive.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let is_zip = name.ends_with(".zip");

        let output = if is_zip {
            std::process::Command::new("unzip")
                .args(["-l", &archive.to_string_lossy()])
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
            let entry_path = if is_zip {
                // unzip -l format: "  Length   Date   Time   Name"
                // Skip header/footer lines
                if line.starts_with("  Length")
                    || line.starts_with(" -------")
                    || !line.starts_with(' ')
                {
                    continue;
                }
                // Path is after the last space-padded column group
                let parts: Vec<&str> = line.split_whitespace().collect();
                // unzip -l output: Length Method Size Cmpr Date Time CRC-32 Name
                // Path is the last field
                if parts.len() < 5 {
                    continue;
                }
                parts.last().copied().unwrap_or("")
            } else {
                // tar tf: path per line
                line.trim().to_string();
                line.trim()
            };

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
