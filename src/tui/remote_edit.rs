use super::*;

pub(super) fn begin_sftp_edit(
    state: &mut AppState,
    location: Location,
    name: String,
    editor: &str,
    effect_dispatcher: &EffectDispatcher,
) {
    // Remote: download → edit → write-back
    if state.pending_effects.contains_key(&EffectLane::RemoteEdit)
        || state.pending_remote_edit_origin.is_some()
    {
        state.message = Some("Another remote edit is still in progress".into());
        return;
    }

    state.pending_remote_edit_session = None;
    state.pending_remote_edit_origin = Some((state.active, location.clone()));
    // #51: one RemoteEdit job observed across all phases (download→editor→writeback).
    let re_job = state
        .job_manager
        .as_ref()
        .expect("job manager bound")
        .create_job(
            "remote-edit",
            arx::jobs::JobKind::RemoteEdit,
            format!("Remote edit {name}"),
            Some(location.clone()),
            None,
        );
    let _ = state
        .job_manager
        .as_ref()
        .expect("job manager bound")
        .publish_event(
            state.job_events.as_ref().expect("job events bound"),
            arx::jobs::JobEvent::Running {
                id: re_job.id.clone(),
            },
        );
    state.pending_remote_edit_job_id = Some(re_job.id.clone());
    // Observable phases: queued → downloading (job id now set).
    publish_remote_edit_phase(state, arx::jobs::RemoteEditPhase::Queued);
    publish_remote_edit_phase(state, arx::jobs::RemoteEditPhase::Downloading);
    let id = effect_dispatcher.dispatch(
        EffectLane::RemoteEdit,
        EffectScope::Location(location.clone()),
        Effect::DownloadRemoteFile {
            location: location.clone(),
            name: name.clone(),
            editor: editor.to_string(),
        },
    );
    state.register_effect(EffectLane::RemoteEdit, id);
    state.message = Some(format!("Downloading: {name}..."));

    // ponytail: Phase 2+3 handled in select! when Downloaded arrives
}

pub(super) async fn drive_deferred_editor(
    state: &mut AppState,
    configured_editor: Option<&str>,
    terminal_session: &mut TuiTerminalSession,
    effect_dispatcher: &EffectDispatcher,
) -> io::Result<bool> {
    state.pending_editor = false;
    // Terminalize (Cancelled/Failed) if the originating pane moved away
    // or the session is no longer ready; otherwise proceed to launch.
    if finalize_remote_edit_if_stale(state) {
        return Ok(true);
    }
    if let Some(mut session) = state.pending_remote_edit_session.take() {
        let editor_cmd = if let Some(cfg_editor) = configured_editor {
            cfg_editor.to_string()
        } else {
            session.editor.clone()
        };
        let working_path = session.temp_dir.path().join("working");
        session.state = RemoteEditState::Editing;
        // #51: observe phase transition (TUI still owns editor lifecycle).
        publish_remote_edit_phase(state, arx::jobs::RemoteEditPhase::Editing);
        let editor_result = terminal_session
            .suspend_while(|| DesktopService::open_editor(&editor_cmd, &working_path))
            .await?;
        if let Some(effect) = finish_remote_editor(session, editor_result, state) {
            let location = match &effect {
                Effect::WriteBackRemoteFile {
                    session,
                    progress: _,
                } => session.location.clone(),
                _ => unreachable!("remote editor can only schedule write-back"),
            };
            let id = effect_dispatcher.dispatch(
                EffectLane::RemoteEdit,
                EffectScope::Location(location),
                effect,
            );
            state.register_effect(EffectLane::RemoteEdit, id);
        }
    }
    Ok(false)
}

