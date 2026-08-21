//! Keyboard-first Transfer Center presentation and control surface.
//!
//! `JobManager` remains lifecycle truth and `TransferQueueRuntime` remains the
//! only scheduler/executor owner. This module owns presentation state only and
//! routes pause/resume/cancel through the existing runtime APIs.

use std::cmp::Ordering;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use crate::app::{AppState, OverlayKind};
use crate::jobs::{Job, JobKind, JobProgress, JobResult, JobStatus, Progress};
use crate::transfer_queue::{CancelAction, PauseAction};
use crate::transfer_queue_runtime::TransferQueueRuntime;

/// Bound only the terminal presentation list. JobManager retention is not
/// changed by this UI policy.
pub const TRANSFER_HISTORY_LIMIT: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TransferCenterFilter {
    #[default]
    Active,
    History,
    All,
}

impl TransferCenterFilter {
    pub const fn next(self) -> Self {
        match self {
            Self::Active => Self::History,
            Self::History => Self::All,
            Self::All => Self::Active,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::History => "history",
            Self::All => "all",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TransferCenterUiState {
    pub cursor: usize,
    pub selected_job_id: Option<String>,
    pub filter: TransferCenterFilter,
    pub feedback: Option<String>,
}

impl TransferCenterUiState {
    fn reconcile(&mut self, visible_ids: &[String]) {
        if visible_ids.is_empty() {
            self.cursor = 0;
            self.selected_job_id = None;
            return;
        }

        if let Some(selected) = self.selected_job_id.as_deref()
            && let Some(index) = visible_ids.iter().position(|id| id == selected)
        {
            self.cursor = index;
            return;
        }

        self.cursor = self.cursor.min(visible_ids.len() - 1);
        self.selected_job_id = visible_ids.get(self.cursor).cloned();
    }

    fn move_up(&mut self, visible_ids: &[String]) {
        self.reconcile(visible_ids);
        self.cursor = self.cursor.saturating_sub(1);
        self.selected_job_id = visible_ids.get(self.cursor).cloned();
    }

    fn move_down(&mut self, visible_ids: &[String]) {
        self.reconcile(visible_ids);
        if !visible_ids.is_empty() {
            self.cursor = self.cursor.saturating_add(1).min(visible_ids.len() - 1);
            self.selected_job_id = visible_ids.get(self.cursor).cloned();
        }
    }

    fn move_home(&mut self, visible_ids: &[String]) {
        self.cursor = 0;
        self.selected_job_id = visible_ids.first().cloned();
    }

    fn move_end(&mut self, visible_ids: &[String]) {
        self.cursor = visible_ids.len().saturating_sub(1);
        self.selected_job_id = visible_ids.last().cloned();
    }
}

/// Return transfer jobs in the deterministic order used by the Center.
///
/// Active jobs are oldest first. History is newest first and presentation-
/// bounded. `All` keeps every active job first and then the same bounded
/// terminal history. This never deletes jobs from JobManager.
pub fn visible_transfer_jobs(jobs: &[Job], filter: TransferCenterFilter) -> Vec<&Job> {
    let mut active = jobs
        .iter()
        .filter(|job| job.kind == JobKind::Transfer && !job.status.is_terminal())
        .collect::<Vec<_>>();
    active.sort_by(compare_oldest_first);

    let mut history = jobs
        .iter()
        .filter(|job| job.kind == JobKind::Transfer && job.status.is_terminal())
        .collect::<Vec<_>>();
    history.sort_by(compare_newest_first);
    history.truncate(TRANSFER_HISTORY_LIMIT);

    match filter {
        TransferCenterFilter::Active => active,
        TransferCenterFilter::History => history,
        TransferCenterFilter::All => {
            active.extend(history);
            active
        }
    }
}

fn compare_oldest_first(left: &&Job, right: &&Job) -> Ordering {
    job_sequence(&left.id)
        .cmp(&job_sequence(&right.id))
        .then_with(|| left.id.cmp(&right.id))
}

fn compare_newest_first(left: &&Job, right: &&Job) -> Ordering {
    job_sequence(&right.id)
        .cmp(&job_sequence(&left.id))
        .then_with(|| right.id.cmp(&left.id))
}

fn job_sequence(id: &str) -> u64 {
    id.rsplit_once('-')
        .and_then(|(_, sequence)| sequence.parse().ok())
        .unwrap_or(0)
}

fn visible_ids(state: &AppState) -> Vec<String> {
    visible_transfer_jobs(&state.jobs, state.transfer_center.filter)
        .into_iter()
        .map(|job| job.id.clone())
        .collect()
}

/// A Transfer Center action key must be a plain character: no Ctrl or Alt
/// modifier. This keeps Ctrl+C/Ctrl+P etc. from accidentally cancelling or
/// pausing transfers.
fn is_plain_char(key: &KeyEvent, expected: char) -> bool {
    key.code == KeyCode::Char(expected)
        && !key.modifiers.contains(KeyModifiers::CONTROL)
        && !key.modifiers.contains(KeyModifiers::ALT)
}

fn selected_job<'a>(state: &'a AppState, visible_ids: &[String]) -> Option<&'a Job> {
    let id = state
        .transfer_center
        .selected_job_id
        .as_deref()
        .or_else(|| {
            visible_ids
                .get(state.transfer_center.cursor)
                .map(String::as_str)
        })?;
    state
        .jobs
        .iter()
        .find(|job| job.kind == JobKind::Transfer && job.id == id)
}

pub fn handle_transfer_center_key(
    state: &mut AppState,
    runtime: &TransferQueueRuntime,
    key: KeyEvent,
) {
    let ids = visible_ids(state);
    state.transfer_center.reconcile(&ids);

    match key.code {
        KeyCode::Esc => state.close_overlay(OverlayKind::TransferCenter),
        KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.toggle_overlay(OverlayKind::TransferCenter);
        }
        KeyCode::Up | KeyCode::Char('k') => state.transfer_center.move_up(&ids),
        KeyCode::Down | KeyCode::Char('j') => state.transfer_center.move_down(&ids),
        KeyCode::Home => state.transfer_center.move_home(&ids),
        KeyCode::End => state.transfer_center.move_end(&ids),
        KeyCode::Char('f') => {
            state.transfer_center.filter = state.transfer_center.filter.next();
            state.transfer_center.cursor = 0;
            state.transfer_center.selected_job_id = None;
            let ids = visible_ids(state);
            state.transfer_center.reconcile(&ids);
            state.transfer_center.feedback = Some(format!(
                "Transfer filter: {}",
                state.transfer_center.filter.label()
            ));
        }
        KeyCode::Char('p') if is_plain_char(&key, 'p') => control_pause_or_resume(state, runtime),
        KeyCode::Char('c') if is_plain_char(&key, 'c') => control_cancel(state, runtime),
        _ => {}
    }
}

