//! Read-only Filesystems (`df++`) overlay state, interaction, filtering and rendering.

use std::cmp::Ordering;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use crate::app::{AppState, OverlayKind};
use crate::filesystem_usage::{
    MountCategory, MountRecord, MountSnapshot, MountStats, MountStatsState, collect_mount_snapshot,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilesystemMode {
    #[default]
    Blocks,
    Inodes,
}

impl FilesystemMode {
    pub const fn toggle(self) -> Self {
        match self {
            Self::Blocks => Self::Inodes,
            Self::Inodes => Self::Blocks,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Blocks => "blocks",
            Self::Inodes => "inodes",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilesystemSort {
    #[default]
    MountPoint,
    Size,
    Used,
    Available,
    Usage,
    Type,
}

impl FilesystemSort {
    pub const fn next(self) -> Self {
        match self {
            Self::MountPoint => Self::Size,
            Self::Size => Self::Used,
            Self::Used => Self::Available,
            Self::Available => Self::Usage,
            Self::Usage => Self::Type,
            Self::Type => Self::MountPoint,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::MountPoint => "mount↑",
            Self::Size => "size↓",
            Self::Used => "used↓",
            Self::Available => "avail↓",
            Self::Usage => "usage↓",
            Self::Type => "type↑",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilesystemFilter {
    /// Default operator view: useful data filesystems, while pseudo/special
    /// mounts remain one keypress away instead of being silently discarded.
    #[default]
    Useful,
    All,
    Local,
    Network,
    Fuse,
    Special,
}

impl FilesystemFilter {
    pub const fn next(self) -> Self {
        match self {
            Self::Useful => Self::All,
            Self::All => Self::Local,
            Self::Local => Self::Network,
            Self::Network => Self::Fuse,
            Self::Fuse => Self::Special,
            Self::Special => Self::Useful,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Useful => "useful",
            Self::All => "all",
            Self::Local => "local",
            Self::Network => "network",
            Self::Fuse => "fuse",
            Self::Special => "special",
        }
    }

    fn includes(self, category: MountCategory) -> bool {
        match self {
            Self::Useful => category != MountCategory::Special,
            Self::All => true,
            Self::Local => category == MountCategory::Local,
            Self::Network => category == MountCategory::Network,
            Self::Fuse => category == MountCategory::Fuse,
            Self::Special => category == MountCategory::Special,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct FilesystemUiState {
    pub snapshot: Option<MountSnapshot>,
    pub cursor: usize,
    pub mode: FilesystemMode,
    pub sort: FilesystemSort,
    pub filter: FilesystemFilter,
    pub last_error: Option<String>,
}

pub fn launch_filesystems(state: &mut AppState) -> Result<(), String> {
    let result = refresh_filesystems(state);
    state.open_overlay(OverlayKind::Filesystems);
    result
}

pub fn refresh_filesystems(state: &mut AppState) -> Result<(), String> {
    match collect_mount_snapshot() {
        Ok(snapshot) => {
            state.filesystems.snapshot = Some(snapshot);
            state.filesystems.last_error = None;
            clamp_cursor(state);
            Ok(())
        }
        Err(error) => {
            let message = format!("Filesystem usage refresh failed: {error}");
            state.filesystems.last_error = Some(message.clone());
            Err(message)
        }
    }
}

pub fn handle_filesystems_key(state: &mut AppState, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => state.close_overlay(OverlayKind::Filesystems),
        KeyCode::Up | KeyCode::Char('k') => {
            state.filesystems.cursor = state.filesystems.cursor.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') => move_cursor_down(state),
        KeyCode::Home => state.filesystems.cursor = 0,
        KeyCode::End => {
            state.filesystems.cursor = visible_mounts(state).len().saturating_sub(1);
        }
        KeyCode::Char('i') => {
            state.filesystems.mode = state.filesystems.mode.toggle();
            state.filesystems.cursor = 0;
        }
        KeyCode::Char('s') => {
            state.filesystems.sort = state.filesystems.sort.next();
            state.filesystems.cursor = 0;
        }
        KeyCode::Char('f') => {
            state.filesystems.filter = state.filesystems.filter.next();
            state.filesystems.cursor = 0;
        }
        KeyCode::Char('r') => {
            let _ = refresh_filesystems(state);
            state.filesystems.cursor = 0;
        }
        _ => {}
    }
}

fn move_cursor_down(state: &mut AppState) {
    let last = visible_mounts(state).len().saturating_sub(1);
    state.filesystems.cursor = state.filesystems.cursor.saturating_add(1).min(last);
}

fn clamp_cursor(state: &mut AppState) {
    let last = visible_mounts(state).len().saturating_sub(1);
    state.filesystems.cursor = state.filesystems.cursor.min(last);
}

fn visible_mounts(state: &AppState) -> Vec<&MountRecord> {
    let Some(snapshot) = state.filesystems.snapshot.as_ref() else {
        return Vec::new();
    };
    let mut rows = snapshot
        .mounts
        .iter()
        .filter(|mount| state.filesystems.filter.includes(mount.category))
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| compare_mounts(left, right, &state.filesystems));
    rows
}

fn compare_mounts(left: &MountRecord, right: &MountRecord, ui: &FilesystemUiState) -> Ordering {
    let primary = match ui.sort {
        FilesystemSort::MountPoint => cmp_os_path(
            left.info.mount_point.as_os_str(),
            right.info.mount_point.as_os_str(),
        ),
        FilesystemSort::Type => left.info.fs_type.cmp(&right.info.fs_type),
        FilesystemSort::Size => cmp_metric_desc(
            metric(left, ui.mode, Metric::Size),
            metric(right, ui.mode, Metric::Size),
        ),
        FilesystemSort::Used => cmp_metric_desc(
            metric(left, ui.mode, Metric::Used),
            metric(right, ui.mode, Metric::Used),
        ),
        FilesystemSort::Available => cmp_metric_desc(
            metric(left, ui.mode, Metric::Available),
            metric(right, ui.mode, Metric::Available),
        ),
        FilesystemSort::Usage => cmp_metric_desc(
            metric(left, ui.mode, Metric::Usage),
            metric(right, ui.mode, Metric::Usage),
        ),
    };

    primary
        .then_with(|| {
            cmp_os_path(
                left.info.mount_point.as_os_str(),
                right.info.mount_point.as_os_str(),
            )
        })
        .then_with(|| left.info.mount_id.cmp(&right.info.mount_id))
}

fn cmp_os_path(left: &OsStr, right: &OsStr) -> Ordering {
    left.as_bytes().cmp(right.as_bytes())
}

fn cmp_metric_desc(left: Option<u128>, right: Option<u128>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => right.cmp(&left),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

#[derive(Debug, Clone, Copy)]
enum Metric {
    Size,
    Used,
    Available,
    Usage,
}

fn metric(mount: &MountRecord, mode: FilesystemMode, metric: Metric) -> Option<u128> {
    let stats = mount.stats.available()?;
    match (mode, metric) {
        (FilesystemMode::Blocks, Metric::Size) => Some(stats.total_bytes),
        (FilesystemMode::Blocks, Metric::Used) => stats.used_bytes,
        (FilesystemMode::Blocks, Metric::Available) => Some(stats.available_bytes),
        (FilesystemMode::Blocks, Metric::Usage) => stats.usage_tenths_percent.map(u128::from),
        (FilesystemMode::Inodes, Metric::Size) => Some(stats.total_inodes),
        (FilesystemMode::Inodes, Metric::Used) => stats.used_inodes,
        (FilesystemMode::Inodes, Metric::Available) => Some(stats.available_inodes),
        (FilesystemMode::Inodes, Metric::Usage) => stats.inode_usage_tenths_percent.map(u128::from),
    }
}

pub fn render_filesystems(frame: &mut ratatui::Frame<'_>, area: Rect, state: &AppState) {
    let popup = centered_rect(94, 82, area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(" Filesystems · df++ · read-only ")
        .borders(Borders::ALL);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(inner);

    let snapshot_summary = state
        .filesystems
        .snapshot
        .as_ref()
        .map(|snapshot| {
            format!(
                "{} mounts · {} parse error(s) · {} unavailable · {} safely unprobed",
                snapshot.mounts.len(),
                snapshot.parse_errors.len(),
                snapshot.unavailable_count(),
                snapshot.intentionally_skipped_count()
            )
        })
        .unwrap_or_else(|| "no filesystem snapshot".to_string());

    let title = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("mode ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(state.filesystems.mode.label()),
            Span::raw("  ·  "),
            Span::styled("sort ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(state.filesystems.sort.label()),
            Span::raw("  ·  "),
            Span::styled("filter ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(state.filesystems.filter.label()),
        ]),
        Line::from(snapshot_summary),
    ]);
    frame.render_widget(title, chunks[0]);

    let header = match state.filesystems.mode {
        FilesystemMode::Blocks => format_row(
            "Mountpoint",
            "Source",
            "Type",
            "Size",
            "Used",
            "Avail",
            "Use%",
        ),
        FilesystemMode::Inodes => format_row(
            "Mountpoint",
            "Source",
            "Type",
            "Inodes",
            "Used",
            "Avail",
            "Use%",
        ),
    };
    frame.render_widget(
        Paragraph::new(header).style(Style::default().add_modifier(Modifier::BOLD)),
        chunks[1],
    );

    let rows = visible_mounts(state);
    let items = rows
        .iter()
        .map(|mount| ListItem::new(render_mount_row(mount, state.filesystems.mode)))
        .collect::<Vec<_>>();
    let list = List::new(items)
        .highlight_symbol("> ")
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD));
    let mut list_state = ListState::default();
    if !rows.is_empty() {
        list_state.select(Some(state.filesystems.cursor.min(rows.len() - 1)));
    }
    frame.render_stateful_widget(list, chunks[2], &mut list_state);

    let footer = if let Some(error) = state.filesystems.last_error.as_deref() {
        format!("r refresh · i blocks/inodes · s sort · f filter · Esc close\n{error}")
    } else {
        "r refresh · i blocks/inodes · s sort · f filter · j/k move · Esc close\nNETWORK/AUTOFS are visible but intentionally not probed; ERR is unavailable truth".to_string()
    };
    frame.render_widget(Paragraph::new(footer).wrap(Wrap { trim: true }), chunks[3]);
}

fn render_mount_row(mount: &MountRecord, mode: FilesystemMode) -> String {
    let mount_point = mount.info.mount_point.to_string_lossy();
    let source = mount.info.mount_source.to_string_lossy();
    let fs_type = if mount.read_only {
        format!("{} ro", mount.info.fs_type)
    } else {
        mount.info.fs_type.clone()
    };

    let (size, used, available, usage) = match &mount.stats {
        MountStatsState::Available(stats) => render_available_stats(stats, mode),
        MountStatsState::SkippedAutoFs => state_cells("AUTOFS"),
        MountStatsState::SkippedNetwork => state_cells("NETWORK"),
        MountStatsState::Unavailable(_) => state_cells("ERR"),
    };

    format_row(
        &mount_point,
        &source,
        &fs_type,
        &size,
        &used,
        &available,
        &usage,
    )
}

fn render_available_stats(
    stats: &MountStats,
    mode: FilesystemMode,
) -> (String, String, String, String) {
    match mode {
        FilesystemMode::Blocks => (
            format_bytes(stats.total_bytes),
            stats
                .used_bytes
                .map(format_bytes)
                .unwrap_or_else(|| "?".into()),
            format_bytes(stats.available_bytes),
            format_percent(stats.usage_tenths_percent),
        ),
        FilesystemMode::Inodes => (
            format_count(stats.total_inodes),
            stats
                .used_inodes
                .map(format_count)
                .unwrap_or_else(|| "?".into()),
            format_count(stats.available_inodes),
            format_percent(stats.inode_usage_tenths_percent),
        ),
    }
}

fn state_cells(label: &str) -> (String, String, String, String) {
    (label.into(), "—".into(), "—".into(), "—".into())
}

fn format_percent(value: Option<u16>) -> String {
    value
        .map(|tenths| format!("{}.{:01}%", tenths / 10, tenths % 10))
        .unwrap_or_else(|| "?".into())
}

fn format_count(value: u128) -> String {
    let text = value.to_string();
    let mut grouped = String::with_capacity(text.len() + text.len() / 3);
    for (index, ch) in text.chars().enumerate() {
        if index > 0 && (text.len() - index).is_multiple_of(3) {
            grouped.push('_');
        }
        grouped.push(ch);
    }
    grouped
}

fn format_bytes(value: u128) -> String {
    const UNITS: &[(&str, u128)] = &[
        ("EiB", 1u128 << 60),
        ("PiB", 1u128 << 50),
        ("TiB", 1u128 << 40),
        ("GiB", 1u128 << 30),
        ("MiB", 1u128 << 20),
        ("KiB", 1u128 << 10),
    ];
    for (label, unit) in UNITS {
        if value >= *unit {
            let whole = value / *unit;
            let tenth = (value % *unit) * 10 / *unit;
            return format!("{whole}.{tenth} {label}");
        }
    }
    format!("{value} B")
}

fn format_row(
    mountpoint: &str,
    source: &str,
    fs_type: &str,
    size: &str,
    used: &str,
    available: &str,
    usage: &str,
) -> String {
    format!(
        "{:<28.28} {:<20.20} {:<14.14} {:>11.11} {:>11.11} {:>11.11} {:>7.7}",
        mountpoint, source, fs_type, size, used, available, usage
    )
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
    use crate::filesystem_usage::{MountInfo, MountStatsState};
    use crossterm::event::KeyModifiers;
    use std::ffi::OsString;
    use std::path::PathBuf;

    fn stats(total: u128, used: u128, available: u128, usage: u16) -> MountStats {
        MountStats {
            total_bytes: total,
            used_bytes: Some(used),
            free_bytes: total - used,
            available_bytes: available,
            reserved_bytes: Some((total - used).saturating_sub(available)),
            usage_tenths_percent: Some(usage),
            total_inodes: total,
            used_inodes: Some(used),
            free_inodes: total - used,
            available_inodes: available,
            inode_usage_tenths_percent: Some(usage),
        }
    }

    fn mount(
        id: u32,
        point: &str,
        fs_type: &str,
        category: MountCategory,
        stats: MountStatsState,
    ) -> MountRecord {
        MountRecord {
            info: MountInfo {
                mount_id: id,
                parent_id: 0,
                major: 0,
                minor: id,
                root: PathBuf::from("/"),
                mount_point: PathBuf::from(point),
                mount_options: vec!["rw".into()],
                optional_fields: Vec::new(),
                fs_type: fs_type.into(),
                mount_source: OsString::from("source"),
                super_options: vec!["rw".into()],
            },
            category,
            read_only: false,
            stats,
        }
    }

    fn state_with_mounts(mounts: Vec<MountRecord>) -> AppState {
        let mut state = AppState::default();
        state.filesystems.snapshot = Some(MountSnapshot {
            mounts,
            parse_errors: Vec::new(),
        });
        state
    }

    #[test]
    fn useful_default_hides_special_but_keeps_other_categories() {
        let state = state_with_mounts(vec![
            mount(
                1,
                "/",
                "ext4",
                MountCategory::Local,
                MountStatsState::Available(stats(100, 50, 40, 500)),
            ),
            mount(
                2,
                "/net",
                "nfs4",
                MountCategory::Network,
                MountStatsState::SkippedNetwork,
            ),
            mount(
                3,
                "/fuse",
                "fuse.portal",
                MountCategory::Fuse,
                MountStatsState::Available(stats(50, 10, 35, 200)),
            ),
            mount(
                4,
                "/proc",
                "proc",
                MountCategory::Special,
                MountStatsState::Available(stats(0, 0, 0, 0)),
            ),
        ]);
        let rows = visible_mounts(&state);
        assert_eq!(rows.len(), 3);
        assert!(
            rows.iter()
                .all(|row| row.category != MountCategory::Special)
        );
    }

    #[test]
    fn filter_cycle_reaches_all_categories() {
        let mut filter = FilesystemFilter::Useful;
        filter = filter.next();
        assert_eq!(filter, FilesystemFilter::All);
        filter = filter.next();
        assert_eq!(filter, FilesystemFilter::Local);
        filter = filter.next();
        assert_eq!(filter, FilesystemFilter::Network);
        filter = filter.next();
        assert_eq!(filter, FilesystemFilter::Fuse);
        filter = filter.next();
        assert_eq!(filter, FilesystemFilter::Special);
        assert_eq!(filter.next(), FilesystemFilter::Useful);
    }

    #[test]
    fn unavailable_metrics_sort_after_available_rows() {
        let mut state = state_with_mounts(vec![
            mount(
                1,
                "/missing",
                "ext4",
                MountCategory::Local,
                MountStatsState::Unavailable("gone".into()),
            ),
            mount(
                2,
                "/large",
                "ext4",
                MountCategory::Local,
                MountStatsState::Available(stats(200, 150, 40, 750)),
            ),
            mount(
                3,
                "/small",
                "ext4",
                MountCategory::Local,
                MountStatsState::Available(stats(100, 10, 80, 100)),
            ),
        ]);
        state.filesystems.sort = FilesystemSort::Size;
        let rows = visible_mounts(&state);
        assert_eq!(rows[0].info.mount_point, PathBuf::from("/large"));
        assert_eq!(rows[1].info.mount_point, PathBuf::from("/small"));
        assert_eq!(rows[2].info.mount_point, PathBuf::from("/missing"));
    }

    #[test]
    fn inode_mode_uses_inode_metrics_for_sorting() {
        let mut state = state_with_mounts(vec![
            mount(
                1,
                "/a",
                "ext4",
                MountCategory::Local,
                MountStatsState::Available(MountStats {
                    total_bytes: 9999,
                    used_bytes: Some(9000),
                    free_bytes: 999,
                    available_bytes: 900,
                    reserved_bytes: Some(99),
                    usage_tenths_percent: Some(900),
                    total_inodes: 10,
                    used_inodes: Some(1),
                    free_inodes: 9,
                    available_inodes: 9,
                    inode_usage_tenths_percent: Some(100),
                }),
            ),
            mount(
                2,
                "/b",
                "ext4",
                MountCategory::Local,
                MountStatsState::Available(MountStats {
                    total_bytes: 100,
                    used_bytes: Some(10),
                    free_bytes: 90,
                    available_bytes: 80,
                    reserved_bytes: Some(10),
                    usage_tenths_percent: Some(100),
                    total_inodes: 100,
                    used_inodes: Some(90),
                    free_inodes: 10,
                    available_inodes: 10,
                    inode_usage_tenths_percent: Some(900),
                }),
            ),
        ]);
        state.filesystems.mode = FilesystemMode::Inodes;
        state.filesystems.sort = FilesystemSort::Used;
        let rows = visible_mounts(&state);
        assert_eq!(rows[0].info.mount_point, PathBuf::from("/b"));
    }

    #[test]
    fn key_controls_toggle_mode_sort_and_filter() {
        let mut state = AppState::default();
        handle_filesystems_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE),
        );
        assert_eq!(state.filesystems.mode, FilesystemMode::Inodes);
        handle_filesystems_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE),
        );
        assert_eq!(state.filesystems.sort, FilesystemSort::Size);
        handle_filesystems_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE),
        );
        assert_eq!(state.filesystems.filter, FilesystemFilter::All);
    }

    #[test]
    fn escape_closes_filesystems_overlay_without_side_effects() {
        let mut state = AppState::default();
        state.open_overlay(OverlayKind::Filesystems);
        assert_eq!(state.active_overlay(), Some(OverlayKind::Filesystems));
        handle_filesystems_key(&mut state, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(state.active_overlay(), None);
    }

    #[test]
    fn skipped_states_render_explicitly() {
        let network = mount(
            1,
            "/net",
            "nfs4",
            MountCategory::Network,
            MountStatsState::SkippedNetwork,
        );
        let autofs = mount(
            2,
            "/auto",
            "autofs",
            MountCategory::Special,
            MountStatsState::SkippedAutoFs,
        );
        assert!(render_mount_row(&network, FilesystemMode::Blocks).contains("NETWORK"));
        assert!(render_mount_row(&autofs, FilesystemMode::Blocks).contains("AUTOFS"));
    }

    #[test]
    fn sort_cycle_reaches_all_six_sorts() {
        let mut sort = FilesystemSort::MountPoint;
        let mut seen = Vec::new();
        for _ in 0..6 {
            sort = sort.next();
            seen.push(sort);
        }
        assert_eq!(
            seen,
            vec![
                FilesystemSort::Size,
                FilesystemSort::Used,
                FilesystemSort::Available,
                FilesystemSort::Usage,
                FilesystemSort::Type,
                FilesystemSort::MountPoint,
            ]
        );
    }

    #[test]
    fn unavailable_render_explicitly_err() {
        let record = mount(
            9,
            "/broken",
            "ext4",
            MountCategory::Local,
            MountStatsState::Unavailable("i/o error".into()),
        );
        assert!(render_mount_row(&record, FilesystemMode::Blocks).contains("ERR"));
    }

    #[test]
    fn filesystems_overlay_is_exclusive_versus_storage_inspector() {
        let mut state = AppState::default();
        state.open_overlay(OverlayKind::StorageInspector);
        assert_eq!(state.active_overlay(), Some(OverlayKind::StorageInspector));
        // Opening Filesystems closes the prior overlay (overlay state machine).
        state.open_overlay(OverlayKind::Filesystems);
        assert_eq!(state.active_overlay(), Some(OverlayKind::Filesystems));
        assert!(!state.show_storage_inspector);
        assert!(state.show_filesystems);
    }
}
