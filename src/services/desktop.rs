use std::path::Path;
use std::process::Stdio;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::process::ProcessService;

pub struct DesktopService;

impl DesktopService {
    pub fn resolve_editor(configured: Option<&str>) -> Option<String> {
        let visual = std::env::var("VISUAL").ok();
        let editor = std::env::var("EDITOR").ok();
        choose_editor(configured, visual.as_deref(), editor.as_deref())
    }

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
        let (program, args) = editor_argv(editor, path)?;
        let status = ProcessService::status(&program, &args, None).await?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!(
                "editor exited with {status}"
            )))
        }
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

fn choose_editor(
    configured: Option<&str>,
    visual: Option<&str>,
    editor: Option<&str>,
) -> Option<String> {
    [configured, visual, editor]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(str::to_owned)
}

fn parse_editor_command(spec: &str) -> std::io::Result<(String, Vec<String>)> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut token_started = false;

    for ch in spec.chars() {
        if escaped {
            token.push(ch);
            escaped = false;
            token_started = true;
            continue;
        }

        match quote {
            Some('\'') if ch == '\'' => quote = None,
            Some('\'') => token.push(ch),
            Some('"') if ch == '"' => quote = None,
            Some('"') if ch == '\\' => escaped = true,
            Some('"') => token.push(ch),
            Some(_) => unreachable!(),
            None if ch.is_whitespace() => {
                if token_started {
                    tokens.push(std::mem::take(&mut token));
                    token_started = false;
                }
            }
            None if matches!(ch, '\'' | '"') => {
                quote = Some(ch);
                token_started = true;
            }
            None if ch == '\\' => {
                escaped = true;
                token_started = true;
            }
            None => {
                token.push(ch);
                token_started = true;
            }
        }
    }

    if escaped || quote.is_some() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "editor command has an unfinished quote or escape",
        ));
    }
    if token_started {
        tokens.push(token);
    }

    let mut tokens = tokens.into_iter();
    let program = tokens
        .next()
        .filter(|program| !program.is_empty())
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "editor command is empty")
        })?;
    Ok((program, tokens.collect()))
}

fn editor_argv(editor: &str, path: &Path) -> std::io::Result<(String, Vec<String>)> {
    let (program, mut args) = parse_editor_command(editor)?;
    args.push(path.to_string_lossy().into_owned());
    Ok((program, args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_resolution_is_config_then_visual_then_editor_without_fallback() {
        assert_eq!(
            choose_editor(Some(" nvim "), Some("code --wait"), Some("vim")),
            Some("nvim".into())
        );
        assert_eq!(
            choose_editor(Some(""), Some("code --wait"), Some("vim")),
            Some("code --wait".into())
        );
        assert_eq!(
            choose_editor(Some(" "), Some(""), Some("vim")),
            Some("vim".into())
        );
        assert_eq!(choose_editor(Some(""), Some(" "), None), None);
    }

    #[test]
    fn editor_args_are_parsed_and_filename_stays_one_argument() {
        let path = Path::new("/tmp/note; touch NEVER");
        let (program, args) = editor_argv(
            r#""/opt/Visual Studio Code/code" --wait --reuse-window"#,
            path,
        )
        .unwrap();

        assert_eq!(program, "/opt/Visual Studio Code/code");
        assert_eq!(args, ["--wait", "--reuse-window", "/tmp/note; touch NEVER"]);
    }

    #[test]
    fn malformed_editor_command_is_rejected() {
        assert!(parse_editor_command("code 'unfinished").is_err());
        assert!(parse_editor_command(" ").is_err());
        assert!(parse_editor_command(r#""" --wait"#).is_err());
    }

    #[tokio::test]
    async fn editor_nonzero_exit_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.txt");
        tokio::fs::write(&path, "text").await.unwrap();

        let error = DesktopService::open_editor("false", &path)
            .await
            .unwrap_err();

        assert!(error.to_string().contains("editor exited with"));
    }
}
