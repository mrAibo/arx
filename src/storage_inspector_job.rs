//! Typed JobManager-facing model for Local Storage Inspector scans.
//!
//! The full drill-down record vector lives in `StorageScanSnapshotStore`.
//! These types are intentionally compact so `Job` snapshots remain cheap and
//! can expose truthful progress/terminal state without string encoding.

use std::path::PathBuf;

use crate::storage_inspector::{
    UsageScanOutcome, UsageScanProgress, UsageScanResult, UsageTotals,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StorageScanProgress {
    pub entries_seen: u64,
    pub logical_bytes: u128,
    pub allocated_bytes: u128,
    pub errors: u64,
}

impl StorageScanProgress {
    /// Storage traversal has no truthful denominator until traversal completes.
    /// Keep this API explicit so callers cannot accidentally fabricate a percent.
    pub const fn percent(&self) -> Option<u8> {
        None
    }
}

impl From<&UsageScanProgress> for StorageScanProgress {
    fn from(progress: &UsageScanProgress) -> Self {
        Self {
            entries_seen: progress.entries_seen,
            logical_bytes: progress.logical_bytes,
            allocated_bytes: progress.allocated_bytes,
            errors: progress.errors,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageScanSummary {
    pub root: PathBuf,
    pub outcome: UsageScanOutcome,
    pub totals: UsageTotals,
}

impl StorageScanSummary {
    pub fn is_complete(&self) -> bool {
        self.outcome == UsageScanOutcome::Complete
    }

    pub fn is_partial(&self) -> bool {
        self.outcome == UsageScanOutcome::Partial
    }

    pub fn is_cancelled(&self) -> bool {
        self.outcome == UsageScanOutcome::Cancelled
    }
}

impl From<&UsageScanResult> for StorageScanSummary {
    fn from(result: &UsageScanResult) -> Self {
        Self {
            root: result.root.clone(),
            outcome: result.outcome,
            totals: result.totals.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_never_invents_a_percent() {
        let progress = StorageScanProgress {
            entries_seen: 1_000,
            logical_bytes: 10_000,
            allocated_bytes: 8_000,
            errors: 3,
        };

        assert_eq!(progress.percent(), None);
    }

    #[test]
    fn progress_conversion_preserves_observed_truth() {
        let source = UsageScanProgress {
            entries_seen: 42,
            logical_bytes: 1_024,
            allocated_bytes: 512,
            errors: 2,
        };

        let progress = StorageScanProgress::from(&source);
        assert_eq!(progress.entries_seen, 42);
        assert_eq!(progress.logical_bytes, 1_024);
        assert_eq!(progress.allocated_bytes, 512);
        assert_eq!(progress.errors, 2);
    }

    #[test]
    fn summary_keeps_terminal_outcome_and_totals_without_records() {
        let result = UsageScanResult {
            root: PathBuf::from("/tmp/scan"),
            outcome: UsageScanOutcome::Partial,
            totals: UsageTotals {
                entries_seen: 7,
                errors: 1,
                ..UsageTotals::default()
            },
            records: Vec::new(),
            top_files: Vec::new(),
        };

        let summary = StorageScanSummary::from(&result);
        assert!(summary.is_partial());
        assert!(!summary.is_complete());
        assert!(!summary.is_cancelled());
        assert_eq!(summary.root, PathBuf::from("/tmp/scan"));
        assert_eq!(summary.totals.entries_seen, 7);
        assert_eq!(summary.totals.errors, 1);
    }
}
