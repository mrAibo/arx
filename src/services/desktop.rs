use std::path::Path;
use std::process::Stdio;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::process::ProcessService;

pub struct DesktopService;

impl DesktopService {
    pub async fn open_path(path: &Path) -> std::io::Result<()> {
        let args = vec![path.to_string_lossy().into_owned()];
        ProcessService::status("xdg-open", &args, None)
            .await
            .map(|_| ())
    }

    pub async fn run_interactive_shell(program: &str) -> std::io::Result<()> {
        ProcessService::status(program, &[], None).await.map(|_| ())
    }

    pub async fn notify(title: &str, body: &str) {
        let args = vec![title.to_string(), body.to_string()];
        let _ = ProcessService::status("notify-send", &args, None).await;
    }

    pub async fn open_editor(editor: &str, path: &Path) -> std::io::Result<()> {
        let args = vec![path.to_string_lossy().into_owned()];
        ProcessService::status(editor, &args, None)
            .await
            .map(|_| ())
    }

    pub async fn page_with_bat(path: &Path) -> std::io::Result<()> {
        let args = vec![
            "--paging=always".into(),
            path.to_string_lossy().into_owned(),
        ];
        ProcessService::status("bat", &args, None).await.map(|_| ())
    }

    /// Copy without shell interpolation. This fixes quoting/injection bugs in
    /// the old `sh -c "echo ... | xclip || ..."` path.
    pub async fn copy_to_clipboard(text: &str) -> std::io::Result<()> {
        for (program, args) in [
            ("wl-copy", Vec::<String>::new()),
            ("xclip", vec!["-selection".into(), "clipboard".into()]),
        ] {
            let mut child = match Command::new(program)
                .args(&args)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(child) => child,
                Err(_) => continue,
            };

            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(text.as_bytes()).await?;
            }
            if child.wait().await?.success() {
                return Ok(());
            }
        }

        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no supported clipboard tool found (wl-copy/xclip)",
        ))
    }
}
