use arx::app::{Action, AppState, QuickActionPrompt};
use arx::effect_dispatcher::{EffectDispatcher, EffectLane, EffectScope};
use arx::effects::{Effect, EffectEvent};
use arx::services::{QuickActionKind, QuickActionOutcome, QuickActionRequest};
use arx::vfs::{Entry, EntryKind, Location};

pub(super) fn display_safe_text(value: &str) -> String {
    let mut safe = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\n' => safe.push_str("\\n"),
            '\r' => safe.push_str("\\r"),
            '\t' => safe.push_str("\\t"),
            ch if ch.is_control() => {
                safe.push_str(&format!("\\u{{{:x}}}", ch as u32));
            }
            ch => safe.push(ch),
        }
    }
    safe
}

fn collect_quick_action_names(
    state: &AppState,
    focused: Option<&Entry>,
    active_entries: &[&Entry],
    files_only: bool,
) -> Result<Vec<String>, String> {
    let location = &state.active_pane().location;
    let names = if let Some(selected) = state.selection_names(state.active, location) {
        selected.iter().cloned().collect::<Vec<_>>()
    } else if let Some(entry) = focused {
        vec![entry.name.clone()]
    } else {
        return Err("Select a file or directory first".into());
    };

    for name in &names {
        let Some(entry) = active_entries
            .iter()
            .copied()
            .find(|entry| entry.name.as_str() == name.as_str())
        else {
            return Err(format!("Selection is stale: {}", display_safe_text(name)));
        };

        if files_only && entry.kind != EntryKind::File {
            return Err(format!(
                "SHA-256 requires regular files; {} is not a regular file",
                display_safe_text(name)
            ));
        }
    }

    Ok(names)
}

pub(super) fn quick_action_refresh_location(
    event: &EffectEvent,
    scope: &EffectScope,
) -> Option<Location> {
    let may_have_mutated = match event {
        EffectEvent::QuickActionFinished { result } => match result {
            Ok(QuickActionOutcome::Touched { .. }) | Ok(QuickActionOutcome::Compressed { .. }) => {
                true
            }
            Ok(QuickActionOutcome::Sha256 { .. }) => false,
            Err(failure) => matches!(
                failure.action,
                QuickActionKind::Touch | QuickActionKind::CompressTarGz
            ),
        },
        _ => false,
    };

    if !may_have_mutated {
        return None;
    }

    match scope {
        EffectScope::Location(location) => Some(location.clone()),
        EffectScope::Global | EffectScope::Workspace { .. } => None,
    }
}

pub(super) fn handle_action(
    state: &mut AppState,
    action: &Action,
    focused: Option<&Entry>,
    active_entries: &[&Entry],
    effect_dispatcher: &EffectDispatcher,
) -> bool {
    match action {
        Action::ComputeSha256 => {
            if state.pending_effect(EffectLane::QuickAction).is_some() {
                state.message = Some("Another Quick Action is still in progress".into());
                return true;
            }

            let Location::Local(dir) = state.active_pane().location.clone() else {
                state.message = Some("Quick Actions are currently local-only".into());
                return true;
            };

            let names = match collect_quick_action_names(state, focused, active_entries, true) {
                Ok(names) => names,
                Err(message) => {
                    state.message = Some(message);
                    return true;
                }
            };

            let location = Location::Local(dir.clone());
            let id = effect_dispatcher.dispatch(
                EffectLane::QuickAction,
                EffectScope::Location(location),
                Effect::QuickAction {
                    request: QuickActionRequest::Sha256 { dir, names },
                },
            );
            state.register_effect(EffectLane::QuickAction, id);
            state.message = Some("Computing SHA-256…".into());
        }
        Action::TouchFile => {
            if state.pending_effect(EffectLane::QuickAction).is_some() {
                state.message = Some("Another Quick Action is still in progress".into());
                return true;
            }

            let Location::Local(dir) = state.active_pane().location.clone() else {
                state.message = Some("Quick Actions are currently local-only".into());
                return true;
            };

            state.pending_mkdir_location = None;
            state.pending_quick_action_prompt = Some(QuickActionPrompt::Touch { dir });
            state.cmd.clear();
            state.cmd_input = true;
            state.message = Some("Touch file: enter child name".into());
        }
        Action::CompressTarGz => {
            if state.pending_effect(EffectLane::QuickAction).is_some() {
                state.message = Some("Another Quick Action is still in progress".into());
                return true;
            }

            let Location::Local(dir) = state.active_pane().location.clone() else {
                state.message = Some("Quick Actions are currently local-only".into());
                return true;
            };

            let names = match collect_quick_action_names(state, focused, active_entries, false) {
                Ok(names) => names,
                Err(message) => {
                    state.message = Some(message);
                    return true;
                }
            };

            state.pending_mkdir_location = None;
            state.pending_quick_action_prompt =
                Some(QuickActionPrompt::CompressTarGz { dir, names });
            state.cmd = "archive.tar.gz".into();
            state.cmd_input = true;
            state.message = Some("Compress: enter output tar.gz name".into());
        }
        _ => return false,
    }

    true
}

