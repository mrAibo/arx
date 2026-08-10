use std::path::Path;

use tokio::io::AsyncReadExt;

use crate::process::ProcessService;

pub struct PreviewService;

/// Pure function: format bounded bytes into preview lines.
/// - NUL byte → "[Binary preview disabled]"
/// - Invalid UTF-8 → "[Binary preview disabled]"
/// - Valid UTF-8 → lines capped at max_lines, with truncation marker if truncated.
///   `total_size` may be None for remote files where metadata may not include size.
pub fn format_bounded_preview(
    bytes: &[u8],
    total_size: Option<u64>,
    truncated: bool,
    display_name: &str,
    max_lines: usize,
) -> std::io::Result<Vec<String>> {
    if bytes.contains(&0) {
        return Ok(vec![format!("[Binary preview disabled] {}", display_name)]);
    }

    let text = match std::str::from_utf8(bytes) {
        Ok(t) => t,
        Err(e) if truncated && e.error_len().is_none() => {
            // Only allow partial prefix when truncated at multi-byte boundary
            std::str::from_utf8(&bytes[..e.valid_up_to()]).map_err(|_| {
                std::io::Error::other("UTF-8 decode failed after boundary truncation")
            })?
        }
        Err(_) => {
            // Invalid UTF-8 not caused by truncation → binary
            return Ok(vec![format!("[Binary preview disabled] {display_name}")]);
        }
    };

    let mut source_lines = text.lines();
    let mut lines: Vec<String> = source_lines
        .by_ref()
        .take(max_lines)
        .map(str::to_string)
        .collect();
    let lines_truncated = source_lines.next().is_some();

    let bytes_label = match total_size {
        Some(sz) => format!("{} bytes", sz),
        None => format!("{} bytes read", bytes.len()),
    };
    lines.insert(0, format!("[Remote Text] {display_name} — {bytes_label}"));

    if truncated || lines_truncated {
        let mut parts = Vec::new();
        if truncated {
            parts.push(format!("{} bytes", MAX_TEXT_PREVIEW_BYTES));
        }
        if lines_truncated {
            parts.push(format!("{} lines", MAX_TEXT_PREVIEW_LINES));
        }
        lines.push(format!("[Truncated at {}]", parts.join(" / ")));
    }

    Ok(lines)
}

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

