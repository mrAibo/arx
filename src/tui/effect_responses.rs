use super::{quick_actions, remote_edit, schedule_pane_load};
use arx::app::{
    ActionAvailability, AppState, CommandItem, CommandKind, CommandTarget, OverlayKind, Pane,
};
use arx::effect_dispatcher::{EffectDispatcher, EffectLane, EffectResponse};
use arx::effects::EffectEvent;
use arx::services::{PaneLoader, QuickActionFailureKind, QuickActionOutcome};
use arx::vfs::Location;

pub(super) fn apply_received(
    dispatcher: &EffectDispatcher,
    mut response: EffectResponse,
    state: &mut AppState,
    pane_loader: &PaneLoader,
) {
    finalize_received_effect(dispatcher, &mut response);
    handle_effect_response(response, state, pane_loader);
}

pub(super) fn finalize_received_effect(
    dispatcher: &EffectDispatcher,
    response: &mut EffectResponse,
) {
    let was_cancelled = dispatcher
        .finish(response.id)
        .is_some_and(|cancellation| cancellation.is_cancelled());
    // #48/MAJOR#1: an explicitly cancelled queued download is a typed Cancelled,
    // never a generic Failed. apply_effect_event maps this to
    // RemoteEditOutcome::Cancelled; the Failed branch is reserved for real
    // provider/download errors.
    if was_cancelled && let EffectEvent::Downloaded { session } = &response.event {
        response.event = EffectEvent::RemoteEditCancelled {
            name: session.name.clone(),
            reason: arx::jobs::RemoteEditCancelReason::Queued,
        };
    }
}