fn control_pause_or_resume(state: &mut AppState, runtime: &TransferQueueRuntime) {
    let ids = visible_ids(state);
    state.transfer_center.reconcile(&ids);
    let Some(job) = selected_job(state, &ids).cloned() else {
        state.transfer_center.feedback = Some("No transfer selected".into());
        return;
    };

    let feedback = match job.status {
        JobStatus::Pending | JobStatus::Running => match runtime.request_pause(&job.id) {
            Ok(PauseAction::AwaitSafeCheckpoint) => {
                "Pause requested; waiting for a safe checkpoint".to_string()
            }
            Ok(PauseAction::ParkedBeforeExecution) => "Transfer paused before execution".into(),
            Ok(PauseAction::AlreadyParked) => "Transfer is already paused".into(),
            Ok(PauseAction::AlreadyFinished) => "Transfer is already finished".into(),
            Err(error) => format!("Pause unavailable: {error}"),
        },
        JobStatus::Paused => match runtime.resume(&job.id) {
            Ok(()) => "Transfer resumed".into(),
            Err(error) => format!("Resume unavailable: {error}"),
        },
        JobStatus::PausePending => "Pause already pending; waiting for safe checkpoint".into(),
        JobStatus::RetryWaiting => "Pause unavailable while waiting to retry".into(),
        JobStatus::Cancelling => "Pause unavailable while cancelling".into(),
        JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled => {
            "Pause/resume unavailable for a terminal transfer".into()
        }
    };

    state.jobs = runtime.manager().snapshot();
    state.transfer_center.feedback = Some(feedback);
    let ids = visible_ids(state);
    state.transfer_center.reconcile(&ids);
}

