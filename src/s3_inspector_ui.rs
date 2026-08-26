//! Read-only S3 Inspector presentation and JobManager adapter.

use std::sync::{Arc, Mutex};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::app::{AppState, OverlayKind};
use crate::jobs::{JobEvent, JobKind, JobProgress, JobResult, JobStatus, Progress};
use crate::s3_inspector::{
    S3InspectionScope, S3InspectionTarget, S3InspectorSnapshot, S3ScanOutcome, inspect_object,
    scan_scope, scope_from_location,
};
use crate::vfs::{EntryIdentity, ListedEntry, Location};

#[derive(Debug, Clone)]
pub struct S3InspectorUiState {
    pub job_id: String,
    pub target: S3InspectionTarget,
    pub snapshot: Arc<Mutex<Option<Arc<S3InspectorSnapshot>>>>,
    pub scroll: u16,
}

impl S3InspectorUiState {
    fn snapshot(&self) -> Option<Arc<S3InspectorSnapshot>> {
        self.snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

fn identity_matches_location(location: &Location, target: &str, bucket: &str) -> bool {
    match location {
        Location::S3 {
            target: current_target,
            bucket: current_bucket,
            ..
        } => {
            current_target == target
                && current_bucket
                    .as_deref()
                    .is_none_or(|current_bucket| current_bucket == bucket)
        }
        _ => false,
    }
}

pub fn target_from_context(
    location: &Location,
    focused: Option<&ListedEntry>,
) -> Result<S3InspectionTarget, String> {
    let Location::S3 { .. } = location else {
        return Err("S3 Inspector requires an S3 pane".into());
    };

    if let Some(listed) = focused {
        match &listed.identity {
            EntryIdentity::S3Object(reference) => {
                if !identity_matches_location(location, &reference.target, &reference.bucket) {
                    return Err("S3 Inspector target is stale for the active pane".into());
                }
                return Ok(S3InspectionTarget::Object(reference.clone()));
            }
            EntryIdentity::S3Prefix(reference) => {
                if !identity_matches_location(location, &reference.target, &reference.bucket) {
                    return Err("S3 Inspector prefix is stale for the active pane".into());
                }
                return Ok(S3InspectionTarget::Scope(S3InspectionScope::Prefix(
                    reference.clone(),
                )));
            }
            EntryIdentity::S3Bucket(reference) => {
                if !identity_matches_location(location, &reference.target, &reference.bucket) {
                    return Err("S3 Inspector bucket is stale for the active pane".into());
                }
                return Ok(S3InspectionTarget::Scope(S3InspectionScope::Bucket(
                    reference.clone(),
                )));
            }
            _ => {}
        }
    }

    scope_from_location(location)
        .map(S3InspectionTarget::Scope)
        .ok_or_else(|| "Choose an S3 bucket, prefix, or exact object to inspect".into())
}

pub fn launch_s3_inspector(
    state: &mut AppState,
    focused: Option<&ListedEntry>,
) -> Result<String, String> {
    let location = state.active_pane().location.clone();
    let target = target_from_context(&location, focused)?;
    let manager = state
        .job_manager
        .clone()
        .ok_or_else(|| "S3 Inspector: JobManager is not bound".to_string())?;
    let events = state
        .job_events
        .clone()
        .ok_or_else(|| "S3 Inspector: job event channel is not bound".to_string())?;

    if let Some(existing) = state.storage_inspector.s3.as_ref()
        && existing.target == target
        && manager
            .get(&existing.job_id)
            .is_some_and(|job| !job.status.is_terminal())
    {
        let id = existing.job_id.clone();
        state.open_overlay(OverlayKind::StorageInspector);
        return Ok(id);
    }

    if let Some(existing) = state.storage_inspector.s3.take()
        && manager
            .get(&existing.job_id)
            .is_some_and(|job| !job.status.is_terminal())
    {
        manager.cancel(&existing.job_id);
    }
    if let Some(local_id) = state.storage_inspector.job_id.take() {
        if manager
            .get(&local_id)
            .is_some_and(|job| !job.status.is_terminal())
        {
            manager.cancel(&local_id);
        }
        state.storage_scan_snapshots.remove(&local_id);
    }
    state.storage_inspector.root = None;
    state.storage_inspector.current_dir = None;

    // Existing same-instance resolver: despite its historical name it returns
    // the exact Arc<S3Provider> used by listing/transfers, including the one
    // lazy client cache. The inspector intentionally introduces no new cache.
    let provider = state
        .registry
        .s3_provider_for_transfer(target.target_id())
        .map_err(|_| "S3 Inspector: configured target is unavailable".to_string())?;

    let description = format!("Inspect S3 {}", display_safe(&target_label(&target)));
    let job = manager.create_job(
        "s3-inspect",
        JobKind::Custom("s3-inspector".into()),
        description,
        Some(location),
        None,
    );
    let id = job.id.clone();
    let cancellation = job.cancel.clone();
    let snapshot = Arc::new(Mutex::new(None));
    state.storage_inspector.s3 = Some(S3InspectorUiState {
        job_id: id.clone(),
        target: target.clone(),
        snapshot: Arc::clone(&snapshot),
        scroll: 0,
    });
    state.jobs = manager.snapshot();
    state.open_overlay(OverlayKind::StorageInspector);

    let worker_id = id.clone();
    let worker_manager = manager.clone();
    let worker_events = events.clone();
    tokio::spawn(async move {
        let _ = worker_manager.publish_event(
            &worker_events,
            JobEvent::Running {
                id: worker_id.clone(),
            },
        );

        match target {
            S3InspectionTarget::Object(object) => {
                let _ = worker_manager.publish_event(
                    &worker_events,
                    JobEvent::Progress {
                        id: worker_id.clone(),
                        progress: JobProgress::Generic(Progress::Phase {
                            phase: "HEAD exact object metadata".into(),
                            percent: None,
                        }),
                    },
                );
                match inspect_object(provider, object, Arc::clone(&cancellation)).await {
                    Ok(result) => {
                        store_snapshot(&snapshot, S3InspectorSnapshot::Object(result));
                        let _ = worker_manager.publish_event(
                            &worker_events,
                            JobEvent::Completed {
                                id: worker_id,
                                result: JobResult::generic_message(
                                    "S3 object inspection complete · LiveScan",
                                ),
                            },
                        );
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                        let _ = worker_manager.publish_event(
                            &worker_events,
                            JobEvent::Cancelled {
                                id: worker_id,
                                result: JobResult::generic_message("S3 object inspection cancelled"),
                            },
                        );
                    }
                    Err(error) => {
                        let _ = worker_manager.publish_event(
                            &worker_events,
                            JobEvent::Failed {
                                id: worker_id,
                                error: error.to_string(),
                                result: None,
                            },
                        );
                    }
                }
            }
            S3InspectionTarget::Scope(scope) => {
                let progress_manager = worker_manager.clone();
                let progress_events = worker_events.clone();
                let progress_id = worker_id.clone();
                match scan_scope(provider, scope, Arc::clone(&cancellation), move |progress| {
                    let _ = progress_manager.publish_event(
                        &progress_events,
                        JobEvent::Progress {
                            id: progress_id.clone(),
                            progress: JobProgress::Generic(Progress::Phase {
                                phase: format!(
                                    "{} pages · {} objects · {} logical · total unknown",
                                    progress.pages_seen,
                                    progress.objects_seen,
                                    format_bytes_u128(progress.logical_bytes_seen)
                                ),
                                percent: None,
                            }),
                        },
                    );
                })
                .await
                {
                    Ok(S3ScanOutcome::Complete(result)) => {
                        store_snapshot(&snapshot, S3InspectorSnapshot::Scan(result));
                        let _ = worker_manager.publish_event(
                            &worker_events,
                            JobEvent::Completed {
                                id: worker_id,
                                result: JobResult::generic_message(
                                    "S3 aggregate inspection complete · LiveScan",
                                ),
                            },
                        );
                    }
                    Ok(S3ScanOutcome::Cancelled(result)) => {
                        store_snapshot(&snapshot, S3InspectorSnapshot::Scan(result));
                        let _ = worker_manager.publish_event(
                            &worker_events,
                            JobEvent::Cancelled {
                                id: worker_id,
                                result: JobResult::generic_message(
                                    "S3 aggregate inspection cancelled · partial snapshot retained",
                                ),
                            },
                        );
                    }
                    Ok(S3ScanOutcome::Partial { snapshot: result, error }) => {
                        store_snapshot(&snapshot, S3InspectorSnapshot::Scan(result));
                        let _ = worker_manager.publish_event(
                            &worker_events,
                            JobEvent::Failed {
                                id: worker_id,
                                error,
                                result: Some(JobResult::generic_message(
                                    "S3 aggregate inspection partial · snapshot retained",
                                )),
                            },
                        );
                    }
                    Err(error) => {
                        let _ = worker_manager.publish_event(
                            &worker_events,
                            JobEvent::Failed {
                                id: worker_id,
                                error: error.to_string(),
                                result: None,
                            },
                        );
                    }
                }
            }
        }
    });

    Ok(id)
}

fn store_snapshot(
    slot: &Arc<Mutex<Option<Arc<S3InspectorSnapshot>>>>,
    snapshot: S3InspectorSnapshot,
) {
    *slot
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::new(snapshot));
}

pub fn handle_s3_inspector_key(state: &mut AppState, key: KeyEvent) {
    if state.storage_inspector.s3.is_none() {
        return;
    }
    match key.code {
        KeyCode::Esc => state.close_overlay(OverlayKind::StorageInspector),
        KeyCode::Up | KeyCode::Char('k') => {
            if let Some(ui) = state.storage_inspector.s3.as_mut() {
                ui.scroll = ui.scroll.saturating_sub(1);
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let Some(ui) = state.storage_inspector.s3.as_mut() {
                ui.scroll = ui.scroll.saturating_add(1);
            }
        }
        KeyCode::PageUp => {
            if let Some(ui) = state.storage_inspector.s3.as_mut() {
                ui.scroll = ui.scroll.saturating_sub(10);
            }
        }
        KeyCode::PageDown => {
            if let Some(ui) = state.storage_inspector.s3.as_mut() {
                ui.scroll = ui.scroll.saturating_add(10);
            }
        }
        KeyCode::Home => {
            if let Some(ui) = state.storage_inspector.s3.as_mut() {
                ui.scroll = 0;
            }
        }
        KeyCode::End => {
            let last = inspector_lines(state)
                .len()
                .saturating_sub(1)
                .min(u16::MAX as usize) as u16;
            if let Some(ui) = state.storage_inspector.s3.as_mut() {
                ui.scroll = last;
            }
        }
        KeyCode::Char('c') => cancel_scan(state),
        _ => {}
    }
}

fn cancel_scan(state: &mut AppState) {
    let Some(ui) = state.storage_inspector.s3.as_ref() else {
        return;
    };
    let id = ui.job_id.clone();
    let Some(manager) = state.job_manager.clone() else {
        return;
    };
    match manager.get(&id) {
        Some(job) if !job.status.is_terminal() => {
            if manager.cancel(&id) {
                state.jobs = manager.snapshot();
                state.message = Some(format!("Cancelling S3 inspection {id}…"));
            }
        }
        Some(_) => state.message = Some("S3 inspection has already finished".into()),
        None => state.message = Some("S3 inspection job is no longer available".into()),
    }
}

pub fn render_s3_inspector(frame: &mut ratatui::Frame, area: Rect, state: &mut AppState) {
    let popup = centered_rect(92, 88, area);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .title(" S3 Inspector · read-only storage intelligence ")
        .borders(Borders::ALL);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(4), Constraint::Length(2)])
        .split(inner);
    let scroll = state
        .storage_inspector
        .s3
        .as_ref()
        .map(|ui| ui.scroll)
        .unwrap_or(0);
    frame.render_widget(
        Paragraph::new(inspector_lines(state))
            .scroll((scroll, 0))
            .wrap(Wrap { trim: false }),
        chunks[0],
    );
    frame.render_widget(
        Paragraph::new(
            "Esc close · ↑/↓ PgUp/PgDn scroll · c cancel scan · no delete / no df semantics",
        ),
        chunks[1],
    );
}

