use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;

use arx::app::AppState;
use arx::jobs::JobKind;

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

pub(super) fn handle_key(state: &mut AppState, key: KeyEvent, sync: &super::SyncUiRuntime) -> bool {
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
                && sync.transfers.request_pause(&job.id).is_ok()
            {
                state.message =
                    Some("Pause requested (will checkpoint at next safe boundary)".into());
            }
        }
        KeyCode::Char('r') => {
            if let Some(job) = state.jobs.get(state.job_cursor)
                && job.kind == JobKind::Transfer
                && sync.transfers.resume(&job.id).is_ok()
            {
                state.message = Some("Resume requested".into());
            }
        }
        KeyCode::Delete if state.show_jobs => {
            if let Some(job) = state.jobs.get(state.job_cursor) {
                let id = job.id.clone();
                super::cancel_job_product_route(state, sync, &id);
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
    use arx::services::WorkspaceSyncController;
    use arx::transfer_queue::TransferQueueConfig;
    use arx::transfer_queue_runtime::TransferQueueRuntime;
    use arx::vfs::ProviderRegistry;
    use crossterm::event::KeyModifiers;
    use tokio::sync::mpsc;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn sync() -> super::SyncUiRuntime {
        let (job_tx, _job_rx) = mpsc::unbounded_channel();
        let (ver_tx, _ver_rx) = mpsc::unbounded_channel();
        let (launch_tx, _launch_rx) = mpsc::unbounded_channel();
        super::SyncUiRuntime {
            controller: WorkspaceSyncController::new(ProviderRegistry::new()),
            jobs: JobManager::default(),
            job_events: job_tx.clone(),
            verification_events: ver_tx,
            launch_events: launch_tx,
            transfers: TransferQueueRuntime::new(
                JobManager::default(),
                job_tx,
                ProviderRegistry::new(),
                TransferQueueConfig::default(),
            ),
        }
    }

    fn transfer_job() -> arx::jobs::Job {
        JobManager::default().create_job("t", JobKind::Transfer, "t", None, None)
    }

    #[test]
    fn handle_key_inactive_returns_false() {
        let mut state = AppState::default();
        state.show_jobs = false;
        let s = sync();
        let handled = handle_key(&mut state, key(KeyCode::Char('a')), &s);
        assert!(!handled);
        assert_eq!(state.job_cursor, 0);
    }

    #[test]
    fn handle_key_close_key_closes() {
        let mut state = AppState::default();
        state.show_jobs = true;
        let s = sync();
        let k = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL);
        let handled = handle_key(&mut state, k, &s);
        assert!(handled);
        assert!(!state.show_jobs);
    }

    #[test]
    fn handle_key_up_does_not_underflow() {
        let mut state = AppState::default();
        state.show_jobs = true;
        state.jobs = vec![transfer_job()];
        let s = sync();
        handle_key(&mut state, key(KeyCode::Up), &s);
        assert_eq!(state.job_cursor, 0);
    }

    #[test]
    fn handle_key_down_advances_and_clamps() {
        let mut state = AppState::default();
        state.show_jobs = true;
        state.jobs = vec![transfer_job(), transfer_job()];
        let s = sync();
        handle_key(&mut state, key(KeyCode::Down), &s);
        assert_eq!(state.job_cursor, 1);
        handle_key(&mut state, key(KeyCode::Down), &s);
        assert_eq!(state.job_cursor, 1);
    }
}