fn control_cancel(state: &mut AppState, runtime: &TransferQueueRuntime) {
    let ids = visible_ids(state);
    state.transfer_center.reconcile(&ids);
    let Some(job) = selected_job(state, &ids).cloned() else {
        state.transfer_center.feedback = Some("No transfer selected".into());
        return;
    };

    if job.status.is_terminal() {
        state.transfer_center.feedback = Some("Cancel unavailable for a terminal transfer".into());
        return;
    }

    let feedback = match runtime.cancel(&job.id) {
        Ok(CancelAction::TerminalizeWithoutExecution) => {
            "Transfer cancelled before execution".into()
        }
        Ok(CancelAction::SignalActiveExecution) => "Transfer cancellation requested".into(),
        Ok(CancelAction::AlreadyFinished) => "Transfer is already finished".into(),
        Err(error) => format!("Cancel unavailable: {error}"),
    };
    state.jobs = runtime.manager().snapshot();
    state.transfer_center.feedback = Some(feedback);
    let ids = visible_ids(state);
    state.transfer_center.reconcile(&ids);
}

pub fn render_transfer_center(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    state: &mut AppState,
    runtime: &TransferQueueRuntime,
) {
    let ids = visible_ids(state);
    state.transfer_center.reconcile(&ids);
    let jobs = visible_transfer_jobs(&state.jobs, state.transfer_center.filter);

    let popup = centered_rect(92, 82, area);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Transfer Center · Ctrl+Y/Esc close ");
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(8),
            Constraint::Length(2),
        ])
        .split(inner);

    let summary = runtime.summary();
    let config = runtime.config();
    let terminal_count = state
        .jobs
        .iter()
        .filter(|job| job.kind == JobKind::Transfer && job.status.is_terminal())
        .count();
    let header = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("filter ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(state.transfer_center.filter.label()),
            Span::raw("  ·  "),
            Span::raw(format!(
                "{} running · {} waiting · {} paused · concurrency {}",
                summary.running,
                summary.waiting,
                summary.paused,
                config.concurrency()
            )),
        ]),
        Line::from(format!(
            "{} visible · {} terminal total · history display limit {}",
            jobs.len(),
            terminal_count,
            TRANSFER_HISTORY_LIMIT
        )),
    ]);
    frame.render_widget(header, vertical[0]);

    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(57), Constraint::Percentage(43)])
        .split(vertical[1]);

    let items = jobs
        .iter()
        .map(|job| ListItem::new(render_list_row(job)))
        .collect::<Vec<_>>();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Transfers "))
        .highlight_symbol("> ")
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD));
    let mut list_state = ListState::default();
    if !jobs.is_empty() {
        list_state.select(Some(state.transfer_center.cursor.min(jobs.len() - 1)));
    }
    frame.render_stateful_widget(list, horizontal[0], &mut list_state);

    let detail = selected_job(state, &ids)
        .map(|job| render_job_detail(job, runtime))
        .unwrap_or_else(|| vec![Line::from("No transfer in this view")]);
    frame.render_widget(
        Paragraph::new(detail)
            .block(Block::default().borders(Borders::ALL).title(" Selected "))
            .wrap(Wrap { trim: false }),
        horizontal[1],
    );

    let feedback = state
        .transfer_center
        .feedback
        .as_deref()
        .map(safe_text)
        .unwrap_or_default();
    let footer = if feedback.is_empty() {
        "j/k move · f filter · p pause/resume · c cancel · Ctrl+Y/Esc close".to_string()
    } else {
        format!("j/k move · f filter · p pause/resume · c cancel · Ctrl+Y/Esc close\n{feedback}")
    };
    frame.render_widget(
        Paragraph::new(footer).wrap(Wrap { trim: true }),
        vertical[2],
    );
}

