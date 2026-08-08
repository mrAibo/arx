use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use crate::services::{
    WorkspaceScanError, WorkspaceScanId, WorkspaceScanOptions, WorkspaceScanResponse,
    WorkspaceScanner,
};
use crate::vfs::{Location, ProviderRegistry};
use crate::workspace_sync::{
    DiffState, WorkspaceDiff, WorkspaceDiffEntry, WorkspaceEntry, WorkspaceSide,
};
use crate::workspace_sync_execution::SyncPlanId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SyncVerificationId(pub u64);

impl SyncVerificationId {
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncVerificationVerdict {
    Synchronized,
    DifferencesRemain {
        changed: usize,
        conflicts: usize,
        unverified: usize,
    },
    Inconclusive {
        unverified: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncVerificationResult {
    pub plan_id: SyncPlanId,
    pub left_root: Location,
    pub right_root: Location,
    pub diff: WorkspaceDiff,
    pub changed_entries: usize,
    pub conflicts: usize,
    pub unverified_entries: usize,
    pub verdict: SyncVerificationVerdict,
}

impl SyncVerificationResult {
    pub fn from_diff(plan_id: SyncPlanId, diff: WorkspaceDiff) -> Self {
        let mut changed_entries = 0usize;
        let mut conflicts = 0usize;
        let mut unverified_entries = 0usize;

        for entry in &diff.entries {
            match verification_evidence(entry) {
                VerificationEvidence::Equal => {}
                VerificationEvidence::Different { conflict } => {
                    changed_entries += 1;
                    conflicts += usize::from(conflict);
                }
                VerificationEvidence::Unverified => unverified_entries += 1,
            }
        }

        let verdict = if changed_entries > 0 {
            SyncVerificationVerdict::DifferencesRemain {
                changed: changed_entries,
                conflicts,
                unverified: unverified_entries,
            }
        } else if unverified_entries > 0 {
            SyncVerificationVerdict::Inconclusive {
                unverified: unverified_entries,
            }
        } else {
            SyncVerificationVerdict::Synchronized
        };

        Self {
            plan_id,
            left_root: diff.left_root.clone(),
            right_root: diff.right_root.clone(),
            diff,
            changed_entries,
            conflicts,
            unverified_entries,
            verdict,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncVerificationStatus {
    Pending,
    Running {
        left_scan: WorkspaceScanId,
        right_scan: WorkspaceScanId,
    },
    Finished(Box<SyncVerificationResult>),
    Failed {
        side: Option<WorkspaceSide>,
        error: String,
    },
    Cancelled,
    Superseded,
}

impl SyncVerificationStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Finished(_) | Self::Failed { .. } | Self::Cancelled | Self::Superseded
        )
    }

    pub fn can_transition_to(&self, next: &Self) -> bool {
        match self {
            Self::Pending => matches!(
                next,
                Self::Running { .. } | Self::Failed { .. } | Self::Cancelled | Self::Superseded
            ),
            Self::Running { .. } => matches!(
                next,
                Self::Finished(_) | Self::Failed { .. } | Self::Cancelled | Self::Superseded
            ),
            Self::Finished(_) | Self::Failed { .. } | Self::Cancelled | Self::Superseded => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncVerificationSnapshot {
    pub id: SyncVerificationId,
    pub plan_id: SyncPlanId,
    pub left_root: Location,
    pub right_root: Location,
    pub status: SyncVerificationStatus,
}

impl SyncVerificationSnapshot {
    fn with_status(&self, status: SyncVerificationStatus) -> Self {
        Self {
            id: self.id,
            plan_id: self.plan_id,
            left_root: self.left_root.clone(),
            right_root: self.right_root.clone(),
            status,
        }
    }

    pub fn accepts_scan(&self, response: &WorkspaceScanResponse) -> bool {
        let SyncVerificationStatus::Running {
            left_scan,
            right_scan,
        } = self.status
        else {
            return false;
        };

        match response.side {
            WorkspaceSide::Left => response.id == left_scan && response.root == self.left_root,
            WorkspaceSide::Right => response.id == right_scan && response.root == self.right_root,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncVerificationEvent {
    pub job_id: String,
    pub verification: SyncVerificationSnapshot,
}

pub struct SyncVerificationRun {
    id: SyncVerificationId,
    updates: mpsc::UnboundedReceiver<SyncVerificationSnapshot>,
}

impl SyncVerificationRun {
    pub const fn id(&self) -> SyncVerificationId {
        self.id
    }

    pub async fn recv(&mut self) -> Option<SyncVerificationSnapshot> {
        self.updates.recv().await
    }
}

#[derive(Clone)]
struct ActiveVerification {
    id: SyncVerificationId,
    plan_id: SyncPlanId,
    left_root: Location,
    right_root: Location,
    cancel: Arc<AtomicBool>,
    updates: mpsc::UnboundedSender<SyncVerificationSnapshot>,
}

impl ActiveVerification {
    fn snapshot(&self, status: SyncVerificationStatus) -> SyncVerificationSnapshot {
        SyncVerificationSnapshot {
            id: self.id,
            plan_id: self.plan_id,
            left_root: self.left_root.clone(),
            right_root: self.right_root.clone(),
            status,
        }
    }
}

#[derive(Clone)]
pub struct SyncVerificationCoordinator {
    registry: ProviderRegistry,
    options: WorkspaceScanOptions,
    next_id: Arc<AtomicU64>,
    active: Arc<Mutex<BTreeMap<String, ActiveVerification>>>,
}

impl SyncVerificationCoordinator {
    pub fn new(registry: ProviderRegistry) -> Self {
        Self::with_options(registry, WorkspaceScanOptions::default())
    }

    pub fn with_options(registry: ProviderRegistry, options: WorkspaceScanOptions) -> Self {
        Self {
            registry,
            options,
            next_id: Arc::new(AtomicU64::new(1)),
            active: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub fn start(
        &self,
        job_id: String,
        plan_id: SyncPlanId,
        left_root: Location,
        right_root: Location,
    ) -> SyncVerificationRun {
        let id = SyncVerificationId(self.next_id.fetch_add(1, Ordering::Relaxed));
        let cancel = Arc::new(AtomicBool::new(false));
        let (updates, rx) = mpsc::unbounded_channel();
        let active = ActiveVerification {
            id,
            plan_id,
            left_root: left_root.clone(),
            right_root: right_root.clone(),
            cancel: cancel.clone(),
            updates: updates.clone(),
        };

        {
            let mut state = self
                .active
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(previous) = state.remove(&job_id) {
                previous.cancel.store(true, Ordering::Relaxed);
                let _ = previous
                    .updates
                    .send(previous.snapshot(SyncVerificationStatus::Superseded));
            }
            state.insert(job_id.clone(), active.clone());
        }

        let pending = active.snapshot(SyncVerificationStatus::Pending);
        let _ = updates.send(pending.clone());

        let (scanner, mut scan_rx) = WorkspaceScanner::channel(self.registry.clone());
        let left_scan = scanner.scan(WorkspaceSide::Left, left_root, self.options, cancel.clone());
        let right_scan = scanner.scan(WorkspaceSide::Right, right_root, self.options, cancel);
        let running = pending.with_status(SyncVerificationStatus::Running {
            left_scan,
            right_scan,
        });

        if self.is_current(&job_id, id) {
            let _ = updates.send(running.clone());
            let active_state = self.active.clone();
            tokio::spawn(async move {
                collect_scan_results(active_state, job_id, running, &mut scan_rx).await;
            });
        }

        SyncVerificationRun { id, updates: rx }
    }

    pub fn cancel(&self, job_id: &str) -> bool {
        self.finish_active(job_id, SyncVerificationStatus::Cancelled)
    }

    pub fn supersede(&self, job_id: &str) -> bool {
        self.finish_active(job_id, SyncVerificationStatus::Superseded)
    }

    fn is_current(&self, job_id: &str, id: SyncVerificationId) -> bool {
        self.active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(job_id)
            .is_some_and(|active| active.id == id)
    }

    fn finish_active(&self, job_id: &str, status: SyncVerificationStatus) -> bool {
        let active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(job_id);
        let Some(active) = active else {
            return false;
        };
        active.cancel.store(true, Ordering::Relaxed);
        let _ = active.updates.send(active.snapshot(status));
        true
    }
}

async fn collect_scan_results(
    active_state: Arc<Mutex<BTreeMap<String, ActiveVerification>>>,
    job_id: String,
    running: SyncVerificationSnapshot,
    scan_rx: &mut mpsc::UnboundedReceiver<WorkspaceScanResponse>,
) {
    let mut left_entries: Option<Vec<WorkspaceEntry>> = None;
    let mut right_entries: Option<Vec<WorkspaceEntry>> = None;

    while let Some(response) = scan_rx.recv().await {
        if !is_current(&active_state, &job_id, running.id) {
            return;
        }
        if !running.accepts_scan(&response) {
            continue;
        }

        let side = response.side;
        match response.result {
            Ok(entries) => match side {
                WorkspaceSide::Left => left_entries = Some(entries),
                WorkspaceSide::Right => right_entries = Some(entries),
            },
            Err(WorkspaceScanError::Cancelled) => {
                finish_current(
                    &active_state,
                    &job_id,
                    running.id,
                    SyncVerificationStatus::Cancelled,
                    true,
                );
                return;
            }
            Err(error) => {
                finish_current(
                    &active_state,
                    &job_id,
                    running.id,
                    SyncVerificationStatus::Failed {
                        side: Some(side),
                        error: error.to_string(),
                    },
                    true,
                );
                return;
            }
        }

        let (Some(left), Some(right)) = (&left_entries, &right_entries) else {
            continue;
        };
        let diff = WorkspaceDiff::compare(
            running.left_root.clone(),
            running.right_root.clone(),
            left.clone(),
            right.clone(),
        );
        let result = SyncVerificationResult::from_diff(running.plan_id, diff);
        finish_current(
            &active_state,
            &job_id,
            running.id,
            SyncVerificationStatus::Finished(Box::new(result)),
            false,
        );
        return;
    }
}

fn is_current(
    active_state: &Mutex<BTreeMap<String, ActiveVerification>>,
    job_id: &str,
    id: SyncVerificationId,
) -> bool {
    active_state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(job_id)
        .is_some_and(|active| active.id == id)
}

fn finish_current(
    active_state: &Mutex<BTreeMap<String, ActiveVerification>>,
    job_id: &str,
    id: SyncVerificationId,
    status: SyncVerificationStatus,
    cancel: bool,
) {
    let active = {
        let mut state = active_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.get(job_id).is_none_or(|active| active.id != id) {
            return;
        }
        state.remove(job_id)
    };

    if let Some(active) = active {
        if cancel {
            active.cancel.store(true, Ordering::Relaxed);
        }
        let _ = active.updates.send(active.snapshot(status));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerificationEvidence {
    Equal,
    Different { conflict: bool },
    Unverified,
}

fn verification_evidence(entry: &WorkspaceDiffEntry) -> VerificationEvidence {
    match entry.state {
        DiffState::SameFingerprint => VerificationEvidence::Equal,
        DiffState::OnlyLeft
        | DiffState::OnlyRight
        | DiffState::LeftNewer
        | DiffState::RightNewer => VerificationEvidence::Different { conflict: false },
        DiffState::Different => {
            let (Some(left), Some(right)) = (&entry.left, &entry.right) else {
                return VerificationEvidence::Different { conflict: false };
            };

            let proven_different = left.kind != right.kind
                || matches!(
                    (&left.content_hash, &right.content_hash),
                    (Some(left_hash), Some(right_hash)) if left_hash != right_hash
                )
                || matches!((left.size, right.size), (Some(left_size), Some(right_size)) if left_size != right_size)
                || matches!(
                    (left.modified_unix_ms, right.modified_unix_ms),
                    (Some(left_time), Some(right_time)) if left_time != right_time
                );

            if proven_different {
                VerificationEvidence::Different { conflict: true }
            } else {
                VerificationEvidence::Unverified
            }
        }
    }
}
