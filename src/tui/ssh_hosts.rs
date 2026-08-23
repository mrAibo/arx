use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use arx::app::AppState;
use arx::services::DesktopService;

use super::*;

const SSH_FORM_LABELS: [&str; 7] = [
    "Alias",
    "HostName",
    "User",
    "Port",
    "IdentityFile",
    "ProxyJump",
    "IdentitiesOnly (yes/no)",
];

fn render_ssh_host_form(
    frame: &mut ratatui::Frame,
    area: Rect,
    form: &arx::app::SshHostForm,
    state: &AppState,
) {
    use ratatui::layout::Constraint;
    let popup_area = centered_rect(70, 80, area);
    frame.render_widget(Clear, popup_area);

    let mode = match form.mode {
        arx::app::SshHostFormMode::Add => "Add SSH Host",
        arx::app::SshHostFormMode::Edit => "Edit SSH Host",
    };
    let title =
        format!(" {mode} (S: save | C: cancel | T: test | Ctrl+K: generate key | Esc: close) ");

    let mut lines: Vec<Line> = Vec::with_capacity(SSH_FORM_LABELS.len() + 4);
    for (i, label) in SSH_FORM_LABELS.iter().enumerate() {
        let cursor = if i == form.focus { ">" } else { " " };
        let val = &form.fields[i];
        let val = if val.is_empty() { "<empty>" } else { val };
        let style = if i == form.focus {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{cursor} {label:<22}: "), style),
            Span::styled(val.to_string(), style),
        ]));
    }

    if let Some(err) = &form.error {
        lines.push(Line::from(Span::styled(
            format!("  ERROR: {err}"),
            Style::default().fg(Color::Red),
        )));
    }

    if form.confirm_generate {
        lines.push(Line::from(Span::styled(
            "  Generate UNENCRYPTED Ed25519 key? (y/n) — private key has NO passphrase and stays on disk only.",
            Style::default().fg(Color::Red),
        )));
    }

    // ponytail: single if-let (edition 2021 has no let-chains); .cloned() lifts out of the guard.
    let test_result = state
        .ssh_test_result
        .lock()
        .ok()
        .and_then(|g| g.as_ref().cloned());
    if let Some(r) = test_result {
        lines.push(Line::from(Span::styled(
            format!("  Test: {r}"),
            Style::default().fg(Color::Green),
        )));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(Color::Cyan));
    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: true });
    let _ = Constraint::Min(0);
    frame.render_widget(paragraph, popup_area);
}

/// Build a ManagedHost from the form and persist it (add or atomic rename).
fn save_ssh_form(state: &mut AppState) -> Result<String, String> {
    let form = state
        .ssh_form
        .as_ref()
        .ok_or_else(|| "no form active".to_string())?;
    let alias = form.fields[0].trim().to_string();
    let hostname = form.fields[1].trim().to_string();
    let user = form.fields[2].trim().to_string();
    let port_str = form.fields[3].trim();
    let identity_file = form.fields[4].trim();
    let proxy_jump = form.fields[5].trim();
    let ident_only = form.fields[6].trim().eq_ignore_ascii_case("yes");

    let port: u16 = port_str
        .parse()
        .map_err(|_| format!("Port must be a number 1-65535, got '{port_str}'"))?;

    let host = arx::remote::ssh_config_manager::ManagedHost {
        alias: alias.clone(),
        hostname,
        user,
        port,
        identity_file: if identity_file.is_empty() {
            None
        } else {
            Some(PathBuf::from(identity_file))
        },
        proxy_jump: if proxy_jump.is_empty() {
            None
        } else {
            Some(proxy_jump.to_string())
        },
        identities_only: ident_only,
    };

    match form.mode {
        arx::app::SshHostFormMode::Add => {
            arx::remote::ssh_config_manager::add_managed_host(&host)?;
            state.ssh_hosts = arx::remote::ssh_config_manager::list_managed_hosts()
                .into_values()
                .collect();
            Ok(format!("Added managed host {alias}"))
        }
        arx::app::SshHostFormMode::Edit => {
            let original = form
                .original_alias
                .clone()
                .ok_or_else(|| "missing original alias".to_string())?;
            arx::remote::ssh_config_manager::update_managed_host(&original, &host)?;
            state.ssh_hosts = arx::remote::ssh_config_manager::list_managed_hosts()
                .into_values()
                .collect();
            Ok(format!("Updated managed host {alias}"))
        }
    }
}