pub(super) fn apply_effect_event(state: &mut AppState, lane: EffectLane, event: EffectEvent) {
    match event {
        EffectEvent::ShellCaptured {
            command,
            success,
            stdout,
            stderr,
        } => {
            let stdout = stdout.trim();
            let stderr = stderr.trim();
            let text = if !stdout.is_empty() {
                stdout
            } else if !stderr.is_empty() {
                stderr
            } else if success {
                "ok"
            } else {
                "failed"
            };
            state.message = Some(format!(": {command} — {}", truncate_message(text, 80)));
        }
        EffectEvent::ProcessExited { label, success } => {
            state.message = Some(format!(
                "{label} — {}",
                if success { "done" } else { "failed" }
            ));
        }
        EffectEvent::Spawned { label } => {
            state.message = Some(format!("{label} — started"));
        }
        EffectEvent::TmuxSessions { sessions } => {
            if sessions.is_empty() {
                state.message = Some("No tmux sessions found".into());
                return;
            }
            state.command_matches = sessions
                .into_iter()
                .map(|name| CommandItem {
                    // Display-safe title; exact raw name stays in the target.
                    title: super::quick_actions::display_safe_text(&name),
                    subtitle: Some("Attach tmux session".into()),
                    kind: CommandKind::Session,
                    target: CommandTarget::TmuxSession(name),
                    score: 0,
                    availability: ActionAvailability::Available,
                })
                .collect();
            state.open_overlay(OverlayKind::CommandCenter);
            state.overlay_list_state.select(Some(0));
        }
        EffectEvent::ScreenSessions { sessions } => {
            if sessions.is_empty() {
                state.message = Some("No screen sessions found".into());
                return;
            }
            state.command_matches = sessions
                .into_iter()
                .map(|session| {
                    let (subtitle, availability) = match session.status.unavailable_reason() {
                        Some(reason) => (
                            format!("GNU Screen — {reason}"),
                            ActionAvailability::Disabled {
                                reason: reason.to_string(),
                            },
                        ),
                        None => (
                            "Attach GNU Screen session".to_string(),
                            ActionAvailability::Available,
                        ),
                    };
                    CommandItem {
                        // Presentation is control-safe; the target keeps the
                        // EXACT raw id — never reconstructed from the title.
                        title: quick_actions::display_safe_text(&session.id),
                        subtitle: Some(subtitle),
                        kind: CommandKind::Session,
                        target: CommandTarget::ScreenSession(session.id),
                        score: 0,
                        availability,
                    }
                })
                .collect();
            state.open_overlay(OverlayKind::CommandCenter);
            state.overlay_list_state.select(Some(0));
        }
        EffectEvent::ViewerLines { title, lines } => {
            state.viewer_content = lines;
            state.viewer_scroll = 0;
            state.message = Some(title);
        }
        EffectEvent::InfrastructureLines { lines } => {
            state.infrastructure_lines = if lines.is_empty() {
                vec!["No SSH hosts discovered".into()]
            } else {
                lines
            }
        }
        EffectEvent::TreeLines { lines } => {
            state.tree_lines = if lines.is_empty() {
                vec!["(empty)".into()]
            } else {
                lines
            }
        }
        EffectEvent::PathOpened { path } => {
            state.message = Some(format!("Opened {}", path.display()));
        }
        EffectEvent::QuickActionFinished { result } => match result {
            Ok(QuickActionOutcome::Sha256 { checksums, .. }) => {
                let count = checksums.len();
                state.viewer_content = checksums
                    .into_iter()
                    .map(|checksum| {
                        format!(
                            "{}  {}",
                            checksum.sha256,
                            quick_actions::display_safe_text(&checksum.name)
                        )
                    })
                    .collect();
                state.viewer_scroll = 0;
                state.message = Some(format!("SHA-256 computed for {count} file(s)"));
            }
            Ok(QuickActionOutcome::Touched { path }) => {
                state.message = Some(format!(
                    "Touched {}",
                    quick_actions::display_safe_text(path.to_string_lossy().as_ref())
                ));
            }
            Ok(QuickActionOutcome::Compressed { path, entries }) => {
                state.message = Some(format!(
                    "Created {} from {entries} entr{}",
                    quick_actions::display_safe_text(path.to_string_lossy().as_ref()),
                    if entries == 1 { "y" } else { "ies" }
                ));
            }
            Err(failure) => {
                if failure.kind == QuickActionFailureKind::Cancelled {
                    state.message = Some(format!("{} cancelled", failure.action.label()));
                } else {
                    state.message = Some(format!(
                        "{} failed: {}",
                        failure.action.label(),
                        quick_actions::display_safe_text(&failure.message)
                    ));
                }
            }
        },
        EffectEvent::Downloaded { session } => {
            let download_name = &session.name;
            state.message = Some(format!("Downloaded: {download_name}"));
            let job_id = state.pending_remote_edit_job_id.clone();
            let mut session = session;
            session.job_id = job_id;
            if session.job_id.is_some() {
                // Downloaded: queued→awaiting-editor phase (TUI owns editor launch).
                remote_edit::publish_remote_edit_phase(
                    state,
                    arx::jobs::RemoteEditPhase::AwaitingEditor,
                );
            }
            state.pending_remote_edit_session = Some(session);
        }
        EffectEvent::WrittenBack { name } => {
            state.message = Some(format!("Uploaded: {name}"));
            // #51/MAJOR#1: Verifying is NOT invented here — it was already
            // published at the real verification boundary by the provider
            // progress closure (before verify_remote_matches). Only terminate.
            remote_edit::terminate_remote_edit_job(
                state,
                arx::jobs::RemoteEditOutcome::Completed,
                None,
            );
        }
        EffectEvent::NoChange { name } => {
            state.message = Some(format!("No changes: {name}"));
            remote_edit::terminate_remote_edit_job(
                state,
                arx::jobs::RemoteEditOutcome::NoChange,
                None,
            );
        }
        EffectEvent::RemoteConflict { name, reason } => {
            state.message = Some(format!(
                "{name} changed on remote — write-back refused: {reason}"
            ));
            remote_edit::terminate_remote_edit_job(
                state,
                arx::jobs::RemoteEditOutcome::Failed,
                Some(format!("remote conflict: {reason}")),
            );
        }
        EffectEvent::RecoveryRequired { name, details } => {
            state.message = Some(format!("{name}: RECOVERY REQUIRED — {details}"));
            // #51: recovery path exposes RollbackOrRecovery before the typed
            // RecoveryRequired terminal so the phase model is observable end-to-end.
            remote_edit::publish_remote_edit_phase(
                state,
                arx::jobs::RemoteEditPhase::RollbackOrRecovery,
            );
            remote_edit::terminate_remote_edit_job(
                state,
                arx::jobs::RemoteEditOutcome::RecoveryRequired,
                Some(format!("recovery required: {details}")),
            );
        }
        EffectEvent::WrittenBackWarning { name, warning } => {
            state.message = Some(format!("Uploaded {name} with warning: {warning}"));
            remote_edit::terminate_remote_edit_job(
                state,
                arx::jobs::RemoteEditOutcome::CommittedWithWarning,
                None,
            );
        }
        EffectEvent::Failed { label, error } => {
            // #51/MAJOR: a generic failure only terminates an in-flight Remote
            // Edit when it actually belongs to the RemoteEdit lane. An unrelated
            // effect (LeftPane, GlobalProcess, …) must NOT mutate another
            // session's lifecycle, job status, or pending ownership.
            if lane == EffectLane::RemoteEdit {
                state.message = Some(format!("{label} failed: {error}"));
                remote_edit::terminate_remote_edit_job(
                    state,
                    arx::jobs::RemoteEditOutcome::Failed,
                    Some(format!("{label} failed: {error}")),
                );
            } else {
                state.message = Some(format!("{label} failed: {error}"));
            }
        }
        EffectEvent::RemoteEditCancelled { name, reason } => {
            state.message = Some(format!("Remote edit cancelled: {name} ({reason:?})"));
            remote_edit::terminate_remote_edit_job(
                state,
                arx::jobs::RemoteEditOutcome::Cancelled,
                Some(format!("{reason:?}")),
            );
        }
    }
}