pub(super) fn submit_prompt(
    state: &mut AppState,
    prompt: QuickActionPrompt,
    command: String,
    effect_dispatcher: &EffectDispatcher,
) -> bool {
    if state.pending_effect(EffectLane::QuickAction).is_some() {
        state.message = Some("Another Quick Action is still in progress".into());
        return true;
    }

    let (request, location, status) = match prompt {
        QuickActionPrompt::Touch { dir } => {
            let location = Location::Local(dir.clone());
            (
                QuickActionRequest::Touch { dir, name: command },
                location,
                "Touching file…",
            )
        }
        QuickActionPrompt::CompressTarGz { dir, names } => {
            let location = Location::Local(dir.clone());
            (
                QuickActionRequest::CompressTarGz {
                    dir,
                    names,
                    output_name: command,
                },
                location,
                "Compressing to tar.gz…",
            )
        }
    };

    let id = effect_dispatcher.dispatch(
        EffectLane::QuickAction,
        EffectScope::Location(location),
        Effect::QuickAction { request },
    );
    state.register_effect(EffectLane::QuickAction, id);
    state.message = Some(status.into());
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn entry(name: &str, kind: EntryKind) -> Entry {
        Entry {
            name: name.into(),
            kind,
            size: None,
            modified_unix_ms: None,
        }
    }

    #[test]
    fn display_safe_text_escapes_controls_but_preserves_unicode() {
        assert_eq!(
            display_safe_text("a\nb\tc\rd\u{1b}ü"),
            "a\\nb\\tc\\rd\\u{1b}ü"
        );
    }

    #[test]
    fn name_collection_rejects_stale_selection_and_non_files_for_sha() {
        let state = AppState::default();
        let directory = entry("dir", EntryKind::Directory);
        let entries = [&directory];
        let error =
            collect_quick_action_names(&state, Some(&directory), &entries, true).unwrap_err();
        assert!(error.contains("requires regular files"));

        let mut state = AppState::default();
        let location = state.active_pane().location.clone();
        state.toggle_selection(state.active, &location, "missing\nname");
        let file = entry("present", EntryKind::File);
        let entries = [&file];
        let error = collect_quick_action_names(&state, Some(&file), &entries, true).unwrap_err();
        assert!(error.contains("Selection is stale"));
        assert!(error.contains("missing\\nname"));
        assert!(!error.contains("missing\nname"));
    }

    #[test]
    fn name_collection_uses_selection_or_focused_entry() {
        let alpha = entry("alpha", EntryKind::File);
        let beta = entry("beta", EntryKind::Directory);
        let entries = [&alpha, &beta];
        let state = AppState::default();
        assert_eq!(
            collect_quick_action_names(&state, Some(&beta), &entries, false).unwrap(),
            vec!["beta"]
        );

        let mut state = AppState::default();
        let location = state.active_pane().location.clone();
        state.toggle_selection(state.active, &location, "alpha");
        assert_eq!(
            collect_quick_action_names(&state, Some(&beta), &entries, false).unwrap(),
            vec!["alpha"]
        );
    }

    #[test]
    fn mutating_outcomes_refresh_but_sha_does_not() {
        let location = Location::Local(PathBuf::from("/tmp"));
        let scope = EffectScope::Location(location.clone());
        let touched = EffectEvent::QuickActionFinished {
            result: Ok(QuickActionOutcome::Touched {
                path: PathBuf::from("/tmp/file"),
            }),
        };
        let compressed = EffectEvent::QuickActionFinished {
            result: Ok(QuickActionOutcome::Compressed {
                path: PathBuf::from("/tmp/archive.tar.gz"),
                entries: 2,
            }),
        };
        let sha = EffectEvent::QuickActionFinished {
            result: Ok(QuickActionOutcome::Sha256 {
                dir: PathBuf::from("/tmp"),
                checksums: Vec::new(),
            }),
        };

        assert_eq!(
            quick_action_refresh_location(&touched, &scope),
            Some(location.clone())
        );
        assert_eq!(
            quick_action_refresh_location(&compressed, &scope),
            Some(location)
        );
        assert_eq!(quick_action_refresh_location(&sha, &scope), None);
    }
}