fn render_list_row(job: &Job) -> String {
    let mut parts = vec![
        format!("{:<11}", status_label(job.status)),
        safe_text(&job.id),
        safe_text(&job.description),
    ];
    if !matches!(&job.progress, JobProgress::Generic(Progress::Indeterminate))
        && !job.status.is_terminal()
    {
        parts.push(safe_text(&job.progress.to_string()));
    }
    safe_text(&parts.join(" · "))
}

fn render_job_detail(job: &Job, runtime: &TransferQueueRuntime) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(detail_line("Job", safe_text(&job.id)));
    lines.push(detail_line("Status", status_label(job.status).to_string()));

    if let Some(source) = job.display_source() {
        lines.push(detail_line("Source", safe_text(&source.to_string())));
    }
    if let Some(destination) = job.display_destination() {
        lines.push(detail_line(
            "Destination",
            safe_text(&destination.to_string()),
        ));
    }

    lines.push(detail_line("Progress", progress_detail(job)));

    if let Some(info) = runtime.inspect_job(&job.id) {
        lines.push(detail_line(
            "Scheduler",
            format!("{:?}", info.scheduler_state),
        ));
        lines.push(detail_line(
            "Attempts",
            format!(
                "{}/{}",
                info.attempts_started,
                runtime.config().max_total_attempts()
            ),
        ));
    } else {
        lines.push(detail_line("Scheduler", "not retained".into()));
    }
    lines.push(detail_line(
        "Concurrency",
        runtime.config().concurrency().to_string(),
    ));

    if let Some(error) = job.error.as_deref() {
        lines.push(detail_line("Error", safe_text(error)));
    }
    if let Some(result) = result_detail(job) {
        lines.push(detail_line("Result", result));
    }
    lines
}

fn detail_line(label: &'static str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label}: "),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(value),
    ])
}

fn progress_detail(job: &Job) -> String {
    match &job.progress {
        JobProgress::Generic(Progress::Bytes { done, total, rate }) => {
            let mut parts = Vec::new();
            match total {
                Some(total) => {
                    parts.push(format!(
                        "{} / {}",
                        format_bytes(*done),
                        format_bytes(*total)
                    ));
                    if *total > 0 {
                        let percent = (((*done as u128) * 100) / (*total as u128)).min(100);
                        parts.push(format!("{percent}%"));
                    } else {
                        parts.push("0%".into());
                    }
                }
                None => {
                    parts.push(format_bytes(*done));
                    parts.push("unknown total".into());
                }
            }
            if *rate > 0 {
                parts.push(format!("{}/s", format_bytes(*rate)));
                if let Some(eta) = job.eta() {
                    parts.push(format!("ETA {eta}"));
                }
            }
            parts.join(" · ")
        }
        JobProgress::Generic(Progress::Items { done, total }) => format!("{done}/{total} items"),
        JobProgress::Generic(Progress::Percent(percent)) => format!("{percent}%"),
        JobProgress::Generic(Progress::Phase { phase, percent }) => match percent {
            Some(percent) => format!("{} · {percent}%", safe_text(phase)),
            None => safe_text(phase),
        },
        JobProgress::Generic(Progress::Indeterminate) => "indeterminate".into(),
        other => safe_text(&other.to_string()),
    }
}

fn result_detail(job: &Job) -> Option<String> {
    match job.result.as_ref()? {
        JobResult::Generic {
            message,
            completed_items,
        } => {
            let mut parts = Vec::new();
            if let Some(message) = message {
                parts.push(safe_text(message));
            }
            if let Some(completed) = completed_items {
                parts.push(format!("{completed} completed"));
            }
            (!parts.is_empty()).then(|| parts.join(" · "))
        }
        _ => None,
    }
}

fn status_label(status: JobStatus) -> &'static str {
    match status {
        JobStatus::Pending => "QUEUED",
        JobStatus::Running => "RUNNING",
        JobStatus::PausePending => "PAUSING",
        JobStatus::Paused => "PAUSED",
        JobStatus::RetryWaiting => "RETRY WAIT",
        JobStatus::Cancelling => "CANCELLING",
        JobStatus::Completed => "COMPLETED",
        JobStatus::Failed => "FAILED",
        JobStatus::Cancelled => "CANCELLED",
    }
}

