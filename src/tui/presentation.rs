use arx::app::{Action, AppState, OverlayKind, SessionCallout, WorkspaceSyncUxState};
use arx::input::{KeyRouter, contextual_hints};
use arx::vfs::{Location, ProviderId};
use arx::workspace_sync::{WorkspaceDiff, WorkspaceSyncPlan};
use arx::workspace_sync_verification::{
    SyncVerificationStatus,
    SyncVerificationVerdict::{
        DifferencesRemain as VerifyDifferencesRemain, Inconclusive as VerifyInconclusive,
        Synchronized as VerifySynchronized,
    },
};

use super::format_size;

pub(super) fn session_callout_text(state: &AppState, key_router: &KeyRouter) -> Option<String> {
    let callout = state.session_callout.as_ref()?;
    if session_callout_is_embedded(state, callout) {
        return None;
    }

    match callout {
        SessionCallout::CompareCompleted {
            differences,
            bytes_to_transfer,
        } => {
            if *differences == 0 {
                return Some("✓ Workspace compared · No proven differences found.".into());
            }

            let changes = if *differences == 1 {
                "1 change found".to_string()
            } else {
                format!("{differences} changes found")
            };
            let transfer = if *bytes_to_transfer > 0 {
                format!(" · {} planned", format_size(*bytes_to_transfer))
            } else {
                String::new()
            };
            let next = if state.remote_workspace.preview_open {
                String::new()
            } else {
                contextual_hints(state, key_router.keymap())
                    .into_iter()
                    .find(|hint| hint.action == Action::PreviewWorkspaceSync.id())
                    .map(|hint| format!(" · {} {}", hint.binding, hint.label))
                    .unwrap_or_default()
            };
            Some(format!("✓ Workspace compared · {changes}{transfer}{next}"))
        }
        SessionCallout::WorkspaceSyncVerified { .. } => Some(
            "✓ First workspace sync verified this session · Both workspace roots are synchronized."
                .into(),
        ),
    }
}

fn session_callout_is_embedded(state: &AppState, callout: &SessionCallout) -> bool {
    let SessionCallout::WorkspaceSyncVerified { job_id } = callout else {
        return false;
    };
    state.active_overlay() == Some(OverlayKind::SyncPreview)
        && state.remote_workspace.ux.job_id() == Some(job_id.as_str())
}

// ponytail: presentation-only model for workspace ribbon.
// Reads runtime truth from `state` but never mutates backend semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RibbonPhase {
    Commander,
    Ready,
    Scanning,
    Differences,
    Preview,
    Running,
    Verifying,
    Synchronized,
    DifferencesRemain,
    Inconclusive,
    VerificationFailed,
    VerificationCancelled,
    VerificationSuperseded,
    Blocked,
}

fn ribbon_phase(state: &AppState) -> RibbonPhase {
    use RibbonPhase::*;
    if !state.remote_workspace.enabled {
        return Commander;
    }
    match state.remote_workspace.ux {
        WorkspaceSyncUxState::Scanning => Scanning,
        WorkspaceSyncUxState::Preview { .. }
        | WorkspaceSyncUxState::ConfirmationRequired { .. } => Preview,
        WorkspaceSyncUxState::Launching { .. }
        | WorkspaceSyncUxState::Queued { .. }
        | WorkspaceSyncUxState::Cancelling { .. }
        | WorkspaceSyncUxState::Running { .. } => Running,
        WorkspaceSyncUxState::Verifying { .. } => Verifying,
        WorkspaceSyncUxState::Finished { .. } | WorkspaceSyncUxState::VerificationDiff { .. } => {
            if let Some(snap) = &state.remote_workspace.verification {
                match &snap.status {
                    SyncVerificationStatus::Finished(result) => match result.verdict {
                        VerifySynchronized => Synchronized,
                        VerifyDifferencesRemain { .. } => DifferencesRemain,
                        VerifyInconclusive { .. } => Inconclusive,
                    },
                    SyncVerificationStatus::Failed { .. } => VerificationFailed,
                    SyncVerificationStatus::Cancelled => VerificationCancelled,
                    SyncVerificationStatus::Superseded => VerificationSuperseded,
                    _ => Inconclusive,
                }
            } else {
                Inconclusive
            }
        }
        WorkspaceSyncUxState::Blocked { .. } => Blocked,
        _ => {
            if state.remote_workspace.diff.is_some() {
                Differences
            } else {
                Ready
            }
        }
    }
}