pub(super) fn finish_remote_editor(
    mut session: RemoteEditSession,
    editor_result: io::Result<()>,
    state: &mut AppState,
) -> Option<Effect> {
    if let Err(error) = editor_result {
        session.state = RemoteEditState::Failed;
        // #51: editor failure terminalizes the job as typed Failed (no leak).
        terminate_remote_edit_job(
            state,
            arx::jobs::RemoteEditOutcome::Failed,
            Some(error.to_string()),
        );
        state.message = Some(format!("Editor failed: {error}"));
        return None;
    }

    session.state = RemoteEditState::WritingBack;
    // #51/MAJOR#9: supply a narrow synchronous Send+Sync progress callback so
    // Verifying is emitted at the real verification boundary inside the provider,
    // in program order BEFORE the terminal event — no detached relay, no scheduler
    // races. The callback captures only the Send+Sync handles (job_manager +
    // event sink + id), never &AppState (AppState is not Sync). The provider never
    // knows about JobManager; it just calls progress(phase).
    publish_remote_edit_phase(state, arx::jobs::RemoteEditPhase::ValidatingWorkingCopy);
    publish_remote_edit_phase(state, arx::jobs::RemoteEditPhase::WriteBack);
    let job_manager = state.job_manager.clone();
    let job_events = state.job_events.clone();
    let job_id = session.job_id.clone();
    let progress: arx::vfs::RemoteEditProgressFn = std::sync::Arc::new(move |phase| {
        if let (Some(jm), Some(events), Some(id)) = (&job_manager, &job_events, &job_id) {
            jm.publish_remote_edit_phase(events, id, phase);
        }
    });
    Some(Effect::WriteBackRemoteFile {
        session,
        progress: ProgressSlot(Some(progress)),
    })
}

/// Centralized RemoteEdit terminalization.
///
/// Session-scoped pending state (origin, session, deferred editor) is cleared
/// UNCONDITIONALLY: once a Remote Edit reaches a terminal outcome, no leftover
/// ownership may survive even when no JobId publication is available (e.g. an
/// isolated lifecycle where the job id was never registered). Job terminal
/// publication stays conditional on an actual job id so we never publish a
/// terminal event for a job that does not exist.
///
/// Called from every terminal production path so no job leaks as Running and no
/// stale session/origin remains.
pub(super) fn terminate_remote_edit_job(
    state: &mut AppState,
    outcome: arx::jobs::RemoteEditOutcome,
    error: Option<String>,
) {
    // Unconditional cleanup of session-scoped pending ownership.
    state.pending_remote_edit_origin = None;
    state.pending_remote_edit_session = None;
    state.pending_editor = false;
    if let Some(jid) = state.pending_remote_edit_job_id.take() {
        state
            .job_manager
            .as_ref()
            .expect("job manager bound")
            .terminate_remote_edit(
                state.job_events.as_ref().expect("job events bound"),
                &jid,
                outcome,
                error,
            );
    }
}

/// Publish an observable RemoteEdit phase on the shared Job Manager.
pub(super) fn publish_remote_edit_phase(state: &AppState, phase: arx::jobs::RemoteEditPhase) {
    if let Some(jid) = &state.pending_remote_edit_job_id {
        state
            .job_manager
            .as_ref()
            .expect("job manager bound")
            .publish_remote_edit_phase(
                state.job_events.as_ref().expect("job events bound"),
                jid,
                phase,
            );
    }
}

/// Production stale/invalid terminalization for a deferred remote edit. Runs the
/// exact same cancellation logic that `event_loop`'s deferred-launch branch
/// uses: if the originating pane navigated away → Cancelled; if the session is
/// no longer ReadyToEdit → Failed. Terminalizes exactly once and clears all
/// in-flight ownership. Returns true if it handled the job (caller should skip
/// launching the editor). Drives the real finalization path so tests exercise
/// production behavior instead of poking fields.
pub(super) fn finalize_remote_edit_if_stale(state: &mut AppState) -> bool {
    let Some(session) = state.pending_remote_edit_session.take() else {
        return false;
    };
    let origin_matches =
        state
            .pending_remote_edit_origin
            .as_ref()
            .is_some_and(|(pane, location)| {
                location == &session.location && pane_still_at_location(state, *pane, location)
            });
    if session.state != RemoteEditState::ReadyToEdit {
        state.pending_remote_edit_origin = None;
        terminate_remote_edit_job(
            state,
            arx::jobs::RemoteEditOutcome::Failed,
            Some("remote edit session invalid".into()),
        );
        return true;
    }
    if !origin_matches {
        state.pending_remote_edit_origin = None;
        terminate_remote_edit_job(
            state,
            arx::jobs::RemoteEditOutcome::Cancelled,
            Some("originating pane navigated away".into()),
        );
        return true;
    }
    state.pending_remote_edit_session = Some(session);
    false
}