fn safe_text(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            ch if ch.is_control() => output.push_str(&format!("\\u{{{:x}}}", ch as u32)),
            ch => output.push(ch),
        }
    }
    output
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: &[(&str, u64)] = &[
        ("TiB", 1u64 << 40),
        ("GiB", 1u64 << 30),
        ("MiB", 1u64 << 20),
        ("KiB", 1u64 << 10),
    ];
    for (label, unit) in UNITS {
        if bytes >= *unit {
            let whole = bytes / *unit;
            let tenth = (bytes % *unit) * 10 / *unit;
            return format!("{whole}.{tenth} {label}");
        }
    }
    format!("{bytes} B")
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::{JobEvent, JobManager};
    use crate::vfs::Location;
    use std::path::PathBuf;
    use tokio::sync::mpsc;

    fn transfer(manager: &JobManager) -> Job {
        manager.create_job(
            "transfer",
            JobKind::Transfer,
            "transfer",
            Some(Location::Local(PathBuf::from("/src"))),
            Some(Location::Local(PathBuf::from("/dst"))),
        )
    }

    fn complete(manager: &JobManager, job: &Job) {
        let (tx, _rx) = mpsc::unbounded_channel();
        assert!(manager.publish_event(&tx, JobEvent::Running { id: job.id.clone() }));
        assert!(manager.publish_event(
            &tx,
            JobEvent::Completed {
                id: job.id.clone(),
                result: JobResult::generic("done", 1),
            }
        ));
    }

    #[test]
    fn active_history_all_filters_are_deterministic() {
        let manager = JobManager::new();
        let a = transfer(&manager);
        let b = transfer(&manager);
        let c = transfer(&manager);
        complete(&manager, &b);
        complete(&manager, &c);
        let jobs = manager.snapshot();

        let active = visible_transfer_jobs(&jobs, TransferCenterFilter::Active);
        assert_eq!(
            active.iter().map(|job| job.id.as_str()).collect::<Vec<_>>(),
            [a.id.as_str()]
        );

        let history = visible_transfer_jobs(&jobs, TransferCenterFilter::History);
        assert_eq!(
            history
                .iter()
                .map(|job| job.id.as_str())
                .collect::<Vec<_>>(),
            [c.id.as_str(), b.id.as_str()]
        );

        let all = visible_transfer_jobs(&jobs, TransferCenterFilter::All);
        assert_eq!(
            all.iter().map(|job| job.id.as_str()).collect::<Vec<_>>(),
            [a.id.as_str(), c.id.as_str(), b.id.as_str()]
        );
    }

    #[test]
    fn history_is_presentation_bounded_without_deleting_manager_jobs() {
        let manager = JobManager::new();
        for _ in 0..(TRANSFER_HISTORY_LIMIT + 7) {
            let job = transfer(&manager);
            complete(&manager, &job);
        }
        let jobs = manager.snapshot();
        assert_eq!(
            jobs.iter()
                .filter(|job| job.kind == JobKind::Transfer)
                .count(),
            TRANSFER_HISTORY_LIMIT + 7
        );
        assert_eq!(
            visible_transfer_jobs(&jobs, TransferCenterFilter::History).len(),
            TRANSFER_HISTORY_LIMIT
        );
    }

    #[test]
    fn selection_tracks_job_id_and_clamps_when_view_changes() {
        let mut ui = TransferCenterUiState::default();
        let ids = vec![
            "transfer-1".into(),
            "transfer-2".into(),
            "transfer-3".into(),
        ];
        ui.reconcile(&ids);
        ui.move_down(&ids);
        assert_eq!(ui.selected_job_id.as_deref(), Some("transfer-2"));

        let reordered = vec!["transfer-3".into(), "transfer-2".into()];
        ui.reconcile(&reordered);
        assert_eq!(ui.cursor, 1);
        assert_eq!(ui.selected_job_id.as_deref(), Some("transfer-2"));

        let filtered = vec!["transfer-3".into()];
        ui.reconcile(&filtered);
        assert_eq!(ui.cursor, 0);
        assert_eq!(ui.selected_job_id.as_deref(), Some("transfer-3"));

        ui.reconcile(&[]);
        assert_eq!(ui.cursor, 0);
        assert_eq!(ui.selected_job_id, None);
    }

    #[test]
    fn unknown_byte_total_never_invents_percent_or_eta() {
        let manager = JobManager::new();
        let (tx, _rx) = mpsc::unbounded_channel();
        let job = transfer(&manager);
        assert!(manager.publish_event(&tx, JobEvent::Running { id: job.id.clone() }));
        assert!(manager.publish_event(
            &tx,
            JobEvent::Progress {
                id: job.id.clone(),
                progress: JobProgress::Generic(Progress::Bytes {
                    done: 1024,
                    total: None,
                    rate: 512,
                }),
            }
        ));
        let job = manager.get(&job.id).unwrap();
        let detail = progress_detail(&job);
        assert!(detail.contains("unknown total"));
        assert!(detail.contains("512 B/s"));
        assert!(!detail.contains('%'));
        assert!(!detail.contains("ETA"));
    }

    #[test]
    fn known_byte_total_uses_real_percent_rate_and_eta() {
        let manager = JobManager::new();
        let (tx, _rx) = mpsc::unbounded_channel();
        let job = transfer(&manager);
        assert!(manager.publish_event(&tx, JobEvent::Running { id: job.id.clone() }));
        assert!(manager.publish_event(
            &tx,
            JobEvent::Progress {
                id: job.id.clone(),
                progress: JobProgress::Generic(Progress::Bytes {
                    done: 512,
                    total: Some(1024),
                    rate: 256,
                }),
            }
        ));
        let job = manager.get(&job.id).unwrap();
        let detail = progress_detail(&job);
        assert!(detail.contains("50%"));
        assert!(detail.contains("256 B/s"));
        assert!(detail.contains("ETA 2s"));
    }

    #[test]
    fn item_progress_has_no_fake_speed_or_eta() {
        let manager = JobManager::new();
        let (tx, _rx) = mpsc::unbounded_channel();
        let job = transfer(&manager);
        assert!(manager.publish_event(&tx, JobEvent::Running { id: job.id.clone() }));
        assert!(manager.publish_event(
            &tx,
            JobEvent::Progress {
                id: job.id.clone(),
                progress: JobProgress::Generic(Progress::Items { done: 2, total: 5 }),
            }
        ));
        let job = manager.get(&job.id).unwrap();
        let detail = progress_detail(&job);
        assert_eq!(detail, "2/5 items");
        assert!(!detail.contains("/s"));
        assert!(!detail.contains("ETA"));
    }

    #[test]
    fn control_text_is_escaped_before_rendering() {
        assert_eq!(safe_text("a\nb\t\u{1b}c"), "a\\nb\\t\\u{1b}c");
    }

    #[test]
    fn pause_action_availability_is_truthful() {
        for status in [JobStatus::Pending, JobStatus::Running, JobStatus::Paused] {
            assert!(!status.is_terminal());
        }
        for status in [
            JobStatus::Completed,
            JobStatus::Failed,
            JobStatus::Cancelled,
        ] {
            assert!(status.is_terminal());
        }
        assert_eq!(status_label(JobStatus::RetryWaiting), "RETRY WAIT");
        assert_eq!(status_label(JobStatus::PausePending), "PAUSING");
        assert_eq!(status_label(JobStatus::Cancelling), "CANCELLING");
    }

    #[test]
    fn plain_c_is_an_action_candidate_but_ctrl_c_is_not() {
        let plain = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE);
        let ctrl = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(is_plain_char(&plain, 'c'));
        assert!(!is_plain_char(&ctrl, 'c'));
    }

    #[test]
    fn plain_p_is_an_action_candidate_but_ctrl_p_is_not() {
        let plain = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE);
        let ctrl = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL);
        assert!(is_plain_char(&plain, 'p'));
        assert!(!is_plain_char(&ctrl, 'p'));
    }
}