pub(super) fn handle_effect_response(
    response: EffectResponse,
    state: &mut AppState,
    pane_loader: &PaneLoader,
) {
    if !state.accepts_effect(response.id, response.lane, &response.scope) {
        return;
    }
    let quick_action_refresh =
        quick_actions::quick_action_refresh_location(&response.event, &response.scope);
    let refresh_origin = if matches!(
        &response.event,
        EffectEvent::WrittenBack { .. } | EffectEvent::WrittenBackWarning { .. }
    ) {
        state.pending_remote_edit_origin.clone()
    } else {
        None
    };
    let remote_terminal = response.lane == EffectLane::RemoteEdit
        && !matches!(&response.event, EffectEvent::Downloaded { .. });

    state.finish_effect(response.lane, response.id);
    apply_effect_event(state, response.lane, response.event);
    if remote_terminal {
        state.pending_remote_edit_origin = None;
    }

    if let Some((pane, location)) = refresh_origin
        && pane_still_at_location(state, pane, &location)
    {
        schedule_pane_load(pane_loader, state, pane);
    }

    if let Some(location) = quick_action_refresh {
        for pane in [Pane::Left, Pane::Right] {
            if pane_still_at_location(state, pane, &location) {
                schedule_pane_load(pane_loader, state, pane);
            }
        }
    }

    match response.lane {
        EffectLane::LeftPane => {
            schedule_pane_load(pane_loader, state, Pane::Left);
        }
        EffectLane::RightPane => {
            schedule_pane_load(pane_loader, state, Pane::Right);
        }
        _ => {}
    }
}

pub(super) fn pane_still_at_location(state: &AppState, pane: Pane, location: &Location) -> bool {
    match pane {
        Pane::Left => &state.left.location == location,
        Pane::Right => &state.right.location == location,
    }
}

