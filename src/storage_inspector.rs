//! Read-only local storage-usage scanner for Linux.
//!
//! Traversal is delegated to the MIT-licensed `dua-core` crate instead of
//! maintaining a second recursive directory walker inside ARX. ARX owns the
//! accounting, cancellation, result truth and later JobManager/TUI integration.

use std::collections::HashSet;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::thread;

use dua_core::{Options as WalkMetadataOptions, Order, walk};

const PROGRESS_EVERY_ENTRIES: u64 = 32;
const LINUX_STAT_BLOCK_BYTES: u64 = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageScanOutcome {
    Complete,
    Partial,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageScanOptions {
    /// Do not descend below this depth. Root is depth 0.
    pub max_depth: Option<usize>,
    /// Stay on the root filesystem (`st_dev`), like `du -x` / `gdu --no-cross`.
    pub same_filesystem: bool,
    /// Number of largest unique regular files retained in `top_files`.
    pub top_n: usize,
    /// Worker threads used by `dua-core`; zero means auto-detect.
    pub threads: usize,
}

impl Default for UsageScanOptions {
    fn default() -> Self {
        Self {
            max_depth: None,
            same_filesystem: false,
            top_n: 20,
            threads: 0,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageTotals {
    /// Apparent/logical bytes (`st_size`) for counted entries.
    pub logical_bytes: u128,
    /// Allocated bytes derived from Linux `st_blocks * 512`.
    pub allocated_bytes: u128,
    pub files: u64,
    pub directories: u64,
    pub symlinks: u64,
    pub other: u64,
    /// Additional directory entries pointing at already-counted regular-file data.
    pub hardlink_duplicates: u64,
    /// Iterator/read/stat failures observed while scanning descendants.
    pub errors: u64,
    pub entries_seen: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageRecord {
    pub path: PathBuf,
    pub depth: usize,
    pub kind: UsageKind,
    /// Zero for an already-counted hard-link duplicate.
    pub logical_bytes: u64,
    /// Zero for an already-counted hard-link duplicate.
    pub allocated_bytes: u64,
    pub hardlink_duplicate: bool,
    /// Metadata failed for this entry, so byte values are not authoritative.
    pub metadata_error: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageScanProgress {
    pub entries_seen: u64,
    pub logical_bytes: u128,
    pub allocated_bytes: u128,
    pub errors: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageScanResult {
    pub root: PathBuf,
    pub outcome: UsageScanOutcome,
    pub totals: UsageTotals,
    /// Parent-first records suitable for a later interactive drill-down tree.
    pub records: Vec<UsageRecord>,
    /// Largest unique regular files, ordered by allocated bytes descending,
    /// then apparent bytes descending, then path ascending.
    pub top_files: Vec<UsageRecord>,
}

/// Scan one local Linux path.
///
/// The callback receives monotonic observed work only. There is deliberately no
/// percent or ETA: the total number/size of descendants is unknown until the
/// traversal completes.
///
/// The supplied cancellation flag is directly compatible with JobManager's
/// per-job cancellation token. Once cancellation is observed, enumeration stops
/// and the `dua-core` iterator is dropped; dropping it stops and joins its worker
/// pool.
pub fn scan_local_with_progress(
    root: &Path,
    options: &UsageScanOptions,
    cancel: Arc<AtomicBool>,
    mut on_progress: impl FnMut(&UsageScanProgress),
) -> io::Result<UsageScanResult> {
    let root_metadata = std::fs::symlink_metadata(root)?;
    let root_device = root_metadata.dev();
    let threads = resolved_threads(options.threads);
    let same_filesystem = options.same_filesystem;
    let max_depth = options.max_depth;
    let descend_cancel = Arc::clone(&cancel);

    let walker = walk(
        root,
        threads,
        Order::ParentFirst,
        WalkMetadataOptions::default(),
        move |entry| {
            if descend_cancel.load(AtomicOrdering::Relaxed) {
                return false;
            }
            if max_depth.is_some_and(|limit| entry.depth >= limit) {
                return false;
            }
            if !same_filesystem {
                return true;
            }
            // Fail closed: when filesystem identity is unavailable, do not
            // schedule descendants across an unproven boundary.
            entry
                .metadata
                .as_ref()
                .map(|metadata| metadata.dev() == root_device)
                .unwrap_or(false)
        },
    );

    let mut totals = UsageTotals::default();
    let mut records = Vec::new();
    let mut seen_hardlinks = HashSet::<(u64, u64)>::new();
    let mut cancelled = false;

    for entry in walker {
        if cancel.load(AtomicOrdering::Relaxed) {
            cancelled = true;
            break;
        }

        totals.entries_seen = totals.entries_seen.saturating_add(1);

        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                totals.errors = totals.errors.saturating_add(1);
                maybe_report_progress(&totals, &mut on_progress);
                continue;
            }
        };

        let path = entry.path();
        let kind = if entry.file_type.is_file() {
            UsageKind::File
        } else if entry.file_type.is_dir() {
            UsageKind::Directory
        } else if entry.file_type.is_symlink() {
            UsageKind::Symlink
        } else {
            UsageKind::Other
        };

        match kind {
            UsageKind::File => totals.files = totals.files.saturating_add(1),
            UsageKind::Directory => {
                totals.directories = totals.directories.saturating_add(1)
            }
            UsageKind::Symlink => totals.symlinks = totals.symlinks.saturating_add(1),
            UsageKind::Other => totals.other = totals.other.saturating_add(1),
        }

        let metadata = match entry.metadata.as_ref() {
            Ok(metadata) => metadata,
            Err(_) => {
                totals.errors = totals.errors.saturating_add(1);
                records.push(UsageRecord {
                    path,
                    depth: entry.depth,
                    kind,
                    logical_bytes: 0,
                    allocated_bytes: 0,
                    hardlink_duplicate: false,
                    metadata_error: true,
                });
                maybe_report_progress(&totals, &mut on_progress);
                continue;
            }
        };

        let hardlink_duplicate = if kind == UsageKind::File && metadata.nlink() > 1 {
            !seen_hardlinks.insert((metadata.dev(), metadata.ino()))
        } else {
            false
        };

        let (logical_bytes, allocated_bytes) = if hardlink_duplicate {
            totals.hardlink_duplicates = totals.hardlink_duplicates.saturating_add(1);
            (0, 0)
        } else {
            (
                metadata.len(),
                metadata.blocks().saturating_mul(LINUX_STAT_BLOCK_BYTES),
            )
        };

        totals.logical_bytes = totals
            .logical_bytes
            .saturating_add(u128::from(logical_bytes));
        totals.allocated_bytes = totals
            .allocated_bytes
            .saturating_add(u128::from(allocated_bytes));

        records.push(UsageRecord {
            path,
            depth: entry.depth,
            kind,
            logical_bytes,
            allocated_bytes,
            hardlink_duplicate,
            metadata_error: false,
        });

        maybe_report_progress(&totals, &mut on_progress);
    }

    let outcome = if cancelled || cancel.load(AtomicOrdering::Relaxed) {
        UsageScanOutcome::Cancelled
    } else if totals.errors > 0 {
        UsageScanOutcome::Partial
    } else {
        UsageScanOutcome::Complete
    };

    let mut top_files = records
        .iter()
        .filter(|record| {
            record.kind == UsageKind::File
                && !record.hardlink_duplicate
                && !record.metadata_error
        })
        .cloned()
        .collect::<Vec<_>>();
    top_files.sort_by(|left, right| {
        right
            .allocated_bytes
            .cmp(&left.allocated_bytes)
            .then_with(|| right.logical_bytes.cmp(&left.logical_bytes))
            .then_with(|| left.path.cmp(&right.path))
    });
    top_files.truncate(options.top_n);

    on_progress(&UsageScanProgress::from(&totals));

    Ok(UsageScanResult {
        root: root.to_path_buf(),
        outcome,
        totals,
        records,
        top_files,
    })
}

/// Convenience wrapper for callers that do not need streaming progress.
pub fn scan_local(
    root: &Path,
    options: &UsageScanOptions,
    cancel: Arc<AtomicBool>,
) -> io::Result<UsageScanResult> {
    scan_local_with_progress(root, options, cancel, |_| {})
}

fn resolved_threads(requested: usize) -> usize {
    if requested > 0 {
        return requested;
    }
    thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1)
}

fn maybe_report_progress(
    totals: &UsageTotals,
    on_progress: &mut impl FnMut(&UsageScanProgress),
) {
    if totals.entries_seen % PROGRESS_EVERY_ENTRIES == 0 {
        on_progress(&UsageScanProgress::from(totals));
    }
}

impl From<&UsageTotals> for UsageScanProgress {
    fn from(totals: &UsageTotals) -> Self {
        Self {
            entries_seen: totals.entries_seen,
            logical_bytes: totals.logical_bytes,
            allocated_bytes: totals.allocated_bytes,
            errors: totals.errors,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, OpenOptions};
    use std::io::Write;

    fn no_cancel() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    #[test]
    fn regular_file_reports_logical_and_allocated_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("alpha.bin");
        fs::write(&path, vec![0x5a; 8192]).unwrap();

        let result = scan_local(dir.path(), &UsageScanOptions::default(), no_cancel()).unwrap();
        let record = result
            .records
            .iter()
            .find(|record| record.path == path)
            .unwrap();

        assert_eq!(record.kind, UsageKind::File);
        assert_eq!(record.logical_bytes, 8192);
        assert!(record.allocated_bytes > 0);
        assert_eq!(result.outcome, UsageScanOutcome::Complete);
    }

    #[test]
    fn sparse_file_keeps_apparent_and_allocated_truth_separate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sparse.bin");
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        file.set_len(8 * 1024 * 1024).unwrap();

        let result = scan_local(dir.path(), &UsageScanOptions::default(), no_cancel()).unwrap();
        let record = result
            .records
            .iter()
            .find(|record| record.path == path)
            .unwrap();

        assert_eq!(record.logical_bytes, 8 * 1024 * 1024);
        assert!(record.allocated_bytes <= record.logical_bytes);
    }

    #[test]
    fn hardlink_data_is_counted_once() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.bin");
        let second = dir.path().join("second.bin");
        fs::write(&first, vec![0x11; 4096]).unwrap();
        fs::hard_link(&first, &second).unwrap();

        let result = scan_local(dir.path(), &UsageScanOptions::default(), no_cancel()).unwrap();
        let pair = result
            .records
            .iter()
            .filter(|record| record.path == first || record.path == second)
            .collect::<Vec<_>>();

        assert_eq!(pair.len(), 2);
        assert_eq!(pair.iter().filter(|record| record.hardlink_duplicate).count(), 1);
        assert_eq!(result.totals.hardlink_duplicates, 1);
        assert_eq!(
            pair.iter().map(|record| record.logical_bytes).sum::<u64>(),
            4096
        );
    }

    #[test]
    fn symlink_to_directory_is_never_followed() {
        let root = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        fs::write(target.path().join("secret.txt"), b"secret").unwrap();
        std::os::unix::fs::symlink(target.path(), root.path().join("linked-dir")).unwrap();

        let result = scan_local(root.path(), &UsageScanOptions::default(), no_cancel()).unwrap();

        assert!(result.records.iter().any(|record| {
            record.path == root.path().join("linked-dir") && record.kind == UsageKind::Symlink
        }));
        assert!(!result
            .records
            .iter()
            .any(|record| record.path.ends_with("secret.txt")));
    }

    #[test]
    fn max_depth_prunes_descendants_but_keeps_boundary_directory() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("level1")).unwrap();
        fs::write(root.path().join("level1/hidden.txt"), b"x").unwrap();

        let options = UsageScanOptions {
            max_depth: Some(1),
            ..UsageScanOptions::default()
        };
        let result = scan_local(root.path(), &options, no_cancel()).unwrap();

        assert!(result
            .records
            .iter()
            .any(|record| record.path == root.path().join("level1")));
        assert!(!result
            .records
            .iter()
            .any(|record| record.path.ends_with("hidden.txt")));
    }

    #[test]
    fn top_files_are_deterministic_for_equal_sizes() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("b.bin"), vec![1_u8; 4096]).unwrap();
        fs::write(root.path().join("a.bin"), vec![2_u8; 4096]).unwrap();

        let options = UsageScanOptions {
            top_n: 2,
            ..UsageScanOptions::default()
        };
        let result = scan_local(root.path(), &options, no_cancel()).unwrap();

        assert_eq!(result.top_files.len(), 2);
        assert!(result.top_files[0].path < result.top_files[1].path);
    }

    #[test]
    fn cancellation_returns_partial_cancelled_truth() {
        let root = tempfile::tempdir().unwrap();
        for index in 0..512 {
            let mut file = fs::File::create(root.path().join(format!("file-{index:04}.bin"))).unwrap();
            file.write_all(b"x").unwrap();
        }

        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_from_progress = Arc::clone(&cancel);
        let result = scan_local_with_progress(
            root.path(),
            &UsageScanOptions::default(),
            cancel,
            move |progress| {
                if progress.entries_seen >= PROGRESS_EVERY_ENTRIES {
                    cancel_from_progress.store(true, AtomicOrdering::Relaxed);
                }
            },
        )
        .unwrap();

        assert_eq!(result.outcome, UsageScanOutcome::Cancelled);
        assert!(result.totals.entries_seen < 513);
    }

    #[test]
    fn zero_top_n_retains_no_top_files() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("file.bin"), b"data").unwrap();
        let options = UsageScanOptions {
            top_n: 0,
            ..UsageScanOptions::default()
        };

        let result = scan_local(root.path(), &options, no_cancel()).unwrap();
        assert!(result.top_files.is_empty());
    }
}
