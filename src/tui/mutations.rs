use super::*;
use arx::services::{MutationError, MutationService};
use ratatui::{Frame, layout::Rect};

#[allow(clippy::too_many_arguments)]
pub(super) fn handle_action(
    state: &mut AppState,
    action: &Action,
    focused: Option<&Entry>,
    focused_listed: Option<&ListedEntry>,
    active_entries: &[&Entry],
    active_listed: &[&ListedEntry],
    sync: &SyncUiRuntime,
    pane_loader: &PaneLoader,
) -> bool {
    match action {
        Action::Mkdir => {
            state.pending_quick_action_prompt = None;
            let provider_id = state.active_pane().location.provider_id();
            if provider_id == arx::vfs::ProviderId::Sftp {
                // SFTP: use provider-backed mkdir via frozen location
                state.pending_mkdir_location = Some(state.active_pane().location.clone());
                state.cmd = String::new();
                state.cmd_input = true;
            } else if provider_id == arx::vfs::ProviderId::S3 {
                // S3: freeze exact Location::S3 at prompt start; provider call on Enter.
                state.pending_mkdir_location = Some(state.active_pane().location.clone());
                state.cmd = String::new();
                state.cmd_input = true;
            } else {
                // Local: keep existing shell-based mkdir (no regression)
                state.cmd = "mkdir ".into();
                state.cmd_input = true;
            }
        }
        Action::Delete => {
            let names: Vec<String> = state
                .selection_names(state.active, &state.active_pane().location)
                .map(|names| names.iter().cloned().collect())
                .or_else(|| focused.map(|entry| vec![entry.name.clone()]))
                .unwrap_or_default();
            if names.is_empty() {
                state.message = Some("Select a file or directory to delete".into());
                return true;
            }

            if state.active_pane().location.provider_id() == arx::vfs::ProviderId::WebDAV {
                let selected_names: Vec<String> = state
                    .selection_names(state.active, &state.active_pane().location)
                    .map(|names| names.iter().cloned().collect())
                    .unwrap_or_default();
                match arx::services::prepare_webdav_recursive_delete_batch(
                    &state.active_pane().location,
                    &selected_names,
                    focused_listed,
                    active_listed,
                ) {
                    Ok(plan) => {
                        let root_count = plan.sources.len();
                        state.pending_webdav_delete = Some(plan);
                        state.message = Some(if root_count == 1 {
                            "Permanently delete this WebDAV directory tree? Enter to confirm, Escape to cancel"
                                .into()
                        } else {
                            format!(
                                "Permanently delete {root_count} WebDAV directory trees? Enter to confirm, Escape to cancel"
                            )
                        });
                    }
                    Err(error) => state.message = Some(error),
                }
                return true;
            }

            // SFTP: freeze plan for confirmation (no mutation yet)
            if state.active_pane().location.provider_id() == arx::vfs::ProviderId::Sftp {
                let targets: Vec<arx::vfs::RemoteDeleteTarget> = names
                    .iter()
                    .filter_map(|name| {
                        // Resolve real EntryKind from active pane listing
                        let entry = active_entries.iter().find(|e| e.name == *name)?;
                        let path = match &state.active_pane().location {
                            arx::vfs::Location::Sftp { path: p, .. } => {
                                format!("{p}/{name}")
                            }
                            _ => unreachable!(),
                        };
                        Some(arx::vfs::RemoteDeleteTarget {
                            name: name.clone(),
                            kind: entry.kind,
                            path,
                        })
                    })
                    .collect();
                if targets.len() != names.len() {
                    state.message = Some("Selection no longer matches directory contents".into());
                    return true;
                }
                state.pending_delete = Some(arx::vfs::RemoteDeletePlan {
                    location: state.active_pane().location.clone(),
                    targets,
                    created_at: std::time::Instant::now(),
                });
                state.message =
                    Some("Press Enter to confirm permanent deletion, Escape to cancel".into());
                return true;
            }

            // S3: freeze plan for confirmation (no mutation yet). Key is derived
            // from the frozen Location prefix + selected name — no dependency on
            // the (currently deferred) S3 listing path. A trailing-slash name is
            // a prefix marker (Directory); everything else is a File object.
            if let arx::vfs::Location::S3 { prefix, .. } = &state.active_pane().location {
                let targets: Vec<arx::vfs::RemoteDeleteTarget> = names
                    .iter()
                    .map(|name| {
                        if name.ends_with('/') {
                            arx::vfs::RemoteDeleteTarget {
                                name: name.clone(),
                                kind: arx::vfs::EntryKind::Directory,
                                path: name.clone(),
                            }
                        } else {
                            let key = if prefix.is_empty() {
                                name.clone()
                            } else {
                                format!("{prefix}/{name}")
                            };
                            arx::vfs::RemoteDeleteTarget {
                                name: name.clone(),
                                kind: arx::vfs::EntryKind::File,
                                path: key,
                            }
                        }
                    })
                    .collect();
                state.pending_delete = Some(arx::vfs::RemoteDeletePlan {
                    location: state.active_pane().location.clone(),
                    targets,
                    created_at: std::time::Instant::now(),
                });
                state.message =
                    Some("Press Enter to confirm permanent deletion, Escape to cancel".into());
                return true;
            }

            // Local: existing trash path
            let Location::Local(dir) = state.active_pane().location.clone() else {
                state.message = Some("Trash is currently available for local files only".into());
                return true;
            };
            let job = sync.jobs.create_job(
                "trash",
                arx::jobs::JobKind::Delete,
                format!("Trash {}", names.join(", ")),
                Some(Location::Local(dir.clone())),
                None,
            );
            let id = job.id.clone();
            let cancel = job.cancel.clone();
            state.jobs = sync.jobs.snapshot();
            let jobs = sync.jobs.clone();
            let tx = sync.job_events.clone();
            let job_id = id.clone();
            tokio::spawn(async move {
                if !jobs.publish_event(&tx, arx::jobs::JobEvent::Running { id: job_id.clone() }) {
                    return;
                }
                let tx_progress = tx.clone();
                let progress_id = job_id.clone();
                let progress_jobs = jobs.clone();
                let result = MutationService::trash_local(dir, names, cancel, move |progress| {
                    let percent = progress.completed.saturating_mul(100) / progress.total.max(1);
                    let _ = progress_jobs.publish_event(
                        &tx_progress,
                        arx::jobs::JobEvent::Progress {
                            id: progress_id.clone(),
                            progress: arx::jobs::Progress::Percent(percent as u8).into(),
                        },
                    );
                })
                .await;
                match result {
                    Ok(outcome) => {
                        let _ = jobs.publish_event(
                            &tx,
                            arx::jobs::JobEvent::Completed {
                                id: job_id,
                                result: arx::jobs::JobResult::generic(
                                    format!("Trashed {} item(s)", outcome.completed),
                                    outcome.completed,
                                ),
                            },
                        );
                    }
                    Err(MutationError::Cancelled { completed }) => {
                        let _ = jobs.publish_event(
                            &tx,
                            arx::jobs::JobEvent::Cancelled {
                                id: job_id,
                                result: arx::jobs::JobResult::generic(
                                    format!("Cancelled after {completed} item(s)"),
                                    completed,
                                ),
                            },
                        );
                    }
                    Err(error) => {
                        let _ = jobs.publish_event(
                            &tx,
                            arx::jobs::JobEvent::Failed {
                                id: job_id,
                                error: error.to_string(),
                                result: None,
                            },
                        );
                    }
                }
            });
            state.clear_selection();
            state.message = Some(format!("Trash queued ({id})"));
        }
        Action::ConfirmRemoteDelete => {
            if let Some(plan) = state.pending_webdav_delete.take() {
                let Location::WebDav { target, .. } = state.active_pane().location.clone() else {
                    state.message = Some("WebDAV delete context changed".into());
                    return true;
                };
                let provider = match state.registry.webdav_provider_for_mutation(&target) {
                    Ok(provider) => provider,
                    Err(error) => {
                        state.message = Some(error.to_string());
                        return true;
                    }
                };
                let pane = state.active;
                let location = state.active_pane().location.clone();
                let loader = pane_loader.clone();
                let jobs = sync.jobs.clone();
                let tx = sync.job_events.clone();
                let root_count = plan.sources.len();
                let job_description = if root_count == 1 {
                    format!("Delete WebDAV tree {}", plan.presentation_name)
                } else {
                    format!("Delete {root_count} WebDAV trees")
                };
                let job = jobs.create_job(
                    "webdav-recursive-delete",
                    arx::jobs::JobKind::Delete,
                    job_description,
                    Some(location.clone()),
                    None,
                );
                state.jobs = jobs.snapshot();
                let cancel = job.cancel.clone();
                let id = job.id.clone();
                let queued_id = id.clone();
                let _ = jobs.publish_event(&tx, arx::jobs::JobEvent::Running { id: id.clone() });
                tokio::spawn(async move {
                    let progress_jobs = jobs.clone();
                    let progress_tx = tx.clone();
                    let progress_id = id.clone();
                    let result = MutationService::delete_webdav_trees(
                        provider,
                        plan.sources,
                        cancel,
                        move |progress| {
                            let percent =
                                progress.completed.saturating_mul(100) / progress.total.max(1);
                            let _ = progress_jobs.publish_event(
                                &progress_tx,
                                arx::jobs::JobEvent::Progress {
                                    id: progress_id.clone(),
                                    progress: arx::jobs::Progress::Percent(percent as u8).into(),
                                },
                            );
                        },
                    )
                    .await;
                    let mutated = !matches!(
                        &result,
                        Err(arx::services::WebDavDeleteError::PreMutation { .. })
                            | Err(arx::services::WebDavDeleteError::Cancelled { completed: 0, .. })
                    );
                    match result {
                        Ok(outcome) => {
                            let _ = jobs.publish_event(
                                &tx,
                                arx::jobs::JobEvent::Completed {
                                    id: id.clone(),
                                    result: arx::jobs::JobResult::generic(
                                        format!("Deleted {} WebDAV item(s)", outcome.completed),
                                        outcome.completed,
                                    ),
                                },
                            );
                        }
                        Err(arx::services::WebDavDeleteError::Cancelled { completed, total }) => {
                            let _ = jobs.publish_event(
                                &tx,
                                arx::jobs::JobEvent::Cancelled {
                                    id: id.clone(),
                                    result: arx::jobs::JobResult::generic(
                                        format!("Cancelled after {completed} of {total} deleted"),
                                        completed,
                                    ),
                                },
                            );
                        }
                        Err(
                            error @ arx::services::WebDavDeleteError::Partial { completed, .. },
                        )
                        | Err(
                            error @ arx::services::WebDavDeleteError::RecoveryRequired {
                                completed,
                                ..
                            },
                        ) => {
                            let _ = jobs.publish_event(
                                &tx,
                                arx::jobs::JobEvent::Failed {
                                    id: id.clone(),
                                    error: error.to_string(),
                                    result: Some(arx::jobs::JobResult::generic(
                                        error.to_string(),
                                        completed,
                                    )),
                                },
                            );
                        }
                        Err(error) => {
                            let _ = jobs.publish_event(
                                &tx,
                                arx::jobs::JobEvent::Failed {
                                    id: id.clone(),
                                    error: error.to_string(),
                                    result: None,
                                },
                            );
                        }
                    }
                    if mutated {
                        let _ = loader.load(pane, location, PaneLoadPurpose::Refresh);
                    }
                });
                state.message = Some(format!("WebDAV recursive delete queued ({queued_id})"));
                return true;
            }
            let Some(plan) = state.pending_delete.take() else {
                return true;
            };
            let registry = state.registry.clone();
            let pane = state.active;
            let loader = pane_loader.clone();
            let location = plan.location.clone();
            let targets = plan.targets;
            let target_count = targets.len();
            let jobs = sync.jobs.clone();
            let tx = sync.job_events.clone();

            let job = jobs.create_job(
                "remote-delete",
                arx::jobs::JobKind::Delete,
                format!("Permanent delete {} target(s)", targets.len()),
                Some(location.clone()),
                None,
            );

            let _ = jobs.publish_event(&tx, arx::jobs::JobEvent::Running { id: job.id.clone() });

            tokio::spawn(async move {
                let mut completed: usize = 0;
                let mut failed: usize = 0;
                let mut cancelled = false;

                // ── Preflight: revalidate all frozen targets ──────────────
                let (provider, parent_path) = match registry.provider_for_location(&location) {
                    Ok(p) => p,
                    Err(e) => {
                        let _ = jobs.publish_event(
                            &tx,
                            arx::jobs::JobEvent::Failed {
                                id: job.id.clone(),
                                error: format!("Cannot access location: {e}"),
                                result: None,
                            },
                        );
                        return;
                    }
                };

                let fresh_listing = match provider.list_async(&parent_path).await {
                    Ok(entries) => entries,
                    Err(e) => {
                        let _ = jobs.publish_event(
                            &tx,
                            arx::jobs::JobEvent::Failed {
                                id: job.id.clone(),
                                error: format!("Cannot re-list directory: {e}"),
                                result: None,
                            },
                        );
                        return;
                    }
                };

                for target in &targets {
                    match fresh_listing.iter().find(|e| e.name == target.name) {
                        None => {
                            let _ = jobs.publish_event(
                        &tx,
                        arx::jobs::JobEvent::Failed {
                            id: job.id.clone(),
                            error: format!(
                                "Remote contents changed: '{}' no longer exists. Review selection.",
                                target.name
                            ),
                            result: None,
                        },
                    );
                            return;
                        }
                        Some(entry) if entry.kind != target.kind => {
                            let _ = jobs.publish_event(
                        &tx,
                        arx::jobs::JobEvent::Failed {
                            id: job.id.clone(),
                            error: format!(
                                "Remote contents changed: '{}' type changed. Review selection.",
                                target.name
                            ),
                            result: None,
                        },
                    );
                            return;
                        }
                        Some(entry) if entry.kind == arx::vfs::EntryKind::Directory => {
                            // S3: a "directory" is a prefix. Deletion is only safe
                            // when it is an empty marker (exactly one zero-byte
                            // object equal to the prefix). Anything else fails
                            // closed — no recursive prefix deletion.
                            if let arx::vfs::Location::S3 { .. } = &location {
                                match registry
                                    .prove_empty_s3_prefix_at(&location, &target.path)
                                    .await
                                {
                                    Ok(true) => {} // empty marker — allowed
                                    Ok(false) => {
                                        let _ = jobs.publish_event(
                                    &tx,
                                    arx::jobs::JobEvent::Failed {
                                        id: job.id.clone(),
                                        error: format!(
                                            "S3 prefix '{}' is not an empty marker. Recursive prefix delete is not supported. Nothing was deleted.",
                                            target.name
                                        ),
                                        result: None,
                                    },
                                );
                                        return;
                                    }
                                    Err(e) => {
                                        let _ = jobs.publish_event(
                                    &tx,
                                    arx::jobs::JobEvent::Failed {
                                        id: job.id.clone(),
                                        error: format!(
                                            "Cannot verify S3 prefix '{}' is empty: {}. Nothing was deleted.",
                                            target.name, e
                                        ),
                                        result: None,
                                    },
                                );
                                        return;
                                    }
                                }
                            } else {
                                match provider.list_async(&target.path).await {
                                    Ok(children) if !children.is_empty() => {
                                        let _ = jobs.publish_event(
                                &tx,
                                arx::jobs::JobEvent::Failed {
                                    id: job.id.clone(),
                                    error: format!(
                                        "Recursive remote delete is not supported: '{}' is not empty",
                                        target.name
                                    ),
                                    result: None,
                                },
                            );
                                        return;
                                    }
                                    Ok(_) => {} // empty directory — allowed
                                    Err(e) => {
                                        let _ = jobs.publish_event(
                                &tx,
                                arx::jobs::JobEvent::Failed {
                                    id: job.id.clone(),
                                    error: format!(
                                        "Cannot verify that remote directory '{}' is empty: {}. Nothing was deleted.",
                                        target.name, e
                                    ),
                                    result: None,
                                },
                            );
                                        return;
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                // ── All targets validated — proceed with deletion ────────

                for target in &targets {
                    if let Some(j) = jobs.get(&job.id)
                        && j.cancel.load(std::sync::atomic::Ordering::Relaxed)
                    {
                        cancelled = true;
                        break;
                    }

                    let result = if let arx::vfs::Location::S3 { .. } = &location {
                        // S3: one exact DeleteObject per frozen target key.
                        // No prefix recursion, no bucket delete. The key is the
                        // frozen selection path, taken verbatim.
                        registry.delete_s3_at(&location, &target.path).await
                    } else {
                        match target.kind {
                            arx::vfs::EntryKind::Directory => {
                                registry.remove_dir_at(&location, &target.path).await
                            }
                            _ => registry.remove_file_at(&location, &target.path).await,
                        }
                    };

                    match result {
                        Ok(()) => completed += 1,
                        Err(_e) => {
                            failed += 1;
                        }
                    }
                }

                // Refresh pane after any physical mutations
                if completed > 0 || failed > 0 {
                    let _ = loader.load(pane, location, PaneLoadPurpose::Refresh);
                }

                if cancelled {
                    let _ = jobs.publish_event(
                        &tx,
                        arx::jobs::JobEvent::Cancelled {
                            id: job.id,
                            result: arx::jobs::JobResult::generic(
                                format!("Cancelled after {completed} deleted, {failed} failed"),
                                completed,
                            ),
                        },
                    );
                } else if failed > 0 {
                    let _ = jobs.publish_event(
                        &tx,
                        arx::jobs::JobEvent::Failed {
                            id: job.id,
                            error: format!("{completed} deleted, {failed} failed"),
                            result: Some(arx::jobs::JobResult::generic(
                                format!("Partial: {completed} deleted, {failed} failed"),
                                completed,
                            )),
                        },
                    );
                } else {
                    let _ = jobs.publish_event(
                        &tx,
                        arx::jobs::JobEvent::Completed {
                            id: job.id,
                            result: arx::jobs::JobResult::generic(
                                format!("{completed} deleted"),
                                completed,
                            ),
                        },
                    );
                }
            });
            state.message = Some(format!("Remote delete: {target_count} target(s) queued"));
        }
        Action::CancelRemoteDelete => {
            state.pending_delete = None;
            state.pending_webdav_delete = None;
            state.message = Some("Remote delete cancelled".into());
        }
        _ => return false,
    }
    true
}

pub(super) fn submit_mkdir(
    state: &mut AppState,
    loc: Location,
    name: String,
    sync: &SyncUiRuntime,
    pane_loader: &PaneLoader,
) -> bool {
    // Validate child name — reject empty, ".", "..", "/", NUL.
    if let Err(e) = arx::vfs::validate_child_name(&name) {
        state.message = Some(e.to_string());
        return true;
    }
    // S3: direct-bypass guard — target root (bucket=None) MUST NOT schedule prefix creation.
    if let Location::S3 { bucket: None, .. } = &loc {
        state.message = Some("mkdir: bucket creation is not supported".into());
        return true;
    }
    let registry = state.registry.clone();
    let name_for_msg = name.clone();
    let pane = state.active;
    let pane_location = loc.clone();
    let loader = pane_loader.clone();
    let job = sync.jobs.create_job(
        "mkdir",
        arx::jobs::JobKind::RemoteCommand,
        format!("mkdir {name}"),
        Some(loc.clone()),
        None,
    );
    state.jobs = sync.jobs.snapshot();
    let jobs = sync.jobs.clone();
    let tx = sync.job_events.clone();
    {
        let jid = job.id.clone();
        let _ = jobs.publish_event(&sync.job_events, arx::jobs::JobEvent::Running { id: jid });
    }
    // Dispatch based on location type
    if let Location::S3 { .. } = &loc {
        // S3: use create_s3_prefix_marker_at
        tokio::spawn(async move {
            let result = registry.create_s3_prefix_marker_at(&loc, &name).await;
            match result {
                Ok(_) => {
                    let _ = jobs.publish_event(
                        &tx,
                        arx::jobs::JobEvent::Completed {
                            id: job.id,
                            result: arx::jobs::JobResult::generic("created", 1),
                        },
                    );
                    let _ = loader.load(pane, pane_location, PaneLoadPurpose::Refresh);
                }
                Err(e) => {
                    let _ = jobs.publish_event(
                        &tx,
                        arx::jobs::JobEvent::Failed {
                            id: job.id,
                            error: e.to_string(),
                            result: None,
                        },
                    );
                }
            }
        });
    } else {
        // SFTP (and Local, if ever routed here): use mkdir_at
        tokio::spawn(async move {
            let result = registry.mkdir_at(&loc, &name).await;
            match result {
                Ok(()) => {
                    let _ = jobs.publish_event(
                        &tx,
                        arx::jobs::JobEvent::Completed {
                            id: job.id,
                            result: arx::jobs::JobResult::generic("created", 1),
                        },
                    );
                    let _ = loader.load(pane, pane_location, PaneLoadPurpose::Refresh);
                }
                Err(e) => {
                    let _ = jobs.publish_event(
                        &tx,
                        arx::jobs::JobEvent::Failed {
                            id: job.id,
                            error: e.to_string(),
                            result: None,
                        },
                    );
                }
            }
        });
    }
    state.message = Some(format!("mkdir {name_for_msg}…"));
    false
}

pub(super) fn render_confirmation(frame: &mut Frame, area: Rect, state: &AppState) {
    if let Some(plan) = &state.pending_webdav_delete {
        let root_count = plan.sources.len();
        let body = if root_count == 1 {
            format!(
                "PERMANENT WEBDAV TREE DELETE\n\n{}\n\nPermanently delete this WebDAV directory tree?\nNo Trash / Undo  Enter=Confirm  Esc=Cancel",
                plan.presentation_name
            )
        } else {
            let max_show = 10;
            let mut names: Vec<String> = plan
                .presentation_names
                .iter()
                .take(max_show)
                .map(|name| format!("  {name}"))
                .collect();
            if root_count > max_show {
                names.push(format!("  ...and {} more", root_count - max_show));
            }
            format!(
                "PERMANENT WEBDAV TREE DELETE\n\n{root_count} WebDAV directory trees\n{}\n\nPermanently delete {root_count} WebDAV directory trees?\nNo Trash / Undo  Enter=Confirm  Esc=Cancel",
                names.join("\n")
            )
        };
        let height = (body.lines().count() + 2).min(area.height as usize) as u16;
        let popup = centered_rect_lines(60, height.max(9), area);
        frame.render_widget(Clear, popup);
        frame.render_widget(
            ratatui::widgets::Paragraph::new(body).block(
                ratatui::widgets::Block::default()
                    .borders(ratatui::widgets::Borders::ALL)
                    .border_style(Style::default().fg(Color::Red))
                    .title(" Confirm WebDAV Tree Delete "),
            ),
            popup,
        );
        return;
    }
    let Some(plan) = &state.pending_delete else {
        return;
    };

    let file_count = plan
        .targets
        .iter()
        .filter(|t| t.kind == arx::vfs::EntryKind::File)
        .count();
    let symlink_count = plan
        .targets
        .iter()
        .filter(|t| t.kind == arx::vfs::EntryKind::Symlink)
        .count();
    let dir_count = plan
        .targets
        .iter()
        .filter(|t| t.kind == arx::vfs::EntryKind::Directory)
        .count();

    let name_lines: Vec<String> = {
        let max_show = 10;
        let mut names: Vec<String> = plan
            .targets
            .iter()
            .take(max_show)
            .map(|t| format!("  {}", t.name))
            .collect();
        if plan.targets.len() > max_show {
            names.push(format!("  ...and {} more", plan.targets.len() - max_show));
        }
        names
    };

    let breakdown = {
        let mut parts = Vec::new();
        if file_count > 0 {
            parts.push(format!("{file_count} file(s)"));
        }
        if symlink_count > 0 {
            parts.push(format!("{symlink_count} symlink(s)"));
        }
        if dir_count > 0 {
            parts.push(format!("{dir_count} empty dir(s)"));
        }
        if parts.is_empty() {
            "".into()
        } else {
            parts.join(", ")
        }
    };

    let msg = format!(
        "PERMANENT REMOTE DELETE\n\n{} target(s) at {}\n{}\n\nNo Trash / Undo  Enter=Confirm  Esc=Cancel",
        plan.targets.len(),
        plan.location,
        breakdown,
    );

    // Append name lines
    let body = format!("{msg}\n\n{}", name_lines.join("\n"));

    // ponytail: enough room for msg (6 lines) + 2-separator + name_lines + 2-border
    let height = (name_lines.len() + msg.lines().count() + 4).min(area.height as usize) as u16;
    let popup = centered_rect_lines(60, height.max(8), area);
    frame.render_widget(Clear, popup);
    let p = ratatui::widgets::Paragraph::new(body)
        .block(
            ratatui::widgets::Block::default()
                .borders(ratatui::widgets::Borders::ALL)
                .border_style(Style::default().fg(Color::Red))
                .title(" Confirm Remote Delete "),
        )
        .alignment(ratatui::layout::Alignment::Left);
    frame.render_widget(p, popup);
}