/// F7 — Run the bounded ssh connection test off the event loop via
/// spawn_blocking (the test runs blocking subprocess polls; never on a tokio worker).
fn spawn_ssh_test(state: &mut AppState, alias: String) {
    if alias.trim().is_empty() {
        state.ssh_host_status = Some("Cannot test: alias is empty".into());
        return;
    }
    if let Ok(mut g) = state.ssh_test_result.lock() {
        *g = None;
    }
    let slot = state.ssh_test_result.clone();
    let alias_for_spawn = alias.clone();
    tokio::task::spawn_blocking(move || {
        let result = arx::remote::ssh_config_manager::test_connection(&alias_for_spawn);
        if let Ok(mut g) = slot.lock() {
            *g = Some(result.message(&alias_for_spawn));
        }
    });
    state.ssh_host_status = Some(format!("Testing {alias}…"));
}

/// Generate an Ed25519 key for the current form alias and attach it.
fn generate_and_attach(form: &mut arx::app::SshHostForm) -> Result<PathBuf, String> {
    let alias = form.fields[0].trim().to_string();
    if alias.is_empty() {
        return Err("Alias required before generating a key".into());
    }
    let key_name = format!("{alias}_ed25519");
    let path = arx::remote::ssh_config_manager::generate_ed25519_key(&key_name)
        .map_err(|e| e.to_string())?;
    Ok(path)
}

/// Open a config file in the user's editor (O / Shift+O). Argument-safe
/// launcher (DesktopService::open_editor) — no shell interpolation.
fn open_ssh_config_file(state: &mut AppState, path: &Path) {
    let editor = DesktopService::resolve_editor(None);
    match editor {
        Some(editor) => {
            let owned = path.to_path_buf();
            let display = owned.display().to_string();
            tokio::spawn(async move {
                let _ = DesktopService::open_editor(&editor, &owned).await;
            });
            state.ssh_host_status = Some(format!("Opening {display} in editor"));
        }
        None => {
            state.ssh_host_status = Some("No $EDITOR/$VISUAL set; cannot open file".into());
        }
    }
}

fn render_ssh_hosts(frame: &mut ratatui::Frame, area: Rect, state: &AppState) {
    if let Some(form) = &state.ssh_form {
        render_ssh_host_form(frame, area, form, state);
        return;
    }
    let popup_area = centered_rect(70, 70, area);
    frame.render_widget(Clear, popup_area);

    let title = " SSH Hosts (Esc: close, A: add, E: edit, D: delete, T: test, Ctrl+K: key, O: open, R: reload) ";
    let items: Vec<ListItem> = if state.ssh_hosts.is_empty() {
        vec![ListItem::new(Line::from(
            "  No managed hosts — press A to add one.",
        ))]
    } else {
        state
            .ssh_hosts
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let prefix = if i == state.ssh_host_cursor {
                    "> "
                } else {
                    "  "
                };
                let ident = if h.identities_only {
                    " IdentitiesOnly"
                } else {
                    ""
                };
                let key = h
                    .identity_file
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or("-".into());
                let line = format!(
                    "{prefix}{} [{}] {}:{} ({}){}",
                    h.alias, h.user, h.hostname, h.port, key, ident
                );
                ListItem::new(Line::from(line))
            })
            .collect()
    };

    let mut list_state = ListState::default();
    list_state.select(Some(state.ssh_host_cursor));

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .highlight_style(Style::default().fg(Color::Black).bg(Color::White));
    frame.render_stateful_widget(list, popup_area, &mut list_state);
}

