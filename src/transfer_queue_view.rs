//! Pure presentation helpers for Transfer Queue status.
//!
//! The TUI can call these helpers with its existing `JobManager::snapshot()`.
//! No lifecycle state is owned here, and no aggregate percentage/ETA is
//! invented across unrelated jobs.

use crate::jobs::{Job, JobKind, JobProgress, JobStatus, Progress};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TransferJobCounts {
    pub running: usize,
    pub queued: usize,
    pub paused: usize,
    pub cancelling: usize,
}

impl TransferJobCounts {
    pub fn total_nonterminal(self) -> usize {
        self.running + self.queued + self.paused + self.cancelling
    }
}

pub fn transfer_job_counts(jobs: &[Job]) -> TransferJobCounts {
    let mut counts = TransferJobCounts::default();
    for job in jobs.iter().filter(|job| job.kind == JobKind::Transfer) {
        match job.status {
            JobStatus::Pending => counts.queued += 1,
            JobStatus::Running | JobStatus::PausePending => counts.running += 1,
            JobStatus::Paused => counts.paused += 1,
            JobStatus::Cancelling => counts.cancelling += 1,
            JobStatus::RetryWaiting => counts.queued += 1,
            JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled => {}
        }
    }
    counts
}

/// Render one compact truthful status-bar segment for currently nonterminal
/// transfer jobs.
///
/// Percentage, rate and ETA are shown only for the oldest visible *running*
/// transfer whose JobProgress actually carries those facts. They are never
/// presented as an aggregate across multiple transfers.
pub fn transfer_status_bar(jobs: &[Job]) -> Option<String> {
    let counts = transfer_job_counts(jobs);
    if counts.total_nonterminal() == 0 {
        return None;
    }

    let mut parts = Vec::new();
    if counts.running > 0 {
        parts.push(format!("{} running", counts.running));
    }
    if counts.queued > 0 {
        parts.push(format!("{} queued", counts.queued));
    }
    if counts.paused > 0 {
        parts.push(format!("{} paused", counts.paused));
    }
    if counts.cancelling > 0 {
        parts.push(format!("{} cancelling", counts.cancelling));
    }

    let mut text = format!("[{}]", parts.join(" · "));

    let Some(primary) = jobs
        .iter()
        .find(|job| job.kind == JobKind::Transfer && job.status == JobStatus::Running)
    else {
        return Some(text);
    };

    let mut detail = Vec::new();
    detail.push(primary.id.clone());

    match &primary.progress {
        JobProgress::Generic(Progress::Bytes { done, total, rate }) => {
            match total {
                Some(0) => detail.push("0%".into()),
                Some(total) => {
                    let percent = (((*done as u128) * 100) / (*total as u128)).min(100);
                    detail.push(format!("{percent}%"));
                }
                None => {
                    detail.push(format_bytes(*done));
                    detail.push("unknown total".into());
                }
            }
            if *rate > 0 {
                detail.push(format!("{}/s", format_bytes(*rate)));
                if let Some(eta) = primary.eta() {
                    detail.push(format!("ETA {eta}"));
                }
            }
        }
        JobProgress::Generic(Progress::Items { done, total }) => {
            detail.push(format!("{done}/{total} items"));
        }
        JobProgress::Generic(Progress::Percent(percent)) => {
            detail.push(format!("{percent}%"));
        }
        JobProgress::Generic(Progress::Phase { phase, percent }) => {
            detail.push(match percent {
                Some(percent) => format!("{phase} {percent}%"),
                None => phase.clone(),
            });
        }
        JobProgress::Generic(Progress::Indeterminate)
        | JobProgress::WorkspaceSync(_)
        | JobProgress::RemoteEdit(_) => {}
        #[cfg(target_os = "linux")]
        JobProgress::StorageScan(_) => {}
    }

    if !detail.is_empty() {
        text.push(' ');
        text.push_str(&detail.join(" · "));
    }
    Some(text)
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::jobs::{JobEvent, JobManager};
    use crate::vfs::Location;
    use tokio::sync::mpsc;

    fn transfer_job(manager: &JobManager) -> Job {
        manager.create_job(
            "transfer",
            JobKind::Transfer,
            "copy",
            Some(Location::Local(PathBuf::from("/src"))),
            Some(Location::Local(PathBuf::from("/dst"))),
        )
    }

    #[test]
    fn no_nonterminal_transfers_means_no_status_segment() {
        assert_eq!(transfer_status_bar(&[]), None);
    }

    #[test]
    fn queued_counts_are_truthful_without_fake_progress() {
        let manager = JobManager::new();
        transfer_job(&manager);
        transfer_job(&manager);

        let snapshot = manager.snapshot();
        assert_eq!(
            transfer_job_counts(&snapshot),
            TransferJobCounts {
                running: 0,
                queued: 2,
                paused: 0,
                cancelling: 0,
            }
        );
        assert_eq!(
            transfer_status_bar(&snapshot).as_deref(),
            Some("[2 queued]")
        );
    }

    #[test]
    fn byte_progress_shows_primary_job_speed_and_eta_not_aggregate_eta() {
        let manager = JobManager::new();
        let (tx, _rx) = mpsc::unbounded_channel();
        let first = transfer_job(&manager);
        let second = transfer_job(&manager);

        assert!(manager.publish_event(
            &tx,
            JobEvent::Running {
                id: first.id.clone(),
            },
        ));
        assert!(manager.publish_event(
            &tx,
            JobEvent::Progress {
                id: first.id.clone(),
                progress: JobProgress::Generic(Progress::Bytes {
                    done: 512,
                    total: Some(1024),
                    rate: 256,
                }),
            },
        ));
        assert!(manager.publish_event(
            &tx,
            JobEvent::Running {
                id: second.id.clone(),
            },
        ));

        let text = transfer_status_bar(&manager.snapshot()).unwrap();
        assert!(text.starts_with("[2 running]"));
        assert!(text.contains(&first.id));
        assert!(text.contains("50%"));
        assert!(text.contains("256 B/s"));
        assert!(text.contains("ETA 2s"));
    }

    #[test]
    fn unknown_and_zero_byte_totals_render_distinct_truth() {
        for (total, expected) in [(None, "unknown total"), (Some(0), "0%")] {
            let manager = JobManager::new();
            let (tx, _rx) = mpsc::unbounded_channel();
            let job = transfer_job(&manager);
            assert!(manager.publish_event(&tx, JobEvent::Running { id: job.id.clone() }));
            assert!(manager.publish_event(
                &tx,
                JobEvent::Progress {
                    id: job.id,
                    progress: JobProgress::Generic(Progress::Bytes {
                        done: 0,
                        total,
                        rate: 0,
                    }),
                },
            ));
            let text = transfer_status_bar(&manager.snapshot()).unwrap();
            assert!(text.contains(expected), "{text}");
        }
    }

    #[test]
    fn item_progress_never_becomes_fake_byte_speed_or_eta() {
        let manager = JobManager::new();
        let (tx, _rx) = mpsc::unbounded_channel();
        let job = transfer_job(&manager);

        assert!(manager.publish_event(&tx, JobEvent::Running { id: job.id.clone() },));
        assert!(manager.publish_event(
            &tx,
            JobEvent::Progress {
                id: job.id.clone(),
                progress: JobProgress::Generic(Progress::Items { done: 1, total: 3 }),
            },
        ));

        let text = transfer_status_bar(&manager.snapshot()).unwrap();
        assert!(text.contains("1/3 items"));
        assert!(!text.contains("/s"));
        assert!(!text.contains("ETA"));
    }
}