fn inspector_lines(state: &AppState) -> Vec<Line<'static>> {
    let Some(ui) = state.storage_inspector.s3.as_ref() else {
        return vec![Line::from("S3 Inspector state unavailable")];
    };
    let mut lines = vec![Line::from(vec![
        Span::styled(
            "status: ",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(job_status(state, &ui.job_id)),
    ])];
    lines.push(Line::from(format!(
        "target: {}",
        display_safe(&target_label(&ui.target))
    )));
    lines.push(Line::from(
        "evidence model: LiveScan | StorageLens | Inventory | OtherProvider | Unavailable",
    ));
    lines.push(Line::from(""));

    let Some(snapshot) = ui.snapshot() else {
        lines.push(Line::from("Collecting provider-proven facts…"));
        lines.push(Line::from(
            "Aggregate total is intentionally unknown while pagination is in progress.",
        ));
        return lines;
    };

    match snapshot.as_ref() {
        S3InspectorSnapshot::Object(object) => {
            lines.push(Line::from(format!(
                "evidence: {} @ {} unix-ms",
                object.evidence.label(),
                object.observed_at_unix_ms
            )));
            lines.push(Line::from(format!(
                "configured target id: {}",
                display_safe(&object.target)
            )));
            lines.push(Line::from(format!(
                "endpoint override: {}",
                object
                    .endpoint_override
                    .as_deref()
                    .map(display_safe)
                    .unwrap_or_else(|| "<none; provider default>".into())
            )));
            lines.push(Line::from(format!(
                "bucket: {}",
                display_safe(&object.bucket)
            )));
            lines.push(Line::from(format!(
                "exact key: {}",
                display_safe(&object.key)
            )));
            lines.push(Line::from(format!("size: {}", optional_bytes(object.size))));
            lines.push(Line::from(format!(
                "last modified unix-ms: {}",
                optional_u64(object.last_modified_unix_ms)
            )));
            lines.push(Line::from(format!(
                "ETag: {}",
                optional_text(object.etag.as_deref())
            )));
            lines.push(Line::from(format!(
                "content type: {}",
                optional_text(object.content_type.as_deref())
            )));
            lines.push(Line::from(format!(
                "storage class: {}",
                optional_text(object.storage_class.as_deref())
            )));
            lines.push(Line::from(format!(
                "version id: {}",
                optional_text(object.version_id.as_deref())
            )));
            lines.push(Line::from(""));
            lines.push(Line::from("metadata:"));
            if object.metadata.is_empty() {
                lines.push(Line::from("  <none reported>"));
            } else {
                for (key, value) in &object.metadata {
                    lines.push(Line::from(format!(
                        "  {} = {}",
                        display_safe(key),
                        display_safe(value)
                    )));
                }
            }
        }
        S3InspectorSnapshot::Scan(scan) => {
            let terminal = if scan.complete {
                "complete"
            } else if scan.cancelled {
                "cancelled / partial"
            } else {
                "partial"
            };
            lines.push(Line::from(format!(
                "evidence: {} @ {} unix-ms · {terminal}",
                scan.evidence.label(),
                scan.observed_at_unix_ms
            )));
            lines.push(Line::from(format!(
                "scope: {}",
                display_safe(&scope_label(&scan.scope))
            )));
            lines.push(Line::from(format!("pages observed: {}", scan.pages_seen)));
            lines.push(Line::from(format!(
                "objects observed: {}",
                scan.object_count
            )));
            let logical_suffix = if scan.objects_without_size == 0 {
                "complete for observed objects".to_string()
            } else {
                format!(
                    "incomplete: {} objects lacked size",
                    scan.objects_without_size
                )
            };
            lines.push(Line::from(format!(
                "logical bytes: {} · {logical_suffix}",
                format_bytes_u128(scan.total_logical_bytes)
            )));
            if let Some(note) = &scan.terminal_note {
                lines.push(Line::from(format!(
                    "terminal note: {}",
                    display_safe(note)
                )));
            }
            lines.push(Line::from(""));
            lines.push(Line::from("largest objects (bounded top 20):"));
            if scan.largest_objects.is_empty() {
                lines.push(Line::from("  <none with provider-reported size>"));
            } else {
                for object in &scan.largest_objects {
                    lines.push(Line::from(format!(
                        "  {:>10}  {}",
                        format_bytes_u128(u128::from(object.size)),
                        display_safe(&object.key)
                    )));
                }
            }
            lines.push(Line::from(""));
            lines.push(Line::from(format!(
                "largest immediate prefixes · evidence {}:",
                scan.largest_prefixes.source.label()
            )));
            match &scan.largest_prefixes.value {
                Some(prefixes) if prefixes.is_empty() => lines.push(Line::from("  <none>")),
                Some(prefixes) => {
                    for prefix in prefixes {
                        lines.push(Line::from(format!(
                            "  {:>10}  {:>7} objects  {}",
                            format_bytes_u128(prefix.logical_bytes),
                            prefix.object_count,
                            display_safe(&prefix.prefix)
                        )));
                    }
                }
                None => lines.push(Line::from(format!(
                    "  unavailable: {}",
                    scan.largest_prefixes
                        .note
                        .as_deref()
                        .map(display_safe)
                        .unwrap_or_else(|| "provider evidence unavailable".into())
                ))),
            }
            lines.push(Line::from(""));
            let age = &scan.age_distribution;
            lines.push(Line::from(format!(
                "age distribution: <30d {} · 30-89d {} · 90-364d {} · >=365d {} · unavailable {}",
                age.under_30_days,
                age.days_30_to_89,
                age.days_90_to_364,
                age.days_365_plus,
                age.unavailable
            )));
            lines.push(Line::from("storage classes:"));
            if scan.storage_classes.is_empty() {
                lines.push(Line::from("  <none reported>"));
            } else {
                for (class, count) in &scan.storage_classes {
                    lines.push(Line::from(format!(
                        "  {}: {count}",
                        display_safe(class)
                    )));
                }
            }
            if scan.objects_without_storage_class > 0 {
                lines.push(Line::from(format!(
                    "  unavailable: {} objects",
                    scan.objects_without_storage_class
                )));
            }
        }
    }
    lines
}