pub(super) fn workspace_ribbon_text(state: &AppState) -> String {
    let enabled = state.remote_workspace.enabled;
    let direction = state.remote_workspace.policy.direction;
    let (src_id, dst_id) = match direction {
        arx::workspace_sync::SyncDirection::LeftToRight => (
            provider_identity(&state.left.location),
            provider_identity(&state.right.location),
        ),
        arx::workspace_sync::SyncDirection::RightToLeft => (
            provider_identity(&state.right.location),
            provider_identity(&state.left.location),
        ),
    };
    let phase = ribbon_phase(state);

    let action_hint = if !enabled {
        "Ctrl+D Compare".into()
    } else {
        match phase {
            RibbonPhase::Commander => "Ctrl+D Compare".into(),
            RibbonPhase::Ready => "Ctrl+D Compare".into(),
            RibbonPhase::Scanning => "Comparing…".into(),
            RibbonPhase::Differences => {
                let count = state
                    .remote_workspace
                    .diff
                    .as_ref()
                    .map(diff_metric_summary)
                    .unwrap_or_default();
                format!("{} · Ctrl+X P Preview", count)
            }
            RibbonPhase::Preview => {
                if let Some(plan) = &state.remote_workspace.plan {
                    let ops = plan_metric_summary(plan);
                    format!("{} · Enter Execute", ops)
                } else {
                    "Preview · Enter Execute".into()
                }
            }
            RibbonPhase::Running => {
                #[allow(clippy::collapsible_if)]
                if let Some(current_job_id) = state.remote_workspace.ux.job_id() {
                    if let Some(job) = state.jobs.iter().find(|j| j.id == current_job_id) {
                        if let arx::jobs::JobProgress::WorkspaceSync(sync_prog) = &job.progress {
                            if let Some(pct) = sync_prog.percent() {
                                return format!("{} → {} · Syncing… {}%", src_id, dst_id, pct);
                            }
                        }
                    }
                }
                "Syncing…".into()
            }
            RibbonPhase::Verifying => "Verifying…".into(),
            RibbonPhase::Synchronized => "✓ SYNCHRONIZED".into(),
            RibbonPhase::DifferencesRemain => "⚠ DIFFERENCES REMAIN".into(),
            RibbonPhase::Inconclusive => "? INCONCLUSIVE".into(),
            RibbonPhase::VerificationFailed => "! VERIFICATION FAILED".into(),
            RibbonPhase::VerificationCancelled => "✗ VERIFICATION CANCELLED".into(),
            RibbonPhase::VerificationSuperseded => "… VERIFICATION SUPERSEDED".into(),
            RibbonPhase::Blocked => "BLOCKED".into(),
        }
    };

    if enabled {
        format!("WORKSPACE {} → {} · {}", src_id, dst_id, action_hint)
    } else {
        format!("COMMANDER {} ⇄ {} · {}", src_id, dst_id, action_hint)
    }
}

fn provider_identity(location: &Location) -> &'static str {
    match location.provider_id() {
        ProviderId::Local => "[LOCAL]",
        ProviderId::Sftp => "[SSH]",
        ProviderId::Archive => "[ARCHIVE]",
        _ => "[?]",
    }
}

fn diff_metric_summary(diff: &WorkspaceDiff) -> String {
    let changes = diff.entries.len();
    let bytes: u64 = diff
        .entries
        .iter()
        .filter_map(|e| {
            e.left
                .as_ref()
                .and_then(|f| f.size)
                .or_else(|| e.right.as_ref().and_then(|f| f.size))
        })
        .sum();
    format!("{} changes · {} B", changes, bytes)
}

fn plan_metric_summary(plan: &WorkspaceSyncPlan) -> String {
    let copies = plan
        .operations
        .iter()
        .filter(|o| matches!(o, arx::workspace_sync::WorkspaceSyncOperation::Copy { .. }))
        .count();
    let deletes = plan
        .operations
        .iter()
        .filter(|o| {
            matches!(
                o,
                arx::workspace_sync::WorkspaceSyncOperation::Delete { .. }
            )
        })
        .count();
    format!("{} copies · {} deletes", copies, deletes)
}