pub(super) fn render(frame: &mut Frame, area: Rect, state: &AppState) {
    render_ssh_hosts(frame, area, state);
}

#[allow(clippy::collapsible_if)]
pub(super) fn handle_key(state: &mut AppState, key: KeyEvent) -> bool {
    if !state.show_ssh_hosts {
        return false;
    }
    // When a form is open, route to form editing.
    if state.ssh_form.is_some() {
        match key.code {
            KeyCode::Esc => {
                if let Some(form) = state.ssh_form.as_mut() {
                    if form.confirm_generate {
                        form.confirm_generate = false;
                    } else {
                        state.ssh_form = None;
                    }
                }
            }
            KeyCode::Up | KeyCode::Char('k') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(form) = state.ssh_form.as_mut() {
                    form.focus = form.focus.saturating_sub(1);
                }
            }
            KeyCode::Down | KeyCode::Char('j')
                if !key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                if let Some(form) = state.ssh_form.as_mut() {
                    if form.focus < form.fields.len() - 1 {
                        form.focus += 1;
                    }
                }
            }
            KeyCode::Char('s') | KeyCode::Enter => match save_ssh_form(state) {
                Ok(msg) => {
                    state.ssh_host_status = Some(msg);
                    state.ssh_form = None;
                }
                Err(e) => {
                    if let Some(form) = state.ssh_form.as_mut() {
                        form.error = Some(e);
                    }
                }
            },
            KeyCode::Char('c') => {
                state.ssh_form = None;
            }
            KeyCode::Char('t') => {
                let alias = state
                    .ssh_form
                    .as_ref()
                    .map(|f| f.fields[0].trim().to_string())
                    .unwrap_or_default();
                spawn_ssh_test(state, alias);
            }
            KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(form) = state.ssh_form.as_mut() {
                    form.confirm_generate = true;
                }
            }
            KeyCode::Char('y') => {
                let generated = {
                    let form = state.ssh_form.as_mut().unwrap();
                    if !form.confirm_generate {
                        None
                    } else {
                        match generate_and_attach(form) {
                            Ok(p) => Some(p),
                            Err(e) => {
                                form.error = Some(e);
                                form.confirm_generate = false;
                                None
                            }
                        }
                    }
                };
                if let Some(path) = generated {
                    if let Some(form) = state.ssh_form.as_mut() {
                        form.fields[4] = path.display().to_string();
                        form.fields[6] = "yes".into();
                        form.confirm_generate = false;
                        form.error = None;
                    }
                }
            }
            KeyCode::Char('n') => {
                if let Some(form) = state.ssh_form.as_mut() {
                    form.confirm_generate = false;
                }
            }
            KeyCode::Tab => {
                if let Some(form) = state.ssh_form.as_mut() {
                    let next = (form.focus + 1) % form.fields.len();
                    form.focus = next;
                }
            }
            KeyCode::Backspace => {
                if let Some(form) = state.ssh_form.as_mut() {
                    form.fields[form.focus].pop();
                }
            }
            KeyCode::Char(c)
                if !c.is_control()
                    && state
                        .ssh_form
                        .as_ref()
                        .map(|f| !f.confirm_generate)
                        .unwrap_or(false) =>
            {
                if let Some(form) = state.ssh_form.as_mut() {
                    form.fields[form.focus].push(c);
                }
            }
            _ => {}
        }
        return true;
    }

    // List-mode handling.
    // PACK B: the confirmation gate runs FIRST. While a key
    // generation is pending, handle_ssh_host_keypress swallows
    // every key (y/n/Esc confirm or cancel; all others are
    // ignored) so D/A/E etc. cannot mutate state mid-pending.
    if arx::app::handle_ssh_host_keypress(state, key) {
        return true;
    }

    match key.code {
        KeyCode::Esc => {
            state.show_ssh_hosts = false;
        }
        KeyCode::Up => {
            state.ssh_host_cursor = state.ssh_host_cursor.saturating_sub(1);
        }
        KeyCode::Down if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            let max = state.ssh_hosts.len().saturating_sub(1);
            if state.ssh_host_cursor < max {
                state.ssh_host_cursor += 1;
            }
        }
        KeyCode::Char('a') => {
            state.ssh_form = Some(arx::app::SshHostForm::new_add());
            if let Some(form) = &mut state.ssh_form {
                form.error = None;
            }
        }
        KeyCode::Char('e') => {
            if let Some(h) = state.ssh_hosts.get(state.ssh_host_cursor).cloned() {
                state.ssh_form = Some(arx::app::SshHostForm::new_edit(&h));
            }
        }
        KeyCode::Char('d') => {
            if let Some(h) = state.ssh_hosts.get(state.ssh_host_cursor).cloned() {
                match arx::remote::ssh_config_manager::delete_managed_host(&h.alias) {
                    Ok(_) => state.ssh_host_status = Some(format!("Deleted {}", h.alias)),
                    Err(e) => state.ssh_host_status = Some(format!("Delete failed: {e}")),
                }
                state.ssh_hosts = arx::remote::ssh_config_manager::list_managed_hosts()
                    .into_values()
                    .collect();
            }
        }
        KeyCode::Char('t') => {
            if let Some(h) = state.ssh_hosts.get(state.ssh_host_cursor).cloned() {
                spawn_ssh_test(state, h.alias);
            }
        }
        KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            // handle_ssh_host_keypress already ran above and
            // returned false (no pending), so this sets pending.
            arx::app::handle_ssh_host_keypress(state, key);
        }
        KeyCode::Char('o') => {
            let path = arx::remote::ssh_config_manager::user_ssh_config_path();
            open_ssh_config_file(state, &path);
        }
        KeyCode::Char('O') => {
            let path = arx::remote::ssh_config_manager::managed_config_path();
            open_ssh_config_file(state, &path);
        }
        KeyCode::Char('r') => {
            state.ssh_hosts = arx::remote::ssh_config_manager::list_managed_hosts()
                .into_values()
                .collect();
            state.ssh_host_status = Some("Reloaded".into());
        }
        _ => {}
    }
    true
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn handle_key_inactive_returns_false() {
        let mut state = AppState::default();
        state.show_ssh_hosts = false;
        let handled = handle_key(&mut state, key(KeyCode::Char('a')));
        assert!(!handled);
        assert!(state.ssh_form.is_none());
    }

    #[test]
    fn handle_key_esc_closes_list_overlay() {
        let mut state = AppState::default();
        state.show_ssh_hosts = true;
        let handled = handle_key(&mut state, key(KeyCode::Esc));
        assert!(handled);
        assert!(!state.show_ssh_hosts);
    }

    #[test]
    fn handle_key_up_down_preserve_cursor() {
        let mut state = AppState::default();
        state.show_ssh_hosts = true;
        state.ssh_hosts = vec![
            arx::remote::ssh_config_manager::ManagedHost {
                alias: "h1".into(),
                hostname: "h1".into(),
                user: "u".into(),
                port: 22,
                identity_file: None,
                proxy_jump: None,
                identities_only: false,
            },
            arx::remote::ssh_config_manager::ManagedHost {
                alias: "h2".into(),
                hostname: "h2".into(),
                user: "u".into(),
                port: 22,
                identity_file: None,
                proxy_jump: None,
                identities_only: false,
            },
        ];
        state.ssh_host_cursor = 0;
        handle_key(&mut state, key(KeyCode::Up));
        assert_eq!(state.ssh_host_cursor, 0);
        handle_key(&mut state, key(KeyCode::Down));
        assert_eq!(state.ssh_host_cursor, 1);
    }

    #[test]
    fn handle_key_a_opens_ssh_form() {
        let mut state = AppState::default();
        state.show_ssh_hosts = true;
        let handled = handle_key(&mut state, key(KeyCode::Char('a')));
        assert!(handled);
        assert!(state.ssh_form.is_some());
    }
}
