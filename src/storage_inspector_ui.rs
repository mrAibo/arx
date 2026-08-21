//! Read-only Storage Inspector UI state, interaction and rendering.
//!
//! The scanner and JobManager remain the sources of runtime truth. This module
//! owns only presentation state and immutable snapshot drill-down behavior.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use crate::app::{AppState, OverlayKind};
use crate::jobs::{JobProgress, JobResult, JobStatus};
use crate::storage_inspector::{
    UsageKind, UsageRecord, UsageScanOptions, UsageScanOutcome, UsageScanResult,
};

const TOP_FILES_LIMIT: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StorageSizeBasis {
    #[default]
    Allocated,
    Logical,
}

impl StorageSizeBasis {
    pub const fn toggle(self) -> Self {
        match self {
            Self::Allocated => Self::Logical,
            Self::Logical => Self::Allocated,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Allocated => "allocated",
            Self::Logical => "apparent",
        }
    }

    fn record_bytes(self, record: &UsageRecord) -> u128 {
        match self {
            Self::Allocated => record.subtree_allocated_bytes,
            Self::Logical => record.subtree_logical_bytes,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StorageSort {
    #[default]
    Size,
    Name,
    Items,
}

impl StorageSort {
    pub const fn next(self) -> Self {
        match self {
            Self::Size => Self::Name,
            Self::Name => Self::Items,
            Self::Items => Self::Size,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Size => "size↓",
            Self::Name => "name↑",
            Self::Items => "items↓",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StorageView {
    #[default]
    Directory,
    TopFiles,
}

impl StorageView {
    pub const fn toggle(self) -> Self {
        match self {
            Self::Directory => Self::TopFiles,
            Self::TopFiles => Self::Directory,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Directory => "directory",
            Self::TopFiles => "top files",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct StorageInspectorUiState {
    pub job_id: Option<String>,
    pub root: Option<PathBuf>,
    pub current_dir: Option<PathBuf>,
    pub cursor: usize,
    pub basis: StorageSizeBasis,
    pub sort: StorageSort,
    pub view: StorageView,
}

impl StorageInspectorUiState {
    pub fn start(&mut self, job_id: String, root: PathBuf) {
        self.job_id = Some(job_id);
        self.root = Some(root.clone());
        self.current_dir = Some(root);
        self.cursor = 0;
        self.basis = StorageSizeBasis::Allocated;
        self.sort = StorageSort::Size;
        self.view = StorageView::Directory;
    }

    fn reset_cursor(&mut self) {
        self.cursor = 0;
    }
}

#[derive(Debug, Clone)]
struct StorageRow {
    path: PathBuf,
    kind: UsageKind,
    bytes: u128,
    items: u64,
    metadata_error: bool,
    hardlink_duplicate: bool,
}

impl StorageRow {
    fn name(&self) -> String {
        self.path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

/// Launch a new local scan or reopen the one already running.
///
/// Only one Storage Inspector scan is retained by the UI at a time. Starting a
/// new scan drops the previous completed snapshot so repeated use in a long ARX
/// session cannot grow snapshot memory without bound.
pub fn launch_storage_inspector(state: &mut AppState) -> Result<String, String> {
    let root = match &state.active_pane().location {
        crate::vfs::Location::Local(path) => path.clone(),
        _ => return Err("Storage Inspector is available for local paths only".into()),
    };

    let manager = state
        .job_manager
        .clone()
        .ok_or_else(|| "Storage Inspector: JobManager is not bound".to_string())?;
    let events = state
        .job_events
        .clone()
        .ok_or_else(|| "Storage Inspector: job event channel is not bound".to_string())?;

    if let Some(id) = state.storage_inspector.job_id.as_deref()
        && manager.get(id).is_some_and(|job| !job.status.is_terminal())
    {
        state.open_overlay(OverlayKind::StorageInspector);
        return Ok(id.to_string());
    }

    if let Some(old_id) = state.storage_inspector.job_id.take() {
        state.storage_scan_snapshots.remove(&old_id);
    }

    let id = manager.spawn_storage_scan(
        root.clone(),
        UsageScanOptions::default(),
        events,
        state.storage_scan_snapshots.clone(),
    );
    state.storage_inspector.start(id.clone(), root);
    state.jobs = manager.snapshot();
    state.open_overlay(OverlayKind::StorageInspector);
    Ok(id)
}

/// Handle one key while the Storage Inspector overlay is active.
/// The overlay is exclusive: callers should consume the key regardless of
/// whether this function changes state.
pub fn handle_storage_inspector_key(state: &mut AppState, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => state.close_overlay(OverlayKind::StorageInspector),
        KeyCode::Up | KeyCode::Char('k') => {
            state.storage_inspector.cursor = state.storage_inspector.cursor.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') => move_cursor_down(state),
        KeyCode::Home => state.storage_inspector.cursor = 0,
        KeyCode::End => {
            state.storage_inspector.cursor = visible_rows(state).len().saturating_sub(1);
        }
        KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => enter_selected_directory(state),
        KeyCode::Backspace | KeyCode::Left | KeyCode::Char('h') => go_to_parent(state),
        KeyCode::Char('b') => {
            state.storage_inspector.basis = state.storage_inspector.basis.toggle();
            state.storage_inspector.reset_cursor();
        }
        KeyCode::Char('s') => {
            state.storage_inspector.sort = state.storage_inspector.sort.next();
            state.storage_inspector.reset_cursor();
        }
        KeyCode::Char('t') => {
            state.storage_inspector.view = state.storage_inspector.view.toggle();
            state.storage_inspector.reset_cursor();
        }
        KeyCode::Char('c') => cancel_running_scan(state),
        _ => {}
    }
}

fn cancel_running_scan(state: &mut AppState) {
    let Some(id) = state.storage_inspector.job_id.clone() else {
        return;
    };
    let Some(manager) = state.job_manager.clone() else {
        return;
    };
    match manager.get(&id) {
        Some(job) if !job.status.is_terminal() => {
            if manager.cancel(&id) {
                state.jobs = manager.snapshot();
                state.message = Some(format!("Cancelling storage scan {id}…"));
            }
        }
        Some(_) => state.message = Some("Storage scan has already finished".into()),
        None => state.message = Some("Storage scan job is no longer available".into()),
    }
}

fn move_cursor_down(state: &mut AppState) {
    let max = visible_rows(state).len().saturating_sub(1);
    state.storage_inspector.cursor = (state.storage_inspector.cursor + 1).min(max);
}

fn enter_selected_directory(state: &mut AppState) {
    if state.storage_inspector.view != StorageView::Directory {
        return;
    }
    let Some(row) = visible_rows(state)
        .get(state.storage_inspector.cursor)
        .cloned()
    else {
        return;
    };
    if row.kind == UsageKind::Directory {
        state.storage_inspector.current_dir = Some(row.path);
        state.storage_inspector.reset_cursor();
    }
}

fn go_to_parent(state: &mut AppState) {
    if state.storage_inspector.view != StorageView::Directory {
        state.storage_inspector.view = StorageView::Directory;
        state.storage_inspector.reset_cursor();
        return;
    }
    let (Some(root), Some(current)) = (
        state.storage_inspector.root.clone(),
        state.storage_inspector.current_dir.clone(),
    ) else {
        return;
    };
    if current == root {
        return;
    }
    if let Some(parent) = current.parent()
        && parent.starts_with(&root)
    {
        state.storage_inspector.current_dir = Some(parent.to_path_buf());
        state.storage_inspector.reset_cursor();
    }
}

fn current_snapshot(state: &AppState) -> Option<Arc<UsageScanResult>> {
    state
        .storage_inspector
        .job_id
        .as_deref()
        .and_then(|id| state.storage_scan_snapshots.get(id))
}

fn visible_rows(state: &AppState) -> Vec<StorageRow> {
    let Some(snapshot) = current_snapshot(state) else {
        return Vec::new();
    };
    rows_for_snapshot(&snapshot, &state.storage_inspector)
}

fn rows_for_snapshot(
    snapshot: &UsageScanResult,
    ui: &StorageInspectorUiState,
) -> Vec<StorageRow> {
    let mut rows = match ui.view {
        StorageView::Directory => {
            let current = ui.current_dir.as_deref().unwrap_or(snapshot.root.as_path());
            snapshot
                .records
                .iter()
                .filter(|record| record.path != current && record.path.parent() == Some(current))
                .map(|record| row_from_record(record, ui.basis))
                .collect::<Vec<_>>()
        }
        StorageView::TopFiles => snapshot
            .records
            .iter()
            .filter(|record| record.kind == UsageKind::File && !record.hardlink_duplicate)
            .map(|record| row_from_record(record, ui.basis))
            .collect::<Vec<_>>(),
    };

    sort_rows(&mut rows, ui.sort);
    if ui.view == StorageView::TopFiles {
        rows.truncate(TOP_FILES_LIMIT);
    }
    rows
}

fn row_from_record(record: &UsageRecord, basis: StorageSizeBasis) -> StorageRow {
    StorageRow {
        path: record.path.clone(),
        kind: record.kind,
        bytes: basis.record_bytes(record),
        items: record.subtree_entries,
        metadata_error: record.metadata_error,
        hardlink_duplicate: record.hardlink_duplicate,
    }
}

fn sort_rows(rows: &mut [StorageRow], sort: StorageSort) {
    rows.sort_by(|left, right| match sort {
        StorageSort::Size => right
            .bytes
            .cmp(&left.bytes)
            .then_with(|| left.path.cmp(&right.path)),
        StorageSort::Name => left
            .name()
            .to_lowercase()
            .cmp(&right.name().to_lowercase())
            .then_with(|| left.path.cmp(&right.path)),
        StorageSort::Items => right
            .items
            .cmp(&left.items)
            .then_with(|| right.bytes.cmp(&left.bytes))
            .then_with(|| left.path.cmp(&right.path)),
    });
}

pub fn render_storage_inspector(frame: &mut ratatui::Frame, area: Rect, state: &mut AppState) {
    let popup = centered_rect(90, 86, area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(" Storage Inspector · read-only ")
        .borders(Borders::ALL);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(4),
            Constraint::Length(3),
        ])
        .split(inner);

    render_header(frame, rows[0], state);
    render_body(frame, rows[1], state);
    render_footer(frame, rows[2], state);
}

fn render_header(frame: &mut ratatui::Frame, area: Rect, state: &AppState) {
    let root = state
        .storage_inspector
        .root
        .as_deref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "-".into());
    let current = state
        .storage_inspector
        .current_dir
        .as_deref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| root.clone());

    let (status, progress) = storage_status(state);
    let text = vec![
        Line::from(vec![
            Span::styled(format!("{status}  "), Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(progress),
        ]),
        Line::from(format!("root: {root}")),
        Line::from(format!(
            "view: {} · basis: {} · sort: {} · path: {}",
            state.storage_inspector.view.label(),
            state.storage_inspector.basis.label(),
            state.storage_inspector.sort.label(),
            current
        )),
    ];
    frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), area);
}

fn storage_status(state: &AppState) -> (String, String) {
    let Some(id) = state.storage_inspector.job_id.as_deref() else {
        return ("IDLE".into(), "no scan".into());
    };
    let Some(job) = state.jobs.iter().find(|job| job.id == id) else {
        return ("UNKNOWN".into(), format!("job {id} unavailable"));
    };

    match job.status {
        JobStatus::Pending => ("PENDING".into(), "waiting to start".into()),
        JobStatus::Running | JobStatus::Cancelling => {
            let prefix = if job.status == JobStatus::Cancelling {
                "CANCELLING"
            } else {
                "RUNNING"
            };
            match &job.progress {
                JobProgress::StorageScan(progress) => (
                    prefix.into(),
                    format!(
                        "{} entries · {} logical · {} allocated · {} errors · total unknown",
                        progress.entries_seen,
                        format_bytes_u128(progress.logical_bytes),
                        format_bytes_u128(progress.allocated_bytes),
                        progress.errors
                    ),
                ),
                _ => (prefix.into(), "observing filesystem · total unknown".into()),
            }
        }
        JobStatus::Completed | JobStatus::Cancelled => match &job.result {
            Some(JobResult::StorageScan(summary)) => match summary.outcome {
                UsageScanOutcome::Complete => (
                    "COMPLETE".into(),
                    format!(
                        "{} entries · {} allocated · {} apparent",
                        summary.totals.entries_seen,
                        format_bytes_u128(summary.totals.allocated_bytes),
                        format_bytes_u128(summary.totals.logical_bytes)
                    ),
                ),
                UsageScanOutcome::Partial => (
                    "PARTIAL".into(),
                    format!(
                        "{} entries · {} errors · results are incomplete",
                        summary.totals.entries_seen, summary.totals.errors
                    ),
                ),
                UsageScanOutcome::Cancelled => (
                    "CANCELLED".into(),
                    format!(
                        "{} observed entries · partial snapshot retained",
                        summary.totals.entries_seen
                    ),
                ),
            },
            _ => (format!("{:?}", job.status).to_uppercase(), String::new()),
        },
        JobStatus::Failed => (
            "FAILED".into(),
            job.error.clone().unwrap_or_else(|| "scan failed".into()),
        ),
        JobStatus::PausePending | JobStatus::Paused | JobStatus::RetryWaiting => (
            format!("{:?}", job.status).to_uppercase(),
            "unexpected storage-scan lifecycle state".into(),
        ),
    }
}

fn render_body(frame: &mut ratatui::Frame, area: Rect, state: &mut AppState) {
    let rows = visible_rows(state);
    state.storage_inspector.cursor = state
        .storage_inspector
        .cursor
        .min(rows.len().saturating_sub(1));

    if rows.is_empty() {
        let message = if current_snapshot(state).is_some() {
            match state.storage_inspector.view {
                StorageView::Directory => "No retained children in this directory",
                StorageView::TopFiles => "No regular files in the retained snapshot",
            }
        } else {
            "Scanning… results will appear when the immutable snapshot is available"
        };
        frame.render_widget(Paragraph::new(message), area);
        return;
    }

    let items = rows
        .iter()
        .map(|row| {
            let kind = match row.kind {
                UsageKind::Directory => "d",
                UsageKind::File => "f",
                UsageKind::Symlink => "l",
                UsageKind::Other => "?",
            };
            let warning = if row.metadata_error {
                " !metadata"
            } else if row.hardlink_duplicate {
                " hardlink-duplicate"
            } else {
                ""
            };
            ListItem::new(format!(
                "{kind}  {:>11}  {:>8} items  {}{}",
                format_bytes_u128(row.bytes),
                row.items,
                row.path.display(),
                warning
            ))
        })
        .collect::<Vec<_>>();

    let mut list_state = ListState::default();
    list_state.select(Some(state.storage_inspector.cursor));
    let list = List::new(items).highlight_symbol("› ").highlight_style(
        Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED),
    );
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn render_footer(frame: &mut ratatui::Frame, area: Rect, state: &AppState) {
    let running = state
        .storage_inspector
        .job_id
        .as_deref()
        .and_then(|id| state.jobs.iter().find(|job| job.id == id))
        .is_some_and(|job| !job.status.is_terminal());
    let cancel = if running { " · c cancel scan" } else { "" };
    let text = format!(
        "↑/↓ j/k move · Enter/l drill down · ←/h back · b basis · s sort · t top files{cancel} · Esc close"
    );
    frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), area);
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

fn format_bytes_u128(bytes: u128) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if value.is_finite() {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::{JobKind, JobManager};
    use crate::storage_inspector::{UsageTotals, UsageScanOutcome};
    use crate::storage_inspector_snapshot::StorageScanSnapshotStore;
    use crate::vfs::Location;
    use tokio::sync::mpsc;

    fn record(
        path: &str,
        kind: UsageKind,
        logical: u128,
        allocated: u128,
        items: u64,
    ) -> UsageRecord {
        UsageRecord {
            path: PathBuf::from(path),
            depth: Path::new(path).components().count(),
            kind,
            logical_bytes: logical.min(u128::from(u64::MAX)) as u64,
            allocated_bytes: allocated.min(u128::from(u64::MAX)) as u64,
            subtree_logical_bytes: logical,
            subtree_allocated_bytes: allocated,
            subtree_entries: items,
            hardlink_duplicate: false,
            metadata_error: false,
        }
    }

    fn snapshot(outcome: UsageScanOutcome) -> UsageScanResult {
        UsageScanResult {
            root: PathBuf::from("/root"),
            outcome,
            totals: UsageTotals {
                logical_bytes: 10_000,
                allocated_bytes: 8_000,
                files: 3,
                directories: 3,
                errors: usize::from(outcome == UsageScanOutcome::Partial) as u64,
                entries_seen: 6,
                ..UsageTotals::default()
            },
            records: vec![
                record("/root", UsageKind::Directory, 10_000, 8_000, 6),
                record("/root/a", UsageKind::Directory, 8_000, 1_000, 3),
                record("/root/a/deep", UsageKind::File, 7_000, 512, 1),
                record("/root/b", UsageKind::Directory, 2_000, 7_000, 2),
                record("/root/b/file", UsageKind::File, 1_000, 6_000, 1),
                record("/root/z", UsageKind::File, 500, 400, 1),
            ],
            top_files: Vec::new(),
        }
    }

    #[test]
    fn directory_view_contains_only_immediate_children() {
        let scan = snapshot(UsageScanOutcome::Complete);
        let ui = StorageInspectorUiState {
            root: Some(PathBuf::from("/root")),
            current_dir: Some(PathBuf::from("/root")),
            ..StorageInspectorUiState::default()
        };
        let rows = rows_for_snapshot(&scan, &ui);
        let paths = rows.into_iter().map(|row| row.path).collect::<Vec<_>>();
        assert!(paths.contains(&PathBuf::from("/root/a")));
        assert!(paths.contains(&PathBuf::from("/root/b")));
        assert!(paths.contains(&PathBuf::from("/root/z")));
        assert!(!paths.contains(&PathBuf::from("/root/a/deep")));
    }

    #[test]
    fn allocated_and_apparent_can_rank_sparse_truth_differently() {
        let scan = snapshot(UsageScanOutcome::Complete);
        let mut ui = StorageInspectorUiState {
            root: Some(PathBuf::from("/root")),
            current_dir: Some(PathBuf::from("/root")),
            sort: StorageSort::Size,
            basis: StorageSizeBasis::Allocated,
            ..StorageInspectorUiState::default()
        };
        let allocated = rows_for_snapshot(&scan, &ui);
        assert_eq!(allocated[0].path, PathBuf::from("/root/b"));
        ui.basis = StorageSizeBasis::Logical;
        let logical = rows_for_snapshot(&scan, &ui);
        assert_eq!(logical[0].path, PathBuf::from("/root/a"));
    }

    #[test]
    fn name_and_items_sorting_are_deterministic() {
        let scan = snapshot(UsageScanOutcome::Complete);
        let mut ui = StorageInspectorUiState {
            root: Some(PathBuf::from("/root")),
            current_dir: Some(PathBuf::from("/root")),
            sort: StorageSort::Name,
            ..StorageInspectorUiState::default()
        };
        let by_name = rows_for_snapshot(&scan, &ui);
        assert_eq!(by_name[0].path, PathBuf::from("/root/a"));
        assert_eq!(by_name[1].path, PathBuf::from("/root/b"));
        ui.sort = StorageSort::Items;
        let by_items = rows_for_snapshot(&scan, &ui);
        assert_eq!(by_items[0].path, PathBuf::from("/root/a"));
    }

    #[test]
    fn top_files_uses_all_unique_file_records_and_current_basis() {
        let mut scan = snapshot(UsageScanOutcome::Complete);
        let mut duplicate = record("/root/dup", UsageKind::File, 99_000, 99_000, 1);
        duplicate.hardlink_duplicate = true;
        scan.records.push(duplicate);
        let ui = StorageInspectorUiState {
            root: Some(PathBuf::from("/root")),
            current_dir: Some(PathBuf::from("/root")),
            view: StorageView::TopFiles,
            basis: StorageSizeBasis::Logical,
            sort: StorageSort::Size,
            ..StorageInspectorUiState::default()
        };
        let rows = rows_for_snapshot(&scan, &ui);
        assert_eq!(rows[0].path, PathBuf::from("/root/a/deep"));
        assert!(!rows.iter().any(|row| row.path == PathBuf::from("/root/dup")));
    }

    #[test]
    fn parent_navigation_never_escapes_scan_root() {
        let mut state = AppState::default();
        state.storage_inspector.root = Some(PathBuf::from("/root"));
        state.storage_inspector.current_dir = Some(PathBuf::from("/root/a"));
        go_to_parent(&mut state);
        assert_eq!(state.storage_inspector.current_dir, Some(PathBuf::from("/root")));
        go_to_parent(&mut state);
        assert_eq!(state.storage_inspector.current_dir, Some(PathBuf::from("/root")));
    }

    #[test]
    fn partial_status_is_distinct_from_complete() {
        let mut state = AppState::default();
        let manager = JobManager::new();
        let (tx, _rx) = mpsc::unbounded_channel();
        let job = manager.create_job(
            "storage",
            JobKind::StorageScan,
            "scan",
            Some(Location::Local(PathBuf::from("/root"))),
            None,
        );
        assert!(manager.publish_event(&tx, crate::jobs::JobEvent::Running { id: job.id.clone() }));
        assert!(manager.publish_event(
            &tx,
            crate::jobs::JobEvent::Completed {
                id: job.id.clone(),
                result: JobResult::StorageScan(crate::storage_inspector_job::StorageScanSummary {
                    root: PathBuf::from("/root"),
                    outcome: UsageScanOutcome::Partial,
                    totals: UsageTotals { errors: 2, entries_seen: 5, ..UsageTotals::default() },
                }),
            },
        ));
        state.jobs = manager.snapshot();
        state.storage_inspector.job_id = Some(job.id);
        let (label, detail) = storage_status(&state);
        assert_eq!(label, "PARTIAL");
        assert!(detail.contains("2 errors"));
        assert!(detail.contains("incomplete"));
    }

    #[tokio::test]
    async fn local_launch_creates_one_storage_job_and_opens_overlay() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = AppState::default();
        state.left.location = Location::Local(temp.path().to_path_buf());
        state.active = crate::app::Pane::Left;
        let manager = JobManager::new();
        let (tx, mut rx) = mpsc::unbounded_channel();
        state.job_manager = Some(manager.clone());
        state.job_events = Some(tx);
        state.storage_scan_snapshots = StorageScanSnapshotStore::new();

        let id = launch_storage_inspector(&mut state).expect("local launch");
        assert_eq!(state.active_overlay(), Some(OverlayKind::StorageInspector));
        let scans = manager
            .snapshot()
            .into_iter()
            .filter(|job| job.kind == JobKind::StorageScan)
            .collect::<Vec<_>>();
        assert_eq!(scans.len(), 1);
        assert_eq!(scans[0].id, id);

        for _ in 0..2_000 {
            if manager.get(&id).is_some_and(|job| job.status.is_terminal()) {
                break;
            }
            let _ = rx.try_recv();
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        assert!(manager.get(&id).unwrap().status.is_terminal());
    }

    #[test]
    fn non_local_launch_fails_closed_without_job() {
        let mut state = AppState::default();
        state.left.location = Location::Sftp {
            host: "example".into(),
            path: "/".into(),
        };
        state.active = crate::app::Pane::Left;
        let result = launch_storage_inspector(&mut state);
        assert_eq!(result.unwrap_err(), "Storage Inspector is available for local paths only");
        assert_eq!(state.active_overlay(), None);
    }
}
