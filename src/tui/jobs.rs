use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;

use arx::app::AppState;
use arx::jobs::JobKind;
use arx::transfer_queue_runtime::TransferQueueRuntime;

use super::*;

pub(super) fn render(frame: &mut Frame, area: Rect, state: &AppState) {
    let popup_area = centered_rect(70, 70, area);
    frame.render_widget(Clear, popup_area);

    let items: Vec<ListItem> = if state.jobs.is_empty() {
        vec![ListItem::new(Line::from(
            "  No jobs yet — start a copy/move to see it here.",
        ))]
    } else {
        state
            .jobs
            .iter()
            .enumerate()
            .map(|(i, j)| {
                let prefix = if i == state.job_cursor { "> " } else { "  " };
                let bar = if j.status == arx::jobs::JobStatus::Running {
                    let filled = j.progress.percent().unwrap_or(0) as usize / 5; // 0-20 chars
                    let empty = 20 - filled;
                    format!(
                        " [{}{}] {}%",
                        "=".repeat(filled),
                        " ".repeat(empty),
                        j.progress
                    )
                } else {
                    String::new()
                };
                ListItem::new(Line::from(format!("{prefix}{j} {bar}")))
            })
            .collect()
    };

    let mut list_state = ListState::default();
    list_state.select(Some(state.job_cursor));

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Jobs (Ctrl+J: close) "),
        )
        .highlight_style(Style::default().fg(Color::Black).bg(Color::White));
    frame.render_stateful_widget(list, popup_area, &mut list_state);
}

pub(super) fn handle_key(
    state: &mut AppState,
    key: KeyEvent,
    transfers: &TransferQueueRuntime,
) -> bool {
    if !state.show_jobs {
        return false;
    }
    match key.code {
        KeyCode::Esc | KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.show_jobs = false;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            state.job_cursor = state.job_cursor.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            let max = state.jobs.len().saturating_sub(1);
            if state.job_cursor < max {
                state.job_cursor += 1;
            }
        }
        KeyCode::Char('p') => {
            if let Some(job) = state.jobs.get(state.job_cursor)
                && job.kind == JobKind::Transfer
                && transfers.request_pause(&job.id).is_ok()
            {
                state.message =
                    Some("Pause requested (will checkpoint at next safe boundary)".into());
            }
        }
        KeyCode::Char('r') => {
            if let Some(job) = state.jobs.get(state.job_cursor)
                && job.kind == JobKind::Transfer
                && transfers.resume(&job.id).is_ok()
            {
                state.message = Some("Resume requested".into());
            }
        }
        _ => {}
    }
    true
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;
    use arx::jobs::JobManager;
    use arx::transfer_queue::TransferQueueConfig;
    use arx::vfs::ProviderRegistry;
    use crossterm::event::KeyModifiers;
    use tokio::sync::mpsc;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn runtime() -> TransferQueueRuntime {
        let (tx, _rx) = mpsc::unbounded_channel();
        TransferQueueRuntime::new(
            JobManager::default(),
            tx,
            ProviderRegistry::new(),
            TransferQueueConfig::default(),
        )
    }

    fn transfer_job() -> arx::jobs::Job {
        JobManager::default().create_job("t", JobKind::Transfer, "t", None, None)
    }

    #[test]
    fn handle_key_inactive_returns_false() {
        let mut state = AppState::default();
        state.show_jobs = false;
        let rt = runtime();
        let handled = handle_key(&mut state, key(KeyCode::Char('a')), &rt);
        assert!(!handled);
        assert_eq!(state.job_cursor, 0);
    }

    #[test]
    fn handle_key_close_key_closes() {
        let mut state = AppState::default();
        state.show_jobs = true;
        let rt = runtime();
        let k = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL);
        let handled = handle_key(&mut state, k, &rt);
        assert!(handled);
        assert!(!state.show_jobs);
    }

    #[test]
    fn handle_key_up_does_not_underflow() {
        let mut state = AppState::default();
        state.show_jobs = true;
        state.jobs = vec![transfer_job()];
        let rt = runtime();
        handle_key(&mut state, key(KeyCode::Up), &rt);
        assert_eq!(state.job_cursor, 0);
    }

    #[test]
    fn handle_key_down_advances_and_clamps() {
        let mut state = AppState::default();
        state.show_jobs = true;
        state.jobs = vec![transfer_job(), transfer_job()];
        let rt = runtime();
        handle_key(&mut state, key(KeyCode::Down), &rt);
        assert_eq!(state.job_cursor, 1);
        handle_key(&mut state, key(KeyCode::Down), &rt);
        assert_eq!(state.job_cursor, 1);
    }
}