fn job_status(state: &AppState, id: &str) -> String {
    let Some(job) = state.jobs.iter().find(|job| job.id == id) else {
        return format!("UNKNOWN · job {id} unavailable");
    };
    match job.status {
        JobStatus::Pending => "PENDING".into(),
        JobStatus::Running => format!("RUNNING · {}", job.progress),
        JobStatus::Cancelling => format!("CANCELLING · {}", job.progress),
        JobStatus::Completed => job
            .result
            .as_ref()
            .and_then(JobResult::message)
            .map(|message| format!("COMPLETE · {message}"))
            .unwrap_or_else(|| "COMPLETE".into()),
        JobStatus::Cancelled => job
            .result
            .as_ref()
            .and_then(JobResult::message)
            .map(|message| format!("CANCELLED · {message}"))
            .unwrap_or_else(|| "CANCELLED".into()),
        JobStatus::Failed => format!(
            "FAILED · {}",
            job.error.as_deref().unwrap_or("inspection failed")
        ),
        JobStatus::PausePending | JobStatus::Paused | JobStatus::RetryWaiting => {
            format!("{:?}", job.status).to_uppercase()
        }
    }
}

fn target_label(target: &S3InspectionTarget) -> String {
    match target {
        S3InspectionTarget::Object(object) => {
            format!("s3://{}/{}", object.bucket, object.key)
        }
        S3InspectionTarget::Scope(scope) => scope_label(scope),
    }
}

