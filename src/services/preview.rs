use std::path::Path;

use tokio::io::{AsyncBufReadExt, BufReader};

use crate::process::ProcessService;

pub struct PreviewService;

impl PreviewService {
    pub async fn preview(path: &Path) -> Vec<String> {
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_lowercase();
        let path_string = path.to_string_lossy().into_owned();

        if matches!(
            extension.as_str(),
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp"
        ) {
            let args = vec![
                "--symbols".into(),
                "block".into(),
                "--size".into(),
                "80x20".into(),
                path_string.clone(),
            ];
            if let Some(mut lines) = output_lines("chafa", &args).await {
                lines.insert(0, format!("[Image/Chafa] {}", path.display()));
                return lines;
            }
            return vec![format!(
                "[Image] {} (install chafa for inline preview)",
                path.display()
            )];
        }

        if extension == "pdf" {
            let args = vec!["-l".into(), "1".into(), path_string.clone(), "-".into()];
            if let Some(mut lines) = output_lines("pdftotext", &args).await {
                let count = lines.len();
                lines.insert(0, format!("[PDF] {} — {count} lines", path.display()));
                return lines;
            }
        }

        if matches!(
            extension.as_str(),
            "mp4" | "mkv" | "avi" | "mov" | "webm" | "mp3" | "flac"
        ) {
            let args = vec![
                "-hide_banner".into(),
                "-show_entries".into(),
                "format=duration,size,bit_rate:stream=codec_type,codec_name,width,height".into(),
                "-of".into(),
                "default=noprint_wrappers=1".into(),
                path_string.clone(),
            ];
            if let Some(mut lines) = output_lines("ffprobe", &args).await {
                lines.insert(0, format!("[Media] {}", path.display()));
                return lines;
            }
        }

        if matches!(
            extension.as_str(),
            "zip" | "tar" | "gz" | "xz" | "7z" | "rar"
        ) {
            let (program, args) = if matches!(extension.as_str(), "7z" | "rar" | "zip") {
                ("7z", vec!["l".into(), path_string.clone()])
            } else {
                ("tar", vec!["tvf".into(), path_string.clone()])
            };
            if let Some(mut lines) = output_lines(program, &args).await {
                lines.insert(0, format!("[Archive] {}", path.display()));
                return lines;
            }
        }

        let bat_args = vec![
            "--style=plain".into(),
            "--color=never".into(),
            "--paging=never".into(),
            "--line-range=:200".into(),
            path_string,
        ];
        if let Some(mut lines) = output_lines("bat", &bat_args).await {
            let total = tokio::fs::metadata(path)
                .await
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            lines.insert(0, format!("[Code] {} — {total} bytes", path.display()));
            return lines;
        }

        read_head(path, 500)
            .await
            .unwrap_or_else(|error| vec![format!("Error: {error}")])
    }
}

async fn output_lines(program: &str, args: &[String]) -> Option<Vec<String>> {
    let output = ProcessService::output(program, args, None).await.ok()?;
    if !output.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_string)
            .collect(),
    )
}

async fn read_head(path: &Path, max_lines: usize) -> std::io::Result<Vec<String>> {
    let file = tokio::fs::File::open(path).await?;
    let mut lines = BufReader::new(file).lines();
    let mut result = Vec::new();
    while result.len() < max_lines {
        match lines.next_line().await? {
            Some(line) => result.push(line),
            None => break,
        }
    }
    Ok(result)
}