fn truncate_message(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let prefix: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{prefix}...")
    } else {
        prefix
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arx::effect_dispatcher::{EffectId, EffectScope};
    use arx::effects::Effect;
    use arx::services::ChecksumResult;
    use arx::vfs::{ProviderRegistry, RemoteEditRevision, RemoteEditSession, RemoteEditState};
    use std::path::PathBuf;
    use std::sync::Arc;

    fn loader() -> PaneLoader {
        PaneLoader::channel(ProviderRegistry::new()).0
    }

    fn response(
        id: EffectId,
        lane: EffectLane,
        scope: EffectScope,
        event: EffectEvent,
    ) -> EffectResponse {
        EffectResponse {
            id,
            lane,
            scope,
            event,
        }
    }

    // ── #7 review: display-safe multiplexer presentation, exact raw targets ──
    #[test]
    fn r7_screen_rows_escape_control_chars_but_target_stays_exact() {
        let raw = "123.demo\u{1b}[31m".to_string();
        let mut state = AppState::default();
        apply_effect_event(
            &mut state,
            EffectLane::TmuxDiscovery,
            EffectEvent::ScreenSessions {
                sessions: vec![arx::effects::ScreenSessionInfo {
                    id: raw.clone(),
                    status: arx::effects::ScreenSessionStatus::Detached,
                }],
            },
        );
        assert_eq!(state.command_matches.len(), 1);
        let row = &state.command_matches[0];
        // Presentation escaped:
        assert!(
            row.title.contains("\\u{1b}"),
            "title must escape ESC: {:?}",
            row.title
        );
        assert!(!row.title.contains('\u{1b}'), "no literal ESC in title");
        // Operational identity exact:
        match &row.target {
            CommandTarget::ScreenSession(id) => assert_eq!(id, &raw),
            other => panic!("wrong target: {other:?}"),
        }
    }

    #[test]
    fn r7_tmux_rows_escape_control_chars_but_target_stays_exact() {
        let raw = "sess\n;rm".to_string();
        let mut state = AppState::default();
        apply_effect_event(
            &mut state,
            EffectLane::TmuxDiscovery,
            EffectEvent::TmuxSessions {
                sessions: vec![raw.clone()],
            },
        );
        let row = &state.command_matches[0];
        assert!(
            row.title.contains("\\n"),
            "newline escaped: {:?}",
            row.title
        );
        assert!(!row.title.contains('\n'));
        match &row.target {
            CommandTarget::TmuxSession(name) => assert_eq!(name, &raw),
            other => panic!("wrong target: {other:?}"),
        }
    }

    #[test]
    fn effect_response_rejects_stale_id_wrong_lane_and_wrong_scope() {
        let pane_loader = loader();
        let location = Location::Local(PathBuf::from("/accepted"));
        let mut state = AppState::default();
        state.left.location = location.clone();
        state.register_effect(EffectLane::LeftPane, EffectId(2));

        for rejected in [
            response(
                EffectId(1),
                EffectLane::LeftPane,
                EffectScope::Location(location.clone()),
                EffectEvent::Spawned {
                    label: "stale".into(),
                },
            ),
            response(
                EffectId(2),
                EffectLane::RightPane,
                EffectScope::Location(location.clone()),
                EffectEvent::Spawned {
                    label: "wrong lane".into(),
                },
            ),
            response(
                EffectId(2),
                EffectLane::LeftPane,
                EffectScope::Location(Location::Local(PathBuf::from("/wrong-scope"))),
                EffectEvent::Spawned {
                    label: "wrong scope".into(),
                },
            ),
        ] {
            handle_effect_response(rejected, &mut state, &pane_loader);
            assert!(state.message.is_none());
            assert_eq!(
                state.pending_effect(EffectLane::LeftPane),
                Some(EffectId(2))
            );
            assert!(state.pending_pane_loads.is_empty());
        }
    }

    #[tokio::test]
    async fn cancelled_downloaded_effect_becomes_typed_remote_edit_cancelled() {
        let registry = ProviderRegistry::new();
        let (dispatcher, _responses) = EffectDispatcher::channel(registry);
        let id = dispatcher.dispatch(
            EffectLane::RemoteEdit,
            EffectScope::Global,
            Effect::SpawnShell {
                command: "true".into(),
            },
        );
        assert!(dispatcher.cancel(id));
        let mut response = response(
            id,
            EffectLane::RemoteEdit,
            EffectScope::Global,
            EffectEvent::Downloaded {
                session: RemoteEditSession {
                    name: "note.txt".into(),
                    location: Location::Local(PathBuf::from("/tmp")),
                    editor: "true".into(),
                    revision: RemoteEditRevision::new(Vec::new(), 0o600, 0, 0),
                    temp_dir: Arc::new(tempfile::tempdir().unwrap()),
                    state: RemoteEditState::ReadyToEdit,
                    job_id: None,
                },
            },
        );

        finalize_received_effect(&dispatcher, &mut response);

        assert!(matches!(
            response.event,
            EffectEvent::RemoteEditCancelled {
                name,
                reason: arx::jobs::RemoteEditCancelReason::Queued,
            } if name == "note.txt"
        ));
    }

    #[tokio::test]
    async fn remote_edit_written_back_refreshes_only_unchanged_origin_pane() {
        let origin = Location::Local(PathBuf::from("/tmp"));

        let pane_loader = loader();
        let mut state = AppState::default();
        state.left.location = origin.clone();
        state.pending_remote_edit_origin = Some((Pane::Left, origin.clone()));
        state.register_effect(EffectLane::RemoteEdit, EffectId(1));
        handle_effect_response(
            response(
                EffectId(1),
                EffectLane::RemoteEdit,
                EffectScope::Location(origin.clone()),
                EffectEvent::WrittenBack { name: "a".into() },
            ),
            &mut state,
            &pane_loader,
        );
        assert!(state.pending_pane_loads.contains_key(&Pane::Left));
        assert!(state.pending_remote_edit_origin.is_none());

        let pane_loader = loader();
        let mut moved = AppState::default();
        moved.left.location = Location::Local(PathBuf::from("/elsewhere"));
        moved.pending_remote_edit_origin = Some((Pane::Left, origin.clone()));
        moved.register_effect(EffectLane::RemoteEdit, EffectId(2));
        handle_effect_response(
            response(
                EffectId(2),
                EffectLane::RemoteEdit,
                EffectScope::Location(origin),
                EffectEvent::WrittenBackWarning {
                    name: "a".into(),
                    warning: "metadata".into(),
                },
            ),
            &mut moved,
            &pane_loader,
        );
        assert!(moved.pending_pane_loads.is_empty());
        assert!(moved.pending_remote_edit_origin.is_none());
    }

    #[tokio::test]
    async fn quick_action_touch_and_compress_refresh_origin_but_sha_does_not() {
        let origin = Location::Local(PathBuf::from("/tmp"));
        let other = Location::Local(PathBuf::from("/elsewhere"));
        let events = [
            (
                EffectEvent::QuickActionFinished {
                    result: Ok(QuickActionOutcome::Touched {
                        path: PathBuf::from("/tmp/a"),
                    }),
                },
                true,
            ),
            (
                EffectEvent::QuickActionFinished {
                    result: Ok(QuickActionOutcome::Compressed {
                        path: PathBuf::from("/tmp/a.tar.gz"),
                        entries: 1,
                    }),
                },
                true,
            ),
            (
                EffectEvent::QuickActionFinished {
                    result: Ok(QuickActionOutcome::Sha256 {
                        dir: PathBuf::from("/tmp"),
                        checksums: vec![ChecksumResult {
                            name: "a".into(),
                            sha256: "00".into(),
                        }],
                    }),
                },
                false,
            ),
        ];

        for (index, (event, refreshes)) in events.into_iter().enumerate() {
            let pane_loader = loader();
            let mut state = AppState::default();
            state.left.location = origin.clone();
            state.right.location = other.clone();
            let id = EffectId(index as u64 + 1);
            state.register_effect(EffectLane::QuickAction, id);
            handle_effect_response(
                response(
                    id,
                    EffectLane::QuickAction,
                    EffectScope::Location(origin.clone()),
                    event,
                ),
                &mut state,
                &pane_loader,
            );
            assert_eq!(
                state.pending_pane_loads.contains_key(&Pane::Left),
                refreshes
            );
            assert!(!state.pending_pane_loads.contains_key(&Pane::Right));
        }
    }

    #[tokio::test]
    async fn left_and_right_effect_lanes_refresh_only_their_pane() {
        for (id, lane, expected, unexpected) in [
            (EffectId(1), EffectLane::LeftPane, Pane::Left, Pane::Right),
            (EffectId(2), EffectLane::RightPane, Pane::Right, Pane::Left),
        ] {
            let pane_loader = loader();
            let mut state = AppState::default();
            state.register_effect(lane, id);
            handle_effect_response(
                response(
                    id,
                    lane,
                    EffectScope::Global,
                    EffectEvent::Spawned {
                        label: "load".into(),
                    },
                ),
                &mut state,
                &pane_loader,
            );
            assert!(state.pending_pane_loads.contains_key(&expected));
            assert!(!state.pending_pane_loads.contains_key(&unexpected));
        }
    }
}
