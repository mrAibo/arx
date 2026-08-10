use std::path::Path;

use tokio::io::AsyncReadExt;

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

        read_text_preview(path)
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

const MAX_TEXT_PREVIEW_BYTES: usize = 1024 * 1024;
const MAX_TEXT_PREVIEW_LINES: usize = 500;

async fn read_text_preview(path: &Path) -> std::io::Result<Vec<String>> {
    let file = tokio::fs::File::open(path).await?;
    let total_bytes = file.metadata().await?.len();
    let mut limited = file.take((MAX_TEXT_PREVIEW_BYTES + 1) as u64);
    let mut bytes = Vec::new();
    limited.read_to_end(&mut bytes).await?;

    let bytes_truncated = bytes.len() > MAX_TEXT_PREVIEW_BYTES;
    bytes.truncate(MAX_TEXT_PREVIEW_BYTES);
    if bytes.contains(&0) {
        return Ok(vec![format!(
            "[Binary preview disabled] {}",
            path.display()
        )]);
    }

    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text,
        Err(error) if bytes_truncated && error.error_len().is_none() => {
            std::str::from_utf8(&bytes[..error.valid_up_to()]).expect("valid UTF-8 prefix")
        }
        Err(_) => {
            return Ok(vec![format!(
                "[Binary preview disabled] {}",
                path.display()
            )]);
        }
    };

    let mut source_lines = text.lines();
    let mut lines = source_lines
        .by_ref()
        .take(MAX_TEXT_PREVIEW_LINES)
        .map(str::to_string)
        .collect::<Vec<_>>();
    let lines_truncated = source_lines.next().is_some();
    lines.insert(
        0,
        format!("[Text] {} — {total_bytes} bytes", path.display()),
    );
    if bytes_truncated || lines_truncated {
        lines.push(format!(
            "[Truncated at {} bytes / {} lines]",
            MAX_TEXT_PREVIEW_BYTES, MAX_TEXT_PREVIEW_LINES
        ));
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn previews_small_utf8_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hello.txt");
        tokio::fs::write(&path, "hello\nworld\n").await.unwrap();

        let lines = PreviewService::preview(&path).await;

        assert!(lines[0].starts_with("[Text]"));
        assert!(lines.iter().any(|line| line == "hello"));
        assert!(lines.iter().any(|line| line == "world"));
    }

    #[tokio::test]
    async fn rejects_binary_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("binary.txt");
        tokio::fs::write(&path, [b'a', 0, b'b']).await.unwrap();

        let lines = PreviewService::preview(&path).await;

        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("Binary preview disabled"));
    }

    #[tokio::test]
    async fn truncates_large_text_with_an_explicit_marker() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.txt");
        let content = (0..600)
            .map(|line| format!("line-{line}-{}", "x".repeat(2048)))
            .collect::<Vec<_>>()
            .join("\n");
        tokio::fs::write(&path, content).await.unwrap();

        let lines = PreviewService::preview(&path).await;

        assert!(lines.len() <= 502);
        assert!(lines.last().is_some_and(|line| line.contains("Truncated")));
    }
}