fn scope_label(scope: &S3InspectionScope) -> String {
    match scope {
        S3InspectionScope::Bucket(bucket) => format!("s3://{}/", bucket.bucket),
        S3InspectionScope::Prefix(prefix) => format!("s3://{}/{}", prefix.bucket, prefix.prefix),
    }
}

fn optional_text(value: Option<&str>) -> String {
    value
        .map(display_safe)
        .unwrap_or_else(|| "<unavailable>".into())
}

fn optional_u64(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "<unavailable>".into())
}

fn optional_bytes(value: Option<u64>) -> String {
    value
        .map(|value| format_bytes_u128(u128::from(value)))
        .unwrap_or_else(|| "<unavailable>".into())
}

pub(crate) fn format_bytes_u128(bytes: u128) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn display_safe(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\x1b' => out.push_str("\\x1b"),
            ch if (ch as u32) < 0x20 || ch as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", ch as u32));
            }
            ch => out.push(ch),
        }
    }
    out
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
    use crate::vfs::s3::{S3BucketRef, S3ObjectRef, S3PrefixRef};

    #[test]
    fn exact_focused_identity_wins_over_presentation_name() {
        let location = Location::S3 {
            target: "t".into(),
            bucket: Some("b".into()),
            prefix: "folder".into(),
        };
        let listed = ListedEntry {
            entry: crate::vfs::Entry {
                name: "DISPLAY-ONLY".into(),
                kind: crate::vfs::EntryKind::File,
                size: Some(7),
                modified_unix_ms: None,
            },
            identity: EntryIdentity::S3Object(S3ObjectRef {
                target: "t".into(),
                bucket: "b".into(),
                key: "folder/exact-key".into(),
            }),
        };
        assert_eq!(
            target_from_context(&location, Some(&listed)).unwrap(),
            S3InspectionTarget::Object(S3ObjectRef {
                target: "t".into(),
                bucket: "b".into(),
                key: "folder/exact-key".into(),
            })
        );
    }

    #[test]
    fn account_root_requires_exact_bucket_focus() {
        let location = Location::S3 {
            target: "t".into(),
            bucket: None,
            prefix: String::new(),
        };
        assert!(target_from_context(&location, None).is_err());
        let listed = ListedEntry {
            entry: crate::vfs::Entry {
                name: "display".into(),
                kind: crate::vfs::EntryKind::Directory,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::S3Bucket(S3BucketRef {
                target: "t".into(),
                bucket: "Exact-Bucket".into(),
            }),
        };
        assert_eq!(
            target_from_context(&location, Some(&listed)).unwrap(),
            S3InspectionTarget::Scope(S3InspectionScope::Bucket(S3BucketRef {
                target: "t".into(),
                bucket: "Exact-Bucket".into(),
            }))
        );
    }

    #[test]
    fn prefix_identity_is_not_reconstructed_from_display_name() {
        let location = Location::S3 {
            target: "t".into(),
            bucket: Some("b".into()),
            prefix: "root".into(),
        };
        let listed = ListedEntry {
            entry: crate::vfs::Entry {
                name: "pretty".into(),
                kind: crate::vfs::EntryKind::Directory,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::S3Prefix(S3PrefixRef {
                target: "t".into(),
                bucket: "b".into(),
                prefix: "root//exact/".into(),
            }),
        };
        assert_eq!(
            target_from_context(&location, Some(&listed)).unwrap(),
            S3InspectionTarget::Scope(S3InspectionScope::Prefix(S3PrefixRef {
                target: "t".into(),
                bucket: "b".into(),
                prefix: "root//exact/".into(),
            }))
        );
    }

    #[test]
    fn display_escape_does_not_mutate_printable_unicode() {
        assert_eq!(display_safe("日本語/ok\n\x1b"), "日本語/ok\\n\\x1b");
    }
}
