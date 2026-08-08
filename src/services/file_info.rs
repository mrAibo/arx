use std::path::Path;

use crate::process::ProcessService;
use crate::vfs::EntryKind;

pub struct FileInfoService;

impl FileInfoService {
    pub async fn directory_children_sizes(path: &Path) -> Vec<String> {
        let mut lines = vec!["Directory sizes:".into()];
        let mut read_dir = match tokio::fs::read_dir(path).await {
            Ok(read_dir) => read_dir,
            Err(error) => {
                lines.push(format!("Error: {error}"));
                return lines;
            }
        };

        let mut directories = Vec::new();
        while let Ok(Some(entry)) = read_dir.next_entry().await {
            if entry
                .file_type()
                .await
                .map(|kind| kind.is_dir())
                .unwrap_or(false)
            {
                directories.push((entry.file_name(), entry.path()));
            }
        }
        directories.sort_by(|left, right| left.0.cmp(&right.0));

        for (name, child) in directories {
            let args = vec!["-sh".into(), child.to_string_lossy().into_owned()];
            let size = ProcessService::output("du", &args, None)
                .await
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| {
                    String::from_utf8_lossy(&output.stdout)
                        .split_whitespace()
                        .next()
                        .map(str::to_string)
                })
                .unwrap_or_else(|| "?".into());
            lines.push(format!("  {size:>8}  {}", name.to_string_lossy()));
        }
        lines
    }

    pub async fn directory_summary(path: &Path) -> Vec<String> {
        let path_string = path.to_string_lossy().into_owned();
        let du_args = vec!["-sh".into(), path_string.clone()];
        let df_args = vec!["-h".into(), path_string];

        let (du, df) = tokio::join!(
            ProcessService::output("du", &du_args, None),
            ProcessService::output("df", &df_args, None),
        );

        let size = du
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .unwrap_or_else(|| "unavailable".into());
        let free = df
            .ok()
            .filter(|output| output.status.success())
            .map(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .last()
                    .unwrap_or_default()
                    .to_string()
            })
            .unwrap_or_else(|| "unavailable".into());

        vec![
            format!("Directory: {}", path.display()),
            format!("Size:     {size}"),
            format!("Free:     {free}"),
        ]
    }

    pub async fn file_hash_summary(path: &Path, size_label: &str) -> Vec<String> {
        let args = vec![path.to_string_lossy().into_owned()];
        let hash = ProcessService::output("sha256sum", &args, None)
            .await
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .split_whitespace()
                    .next()
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "unavailable".into());

        vec![
            format!("File: {}", path.display()),
            format!("Size: {size_label}"),
            format!("SHA256: {hash}"),
        ]
    }

    pub async fn metadata_summary(
        path: &Path,
        name: &str,
        kind: EntryKind,
        size_label: &str,
    ) -> std::io::Result<Vec<String>> {
        let metadata = tokio::fs::symlink_metadata(path).await?;
        let access = if metadata.permissions().readonly() {
            "read-only"
        } else {
            "read-write"
        };

        let mut lines = vec![
            format!("Name:      {name}"),
            format!("Path:      {}", path.display()),
            format!("Type:      {kind:?}"),
            format!("Size:      {size_label}"),
            format!("Access:    {access}"),
        ];

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            lines.push(format!("Mode:      {:04o}", metadata.mode() & 0o7777));
            lines.push(format!("UID:GID:   {}:{}", metadata.uid(), metadata.gid()));
        }

        Ok(lines)
    }
}