pub const MAX_TEXT_PREVIEW_BYTES: usize = 1024 * 1024;
pub const MAX_TEXT_PREVIEW_LINES: usize = 500;

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

    // ── format_bounded_preview unit tests ──

    #[test]
    fn format_bounded_preview_small_utf8() {
        let bytes = b"hello\nworld\n";
        let lines = format_bounded_preview(bytes, Some(13), false, "test.txt", 500).unwrap();

        assert!(lines[0].contains("[Remote Text]"));
        assert!(lines.iter().any(|l| l == "hello"));
        assert!(lines.iter().any(|l| l == "world"));
        assert!(!lines.iter().any(|l| l.contains("Truncated")));
    }

    #[test]
    fn format_bounded_preview_binary_nul() {
        let bytes = &[b'a', 0, b'b'];
        let lines = format_bounded_preview(bytes, Some(3), false, "data.bin", 500).unwrap();

        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("Binary preview disabled"));
    }

    #[test]
    fn format_bounded_preview_invalid_utf8() {
        // 0xC0 is an invalid start byte not at a truncation boundary
        let bytes = &[b'h', b'e', b'l', 0xC0, b'o'];
        let lines = format_bounded_preview(bytes, Some(5), false, "broken.txt", 500).unwrap();

        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("Binary preview disabled"));
        assert!(!lines.iter().any(|l| l == "hel"));
    }

    #[test]
    fn format_bounded_preview_empty() {
        let lines = format_bounded_preview(b"", Some(0), false, "empty.txt", 500).unwrap();

        assert!(lines[0].contains("[Remote Text]"));
        assert_eq!(lines.len(), 1); // header only, no content lines, no truncation
    }

    #[test]
    fn format_bounded_preview_truncated_lines() {
        let content = (0..600)
            .map(|i| format!("line-{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let bytes = content.as_bytes();
        let lines = format_bounded_preview(bytes, Some(bytes.len() as u64), false, "long.txt", 500)
            .unwrap();

        assert!(lines[0].contains("[Remote Text]"));
        assert!(
            lines
                .last()
                .is_some_and(|l| l.contains("Truncated at 500 lines"))
        );
    }

    // ── VIEW-09B: bounds ──

    #[test]
    fn format_bounded_preview_1_byte() {
        let bytes = b"x";
        let lines = format_bounded_preview(bytes, Some(1), false, "one.txt", 500).unwrap();
        assert!(lines[0].contains("[Remote Text]"));
        assert!(lines.iter().any(|l| l == "x"));
        assert_eq!(lines.len(), 2); // header + one line
    }

    #[test]
    fn format_bounded_preview_max_minus_1() {
        let content = "a".repeat(MAX_TEXT_PREVIEW_BYTES - 1);
        let bytes = content.as_bytes();
        let lines =
            format_bounded_preview(bytes, Some(bytes.len() as u64), false, "near_max.txt", 500)
                .unwrap();
        assert!(lines[0].contains("[Remote Text]"));
        assert!(!lines.iter().any(|l| l.contains("Truncated")));
    }

    #[test]
    fn format_bounded_preview_exactly_max() {
        let content = "a".repeat(MAX_TEXT_PREVIEW_BYTES);
        let bytes = content.as_bytes();
        let lines = format_bounded_preview(
            bytes,
            Some(MAX_TEXT_PREVIEW_BYTES as u64),
            false,
            "exact_max.txt",
            500,
        )
        .unwrap();
        assert!(lines[0].contains("[Remote Text]"));
        assert!(!lines.iter().any(|l| l.contains("Truncated")));
    }

    #[test]
    fn format_bounded_preview_max_plus_1() {
        let content = "a".repeat(MAX_TEXT_PREVIEW_BYTES + 1);
        let mut bytes = content.into_bytes();
        bytes.truncate(MAX_TEXT_PREVIEW_BYTES);
        let lines = format_bounded_preview(
            &bytes,
            Some((MAX_TEXT_PREVIEW_BYTES + 1) as u64),
            true,
            "over_max.txt",
            500,
        )
        .unwrap();
        assert!(lines[0].contains("[Remote Text]"));
        assert!(lines.iter().any(|l| l.contains("Truncated")));
    }

    #[test]
    fn format_bounded_preview_short_chunks() {
        let full = "line1\nline2\nline3\n";
        let bytes = full.as_bytes();
        let mut assembled = Vec::new();
        for chunk in bytes.chunks(3) {
            assembled.extend_from_slice(chunk);
        }
        let lines = format_bounded_preview(
            &assembled,
            Some(bytes.len() as u64),
            false,
            "chunked.txt",
            500,
        )
        .unwrap();
        assert!(lines.iter().any(|l| l == "line1"));
        assert!(lines.iter().any(|l| l == "line2"));
        assert!(lines.iter().any(|l| l == "line3"));
    }

    // ── VIEW-09B: UTF-8 ──

    #[test]
    fn format_bounded_preview_unicode_content() {
        let text = "héllo wörld — café\nline two\n";
        let bytes = text.as_bytes();
        let lines =
            format_bounded_preview(bytes, Some(bytes.len() as u64), false, "unicode.txt", 500)
                .unwrap();
        assert!(lines.iter().any(|l| l.contains("héllo")));
        assert!(lines.iter().any(|l| l.contains("café")));
    }

    #[test]
    fn format_bounded_preview_unicode_display_name() {
        let bytes = b"hello\n";
        let lines = format_bounded_preview(bytes, Some(6), false, "日本語.txt", 500).unwrap();
        assert!(lines[0].contains("日本語.txt"));
    }

    #[test]
    fn format_bounded_preview_truncated_multibyte_boundary() {
        // "a" (0x61) + truncated start of 2-byte "é" (0xC3 0xA9)
        // Cutting after 0xC3 → valid_up_to = 1, error_len = None
        let bytes = &[0x61, 0xC3];
        let lines = format_bounded_preview(bytes, Some(2), true, "cut.txt", 500).unwrap();
        assert!(lines.iter().any(|l| l == "a"));
        assert!(lines.iter().any(|l| l.contains("Truncated")));
    }

    #[test]
    fn format_bounded_preview_invalid_utf8_mid_stream_not_truncated() {
        // 0xFF is never valid UTF-8 — even mid-stream, not boundary-truncated
        let bytes = &[b'h', b'i', 0xFF, b'!'];
        let lines = format_bounded_preview(bytes, Some(4), false, "broken.txt", 500).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("Binary preview disabled"));
    }

    // ── VIEW-09B: truncation ──

    #[test]
    fn format_bounded_preview_500_lines_no_marker() {
        let content = (0..500)
            .map(|i| format!("line-{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let bytes = content.as_bytes();
        let lines = format_bounded_preview(
            bytes,
            Some(bytes.len() as u64),
            false,
            "exact_500.txt",
            MAX_TEXT_PREVIEW_LINES,
        )
        .unwrap();
        assert!(!lines.iter().any(|l| l.contains("Truncated")));
    }

    #[test]
    fn format_bounded_preview_501_lines_marker() {
        let content = (0..501)
            .map(|i| format!("line-{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let bytes = content.as_bytes();
        let lines = format_bounded_preview(
            bytes,
            Some(bytes.len() as u64),
            false,
            "over_500.txt",
            MAX_TEXT_PREVIEW_LINES,
        )
        .unwrap();
        assert!(lines.iter().any(|l| l.contains("Truncated at 500 lines")));
    }

    #[test]
    fn format_bounded_preview_1mib_exact_not_truncated() {
        let content = "x".repeat(MAX_TEXT_PREVIEW_BYTES);
        let bytes = content.as_bytes();
        let lines = format_bounded_preview(
            bytes,
            Some(MAX_TEXT_PREVIEW_BYTES as u64),
            false,
            "1mib_exact.txt",
            500,
        )
        .unwrap();
        assert!(!lines.iter().any(|l| l.contains("Truncated")));
    }

    #[test]
    fn format_bounded_preview_over_1mib_truncated() {
        let content = "x".repeat(MAX_TEXT_PREVIEW_BYTES + 100);
        let mut bytes = content.into_bytes();
        bytes.truncate(MAX_TEXT_PREVIEW_BYTES);
        let lines = format_bounded_preview(
            &bytes,
            Some((MAX_TEXT_PREVIEW_BYTES + 100) as u64),
            true,
            "over_1mib.txt",
            500,
        )
        .unwrap();
        assert!(lines.iter().any(|l| l.contains("Truncated")));
    }
}
