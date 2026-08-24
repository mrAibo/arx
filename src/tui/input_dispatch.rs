use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InputFlow {
    ContinueLoop,
    Proceed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EntryMutation {
    None,
    SwapPaneEntries,
    ResortPaneEntries(SortMode),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct InputDispatchOutcome {
    pub flow: InputFlow,
    pub entry_mutation: EntryMutation,
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_event(
    event: Event,
    state: &mut AppState,
    left_visible: &[VisiblePaneRow<'_>],
    right_visible: &[VisiblePaneRow<'_>],
    left_filtered: &[&Entry],
    right_filtered: &[&Entry],
    workspace_scanner: &WorkspaceScanner,
    sync: &SyncUiRuntime,
    effect_dispatcher: &EffectDispatcher,
    pane_loader: &PaneLoader,
    terminal_session: &mut TuiTerminalSession,
    configured_editor: Option<&str>,
    key_router: &mut KeyRouter,
) -> io::Result<InputDispatchOutcome> {
    let mut entry_mutation = EntryMutation::None;
    if matches!(&event, Event::Key(_) | Event::Mouse(_)) {
        state.dismiss_session_callout();
    }
    match event {
        Event::Key(key) if state.show_terminal && state.active == Pane::Right => {
            embedded_terminal::handle_key(&mut *state, key);
        }
        Event::Mouse(mouse) => {
            match mouse::classify(state, mouse) {
                mouse::MouseRoute::Ignore => {}
                mouse::MouseRoute::CommandBar { action, available } => {
                    // Clear any pending keyboard chord on explicit mouse command
                    key_router.clear_pending();

                    // Derive the SAME active context as keyboard dispatch
                    let entries = if state.active == Pane::Left {
                        &left_filtered
                    } else {
                        &right_filtered
                    };
                    let rows = if state.active == Pane::Left {
                        &left_visible
                    } else {
                        &right_visible
                    };
                    let cursor = {
                        let pane = state.active_pane();
                        if pane.split && pane.split_active {
                            pane.split_cursor
                        } else {
                            pane.cursor
                        }
                    };
                    let focused = rows.get(cursor).copied();
                    let other_focused_row = if state.active == Pane::Left {
                        right_visible.get(state.right.cursor).copied()
                    } else {
                        left_visible.get(state.left.cursor).copied()
                    };
                    let visible_count = entries.len();

                    if available {
                        dispatch_ui_action(
                            &mut *state,
                            action,
                            focused,
                            other_focused_row,
                            entries,
                            visible_count,
                            workspace_scanner,
                            sync,
                            effect_dispatcher,
                            pane_loader,
                            terminal_session,
                            configured_editor,
                            key_router,
                        )
                        .await?;
                    } else {
                        let ctx = ActionContext::from_state(state).with_file_context(
                            focused
                                .and_then(|row| row.action_entry())
                                .map(|entry| entry.kind),
                            configured_editor.is_some(),
                        );
                        let action_id = command_bar::action_to_id(action);
                        let avail = action_availability(action_id, &ctx);
                        state.message = Some(avail.reason().unwrap_or("unavailable").to_string());
                    }
                    return Ok(InputDispatchOutcome {
                        flow: InputFlow::ContinueLoop,
                        entry_mutation: EntryMutation::None,
                    });
                }
                mouse::MouseRoute::ViewerScrollDown => {
                    state.viewer_scroll =
                        (state.viewer_scroll + 1).min(state.viewer_content.len().saturating_sub(1));
                }
                mouse::MouseRoute::ViewerScrollUp => {
                    state.viewer_scroll = state.viewer_scroll.saturating_sub(1);
                }
                mouse::MouseRoute::ContextMenu { column, row } => {
                    state.show_context_menu = !state.show_context_menu;
                    state.context_menu_pos = (column, row);
                }
                mouse::MouseRoute::DragSelect { pane, row } => {
                    let rows = if pane == Pane::Left {
                        &left_visible
                    } else {
                        &right_visible
                    };
                    if let Some(entry) = rows
                        .get(row)
                        .and_then(VisiblePaneRow::listed)
                        .map(|listed| &listed.entry)
                    {
                        let location = if pane == Pane::Left {
                            state.left.location.clone()
                        } else {
                            state.right.location.clone()
                        };
                        if !matches!(location, Location::S3 { .. }) {
                            state.toggle_selection(pane, &location, &entry.name);
                        }
                    }
                }
                mouse::MouseRoute::ActivatePaneRow { pane, row } => {
                    let filt = if pane == Pane::Left {
                        &left_filtered
                    } else {
                        &right_filtered
                    };
                    if row < filt.len() {
                        if pane == Pane::Left {
                            state.left.cursor = row;
                        } else {
                            state.right.cursor = row;
                        }
                        state.active = pane;
                    }
                }
            }
        }
        Event::Key(key) => {
            // Remote delete confirmation intercepts Enter/Escape
            if state.pending_delete.is_some() {
                match key.code {
                    KeyCode::Enter => {
                        dispatch_ui_action(
                            &mut *state,
                            Action::ConfirmRemoteDelete,
                            None, // no focused entry during confirmation
                            None,
                            &[],
                            0,
                            workspace_scanner,
                            sync,
                            effect_dispatcher,
                            pane_loader,
                            terminal_session,
                            configured_editor,
                            key_router,
                        )
                        .await?;
                        return Ok(InputDispatchOutcome {
                            flow: InputFlow::ContinueLoop,
                            entry_mutation: EntryMutation::None,
                        });
                    }
                    KeyCode::Esc => {
                        dispatch_ui_action(
                            &mut *state,
                            Action::CancelRemoteDelete,
                            None,
                            None,
                            &[],
                            0,
                            workspace_scanner,
                            sync,
                            effect_dispatcher,
                            pane_loader,
                            terminal_session,
                            configured_editor,
                            key_router,
                        )
                        .await?;
                        return Ok(InputDispatchOutcome {
                            flow: InputFlow::ContinueLoop,
                            entry_mutation: EntryMutation::None,
                        });
                    }
                    _ => {
                        // Ignore other keys during confirmation
                        return Ok(InputDispatchOutcome {
                            flow: InputFlow::ContinueLoop,
                            entry_mutation: EntryMutation::None,
                        });
                    }
                }
            }
            // Command Center owns keyboard input before generic text-input routing.
            let focused_kind =
                focused_visible_entry(state, left_visible, right_visible).map(|entry| entry.kind);
            match command_center::handle_key(
                &mut *state,
                key,
                focused_kind,
                configured_editor.is_some(),
            ) {
                command_center::KeyOutcome::NotHandled => {}
                command_center::KeyOutcome::Consumed => {
                    return Ok(InputDispatchOutcome {
                        flow: InputFlow::ContinueLoop,
                        entry_mutation: EntryMutation::None,
                    });
                }
                command_center::KeyOutcome::Execute(target) => {
                    let cursor = {
                        let pane = state.active_pane();
                        if pane.split && pane.split_active {
                            pane.split_cursor
                        } else {
                            pane.cursor
                        }
                    };
                    let (focused_row, visible_count) = if state.active == Pane::Left {
                        (left_visible.get(cursor).copied(), left_filtered.len())
                    } else {
                        (right_visible.get(cursor).copied(), right_filtered.len())
                    };
                    let active_entries: Vec<&Entry> = if state.active == Pane::Left {
                        left_visible
                            .iter()
                            .filter_map(VisiblePaneRow::listed_entry)
                            .collect()
                    } else {
                        right_visible
                            .iter()
                            .filter_map(VisiblePaneRow::listed_entry)
                            .collect()
                    };
                    let other_focused_row = if state.active == Pane::Left {
                        right_visible.get(state.right.cursor).copied()
                    } else {
                        left_visible.get(state.left.cursor).copied()
                    };
                    if let Some(effect) = execute_command_target(
                        &mut *state,
                        target,
                        focused_row,
                        other_focused_row,
                        &active_entries,
                        visible_count,
                        workspace_scanner,
                        pane_loader,
                        sync,
                        effect_dispatcher,
                        terminal_session,
                        configured_editor,
                        key_router,
                    )
                    .await?
                    {
                        let id = effect_dispatcher.dispatch(
                            EffectLane::GlobalProcess,
                            EffectScope::Global,
                            effect,
                        );
                        state.register_effect(EffectLane::GlobalProcess, id);
                    }
                    schedule_both_pane_loads(pane_loader, &mut *state);
                    return Ok(InputDispatchOutcome {
                        flow: InputFlow::ContinueLoop,
                        entry_mutation: EntryMutation::None,
                    });
                }
            }

            // If composing filter/glob/go-to, keys go to buffer
            if state.filtering || state.glob_input || state.go_input || state.cmd_input {
                match key.code {
                    KeyCode::Esc => {
                        state.filter.clear();
                        state.filtering = false;
                        state.glob_input = false;
                        state.go_input = false;
                        state.cmd_input = false;
                        state.pending_mkdir_location = None;
                        state.pending_quick_action_prompt = None;
                    }
                    KeyCode::Enter => {
                        if state.glob_input && !state.filter.is_empty() {
                            let active = state.active;
                            let location = state.active_pane().location.clone();
                            if !matches!(location, Location::S3 { .. }) {
                                let rows = if state.active == Pane::Left {
                                    &left_visible
                                } else {
                                    &right_visible
                                };
                                for e in rows.iter().filter_map(VisiblePaneRow::listed) {
                                    if !state.is_selected(active, &location, &e.entry.name) {
                                        state.toggle_selection(active, &location, &e.entry.name);
                                    }
                                }
                                state.message = Some(format!(
                                    "Selected {}",
                                    state.selection_count(active, &location)
                                ));
                            } else {
                                state.message =
                                    Some("Selection by name is not supported for S3".into());
                            }
                            state.filter.clear();
                        } else if state.go_input && !state.filter.is_empty() {
                            // Navigate to typed path
                            let target = PathBuf::from(&state.filter);
                            let resolved = if target.is_absolute() {
                                target
                            } else {
                                match &state.active_pane().location {
                                    Location::Local(p) => p.join(&target),
                                    _ => target,
                                }
                            };
                            let active = state.active;
                            schedule_pane_navigation(
                                pane_loader,
                                &mut *state,
                                active,
                                Location::Local(resolved),
                                PaneLoadPurpose::Navigate {
                                    remember_current: true,
                                },
                            );
                            state.message = Some("Opening path…".into());
                            state.filter.clear();
                        }
                        if state.cmd_input {
                            let command = std::mem::take(&mut state.cmd);
                            let pending_quick_action =
                                std::mem::take(&mut state.pending_quick_action_prompt);
                            let pending_mkdir = std::mem::take(&mut state.pending_mkdir_location);
                            state.cmd_input = false;
                            if command.is_empty() {
                                state.message = Some(": command cancelled".into());
                            } else if let Some(prompt) = pending_quick_action {
                                if quick_actions::submit_prompt(
                                    &mut *state,
                                    prompt,
                                    command,
                                    effect_dispatcher,
                                ) {
                                    return Ok(InputDispatchOutcome {
                                        flow: InputFlow::ContinueLoop,
                                        entry_mutation: EntryMutation::None,
                                    });
                                }
                            } else if let Some(loc) = pending_mkdir {
                                if mutations::submit_mkdir(
                                    &mut *state,
                                    loc,
                                    command,
                                    sync,
                                    pane_loader,
                                ) {
                                    return Ok(InputDispatchOutcome {
                                        flow: InputFlow::ContinueLoop,
                                        entry_mutation: EntryMutation::None,
                                    });
                                }
                            } else {
                                let id = effect_dispatcher.dispatch(
                                    EffectLane::GlobalProcess,
                                    EffectScope::Global,
                                    Effect::RunShellCapture { command },
                                );
                                state.register_effect(EffectLane::GlobalProcess, id);
                                state.message = Some("Command started…".into());
                            }
                        }
                        state.filtering = false;
                        state.glob_input = false;
                        state.go_input = false;
                    }
                    KeyCode::Backspace => {
                        if state.cmd_input {
                            state.cmd.pop();
                        } else {
                            state.filter.pop();
                        }
                    }
                    KeyCode::Char(c) => {
                        if state.cmd_input {
                            state.cmd.push(c);
                        } else {
                            state.filter.push(c);
                            if state.show_command_center {
                                state.command_matches = build_command_items_with_file_context(
                                    &state.filter,
                                    state,
                                    focused_visible_entry(state, left_visible, right_visible)
                                        .map(|entry| entry.kind),
                                    configured_editor.is_some(),
                                );
                            }
                        }
                    }
                    _ => {}
                }
                return Ok(InputDispatchOutcome {
                    flow: InputFlow::ContinueLoop,
                    entry_mutation: EntryMutation::None,
                });
            }

            if viewer::handle_key(&mut *state, key) {
                return Ok(InputDispatchOutcome {
                    flow: InputFlow::ContinueLoop,
                    entry_mutation: EntryMutation::None,
                });
            }

            // Active-overlay input owner (PACK R review fix): distinct from the
            // Alt+U launch route. Once the overlay is up, this consumes keys.
            #[cfg(target_os = "linux")]
            if state.show_storage_inspector {
                arx::storage_inspector_ui::handle_storage_inspector_key(&mut *state, key);
                return Ok(InputDispatchOutcome {
                    flow: InputFlow::ContinueLoop,
                    entry_mutation: EntryMutation::None,
                });
            }

            #[cfg(target_os = "linux")]
            if state.show_filesystems {
                arx::filesystem_usage_ui::handle_filesystems_key(&mut *state, key);
                return Ok(InputDispatchOutcome {
                    flow: InputFlow::ContinueLoop,
                    entry_mutation: EntryMutation::None,
                });
            }

            if bookmarks::handle_key(&mut *state, key, pane_loader) {
                return Ok(InputDispatchOutcome {
                    flow: InputFlow::ContinueLoop,
                    entry_mutation: EntryMutation::None,
                });
            }

            if hotlist::handle_key(&mut *state, key, pane_loader) {
                return Ok(InputDispatchOutcome {
                    flow: InputFlow::ContinueLoop,
                    entry_mutation: EntryMutation::None,
                });
            }

            if hosts::handle_key(&mut *state, key, pane_loader) {
                return Ok(InputDispatchOutcome {
                    flow: InputFlow::ContinueLoop,
                    entry_mutation: EntryMutation::None,
                });
            }

            if ssh_hosts::handle_key(&mut *state, key) {
                return Ok(InputDispatchOutcome {
                    flow: InputFlow::ContinueLoop,
                    entry_mutation: EntryMutation::None,
                });
            }

            let sync_runtime = sync;
            if jobs::handle_key(&mut *state, key, sync_runtime) {
                return Ok(InputDispatchOutcome {
                    flow: InputFlow::ContinueLoop,
                    entry_mutation: EntryMutation::None,
                });
            }

            if state.show_transfer_center {
                arx::transfer_center_ui::handle_transfer_center_key(
                    &mut *state,
                    &sync.transfers,
                    key,
                );
                return Ok(InputDispatchOutcome {
                    flow: InputFlow::ContinueLoop,
                    entry_mutation: EntryMutation::None,
                });
            }

            if user_menu::handle_key(&mut *state, key, effect_dispatcher) {
                return Ok(InputDispatchOutcome {
                    flow: InputFlow::ContinueLoop,
                    entry_mutation: EntryMutation::None,
                });
            }

            let entries = if state.active == Pane::Left {
                &left_filtered
            } else {
                &right_filtered
            };
            let visible_rows = if state.active == Pane::Left {
                &left_visible
            } else {
                &right_visible
            };
            let cursor = {
                let pane = state.active_pane();
                if pane.split && pane.split_active {
                    pane.split_cursor
                } else {
                    pane.cursor
                }
            };

            // Help overlay owns navigation keys when open — intercept BEFORE router
            if help::handle_key(&mut *state, key, key_router) {
                return Ok(InputDispatchOutcome {
                    flow: InputFlow::ContinueLoop,
                    entry_mutation: EntryMutation::None,
                });
            }

            // First migration slice: resolve stable app actions before
            // falling back to the legacy key matcher below.
            let routed_context = state.input_context();
            match key_router.resolve(routed_context, key) {
                KeyResolution::Pending => {
                    return Ok(InputDispatchOutcome {
                        flow: InputFlow::ContinueLoop,
                        entry_mutation: EntryMutation::None,
                    });
                }
                KeyResolution::Action(action) => {
                    let context = ActionContext::from_state(state).with_file_context(
                        visible_rows
                            .get(cursor)
                            .and_then(|row| row.action_entry())
                            .map(|entry| entry.kind),
                        configured_editor.is_some(),
                    );
                    match action_availability(action.id(), &context) {
                        ActionAvailability::Available => {
                            let other_focused_row = if state.active == Pane::Left {
                                right_visible.get(state.right.cursor).copied()
                            } else {
                                left_visible.get(state.left.cursor).copied()
                            };
                            dispatch_ui_action(
                                &mut *state,
                                action,
                                visible_rows.get(cursor).copied(),
                                other_focused_row,
                                entries,
                                entries.len(),
                                workspace_scanner,
                                sync,
                                effect_dispatcher,
                                pane_loader,
                                terminal_session,
                                configured_editor,
                                key_router,
                            )
                            .await?
                        }
                        ActionAvailability::Disabled { reason } => {
                            state.message = Some(reason);
                        }
                        ActionAvailability::Hidden => {}
                    }
                    return Ok(InputDispatchOutcome {
                        flow: InputFlow::ContinueLoop,
                        entry_mutation: EntryMutation::None,
                    });
                }
                KeyResolution::Unhandled => {}
            }

            // #214: sync contexts fail CLOSED — an unhandled (e.g. disabled)
            // key must never leak into the browser fallback classifier.
            if matches!(
                routed_context,
                arx::app::InputContext::SyncPreview
                    | arx::app::InputContext::SyncConfirmation
                    | arx::app::InputContext::SyncJob
            ) {
                return Ok(InputDispatchOutcome {
                    flow: InputFlow::ContinueLoop,
                    entry_mutation: EntryMutation::None,
                });
            }

            match browser_input::classify(state, key) {
                #[cfg(target_os = "linux")]
                browser_input::BrowserRoute::OpenFilesystems => {
                    match arx::filesystem_usage_ui::launch_filesystems(&mut *state) {
                        Ok(()) => {
                            state.message = Some("Filesystems refreshed".into());
                        }
                        Err(message) => {
                            state.message = Some(message);
                        }
                    }
                    return Ok(InputDispatchOutcome {
                        flow: InputFlow::ContinueLoop,
                        entry_mutation: EntryMutation::None,
                    });
                }
                browser_input::BrowserRoute::TreeFilterBackspace => {
                    state.tree_filter.pop();
                    return Ok(InputDispatchOutcome {
                        flow: InputFlow::ContinueLoop,
                        entry_mutation: EntryMutation::None,
                    });
                }
                browser_input::BrowserRoute::SwitchPane => {
                    let pane = state.active_pane_mut();
                    if pane.split {
                        pane.split_active = !pane.split_active;
                    } else {
                        state.apply(Action::SwitchPane);
                    }
                }
                browser_input::BrowserRoute::MoveUp => {
                    let pane = state.active_pane_mut();
                    if pane.split && pane.split_active {
                        if pane.split_cursor > 0 {
                            pane.split_cursor -= 1;
                        }
                    } else if cursor > 0 {
                        pane.cursor -= 1;
                    }
                }
                browser_input::BrowserRoute::MoveDown => {
                    let pane = state.active_pane_mut();
                    if pane.split && pane.split_active {
                        if pane.split_cursor + 1 < entries.len() {
                            pane.split_cursor += 1;
                        }
                    } else if cursor + 1 < entries.len() {
                        pane.cursor += 1;
                    }
                }
                // Ctrl+Space: hash for files, du/df for dirs
                browser_input::BrowserRoute::InspectFocusedEntry => {
                    let pane = state.active_pane();
                    if let Some(entry) = visible_rows
                        .get(cursor)
                        .and_then(VisiblePaneRow::listed_entry)
                    {
                        match entry.kind {
                            EntryKind::Directory => {
                                if let Location::Local(dir) = &pane.location {
                                    let p = dir.join(&entry.name);
                                    state.viewer_content =
                                        FileInfoService::directory_summary(&p).await;
                                    state.viewer_scroll = 0;
                                }
                            }
                            _ => {
                                if let Location::Local(dir) = &pane.location {
                                    let p = dir.join(&entry.name);
                                    let size = entry.size.map(format_size).unwrap_or_default();
                                    state.viewer_content =
                                        FileInfoService::file_hash_summary(&p, &size).await;
                                    state.viewer_scroll = 0;
                                }
                            }
                        }
                    }
                }
                // Alt+/ : recursive file search
                browser_input::BrowserRoute::BeginRecursiveSearch => {
                    if let Location::Local(dir) = &state.active_pane().location {
                        state.cmd = format!("find {} -name ''", dir.display());
                        state.cmd_input = true;
                    }
                }
                browser_input::BrowserRoute::BeginFilter => {
                    state.filter.clear();
                    state.filtering = true;
                }
                browser_input::BrowserRoute::MeasureDirectoryChildren => {
                    let pane = state.active_pane();
                    if let Location::Local(dir) = &pane.location {
                        let location = Location::Local(dir.clone());
                        let id = effect_dispatcher.dispatch(
                            EffectLane::Preview,
                            EffectScope::Location(location),
                            Effect::DirectoryChildrenSizes { path: dir.clone() },
                        );
                        state.register_effect(EffectLane::Preview, id);
                        state.message = Some("Calculating directory sizes…".into());
                    }
                }
                browser_input::BrowserRoute::ActivateEntry => {
                    let pane = state.active_pane();
                    if matches!(visible_rows.get(cursor), Some(VisiblePaneRow::LoadMore(_))) {
                        let active = state.active;
                        schedule_next_page(pane_loader, &mut *state, active);
                        return Ok(InputDispatchOutcome {
                            flow: InputFlow::ContinueLoop,
                            entry_mutation: EntryMutation::None,
                        });
                    }
                    if let Some(row) = visible_rows.get(cursor) {
                        let entry = row.entry();
                        let pane_location = pane.location.clone();
                        if let Some(new_location) =
                            row.navigation_target(&pane_location, &state.registry)
                        {
                            let active = state.active;
                            schedule_pane_navigation(
                                pane_loader,
                                &mut *state,
                                active,
                                new_location,
                                PaneLoadPurpose::Navigate {
                                    remember_current: row.listed().is_some(),
                                },
                            );
                            state.message = Some("Opening directory…".into());
                        } else if is_archive(&entry.name) {
                            // Open archive file
                            if let Location::Local(dir) = &pane_location {
                                let archive_path = dir.join(&entry.name);
                                let target = Location::Archive {
                                    archive: archive_path,
                                    inner_path: String::new(),
                                };
                                let active = state.active;
                                schedule_pane_navigation(
                                    pane_loader,
                                    &mut *state,
                                    active,
                                    target,
                                    PaneLoadPurpose::Navigate {
                                        remember_current: true,
                                    },
                                );
                                state.message = Some("Opening archive…".into());
                            }
                        } else if state.show_diff {
                            // Content diff: diff this file against other pane's same-named file
                            if let (Location::Local(left_dir), Location::Local(right_dir)) =
                                (&state.left.location, &state.right.location)
                            {
                                let left_path = left_dir.join(&entry.name);
                                let right_path = right_dir.join(&entry.name);
                                let scope = EffectScope::Workspace {
                                    left: state.left.location.clone(),
                                    right: state.right.location.clone(),
                                };
                                let id = effect_dispatcher.dispatch(
                                    EffectLane::Preview,
                                    scope,
                                    Effect::UnifiedDiff {
                                        left: left_path,
                                        right: right_path,
                                    },
                                );
                                state.register_effect(EffectLane::Preview, id);
                                state.message = Some("Building diff…".into());
                            }
                        }
                    }
                }
                browser_input::BrowserRoute::OpenParent => {
                    let pane = state.active_pane();
                    let loc = pane.location.clone();
                    if let Some(new_loc) = navigation_parent_target(&loc, &state.registry) {
                        let active = state.active;
                        schedule_pane_navigation(
                            pane_loader,
                            &mut *state,
                            active,
                            new_loc,
                            PaneLoadPurpose::Navigate {
                                remember_current: false,
                            },
                        );
                        state.message = Some("Opening parent…".into());
                    }
                }
                browser_input::BrowserRoute::Refresh => {
                    schedule_both_pane_loads(pane_loader, &mut *state);
                    state.message = Some("Refreshing panes…".into());
                }
                // Ctrl+U: swap panes
                browser_input::BrowserRoute::SwapPanes => {
                    std::mem::swap(&mut state.left, &mut state.right);
                    entry_mutation = EntryMutation::SwapPaneEntries;
                    state.clear_selection();
                    state.remote_workspace.disable();
                    state.show_diff = false;
                    state.message = Some("Swapped".into());
                    schedule_both_pane_loads(pane_loader, &mut *state);
                }
                // Shift+F6: rename file under cursor (local-only)
                browser_input::BrowserRoute::BeginRename => {
                    let pane = state.active_pane();
                    if let Some(entry) = visible_rows
                        .get(cursor)
                        .and_then(VisiblePaneRow::listed)
                        .map(|listed| &listed.entry)
                    {
                        if matches!(pane.location, Location::Local(_)) {
                            state.cmd = format!("mv '{}' ", entry.name);
                            state.cmd_input = true;
                        } else {
                            state.message = Some("Rename is currently local-only".into());
                        }
                    }
                }
                // Ctrl+A: file attributes (permissions/owner)
                browser_input::BrowserRoute::FileAttributes => {
                    let pane = state.active_pane();
                    if let Some(entry) = visible_rows
                        .get(cursor)
                        .and_then(VisiblePaneRow::listed_entry)
                        && let Location::Local(dir) = &pane.location
                    {
                        let p = dir.join(&entry.name);
                        let size = entry.size.map(format_size).unwrap_or_default();
                        state.viewer_content =
                            FileInfoService::metadata_summary(&p, &entry.name, entry.kind, &size)
                                .await
                                .unwrap_or_else(|error| vec![format!("File info failed: {error}")]);
                        state.viewer_scroll = 0;
                    }
                }
                // Ctrl+I: file info (stat)
                browser_input::BrowserRoute::FileInfo => {
                    let pane = state.active_pane();
                    if let Some(entry) = visible_rows
                        .get(cursor)
                        .and_then(VisiblePaneRow::listed_entry)
                        && let Location::Local(dir) = &pane.location
                    {
                        let path = dir.join(&entry.name);
                        let size = entry.size.map(format_size).unwrap_or_default();
                        state.viewer_content = FileInfoService::metadata_summary(
                            &path,
                            &entry.name,
                            entry.kind,
                            &size,
                        )
                        .await
                        .unwrap_or_else(|error| vec![format!("File info failed: {error}")]);
                        state.viewer_scroll = 0;
                    }
                }
                // Alt+O: sync other pane to active pane
                browser_input::BrowserRoute::SyncOtherPane => {
                    let src = state.active_pane().location.clone();
                    let destination_pane = match state.active {
                        Pane::Left => Pane::Right,
                        Pane::Right => Pane::Left,
                    };
                    let dst = state.other_pane_mut();
                    dst.location = src;
                    dst.cursor = 0;
                    state.remote_workspace.disable();
                    state.show_diff = false;
                    state.message = Some("Directory synced".into());
                    schedule_pane_load(pane_loader, &mut *state, destination_pane);
                }
                // Alt+Down: go back in directory history
                browser_input::BrowserRoute::HistoryBack => {
                    let pane = state.active_pane_mut();
                    if let Some(prev) = pane.dir_history.last().cloned() {
                        let active = state.active;
                        schedule_pane_navigation(
                            pane_loader,
                            &mut *state,
                            active,
                            prev,
                            PaneLoadPurpose::HistoryBack,
                        );
                        state.message = Some("History back…".into());
                    }
                }
                // Ctrl+Shift+Left/Right: resize panel ratio
                browser_input::BrowserRoute::ResizePanelLeft => {
                    state.panel_ratio = state.panel_ratio.saturating_sub(5).max(10);
                    state.message = Some(format!(
                        "Panel: {}/{}",
                        state.panel_ratio,
                        100 - state.panel_ratio
                    ));
                }
                browser_input::BrowserRoute::ResizePanelRight => {
                    state.panel_ratio = (state.panel_ratio + 5).min(90);
                    state.message = Some(format!(
                        "Panel: {}/{}",
                        state.panel_ratio,
                        100 - state.panel_ratio
                    ));
                }
                // Alt+`: tab switcher
                browser_input::BrowserRoute::ToggleTabSwitcher => {
                    state.show_tab_switcher = !state.show_tab_switcher;
                    state.tab_switcher_cursor = 0;
                }
                // Alt+H: directory history
                browser_input::BrowserRoute::ToggleHistory => {
                    state.show_history = !state.show_history;
                }
                // Ctrl+O: drop to subshell, restore on exit
                browser_input::BrowserRoute::OpenSubshell => {
                    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
                    let shell_result = terminal_session
                        .suspend_while(|| DesktopService::run_interactive_shell(&shell))
                        .await?;
                    if let Err(error) = shell_result {
                        state.message = Some(format!("Shell failed: {error}"));
                    }
                }
                browser_input::BrowserRoute::BeginGoTo => {
                    state.filter.clear();
                    state.go_input = true;
                }
                browser_input::BrowserRoute::ToggleHidden => {
                    state.show_hidden = !state.show_hidden;
                    state.message = Some(if state.show_hidden {
                        "Hidden files shown".into()
                    } else {
                        "Hidden files hidden".into()
                    });
                    schedule_both_pane_loads(pane_loader, &mut *state);
                }
                // *: invert selection on visible entries (local/SFTP/archive only)
                browser_input::BrowserRoute::InvertSelection => {
                    let active = state.active;
                    let location = state.active_pane().location.clone();
                    if !matches!(location, Location::S3 { .. }) {
                        let rows = if state.active == Pane::Left {
                            &left_visible
                        } else {
                            &right_visible
                        };
                        for e in rows.iter().filter_map(VisiblePaneRow::listed) {
                            state.toggle_selection(active, &location, &e.entry.name);
                        }
                        state.message = Some(format!(
                            "Selected {}",
                            state.selection_count(active, &location)
                        ));
                    } else {
                        state.message = Some("Selection by name is not supported for S3".into());
                    }
                }
                // +: enter glob-select mode (uses filter buffer)
                browser_input::BrowserRoute::BeginGlob => {
                    state.filter.clear();
                    state.glob_input = true;
                }
                // F2: user menu (if loaded), otherwise cycle sort
                browser_input::BrowserRoute::UserMenuOrSort => {
                    if !state.menu.is_empty() {
                        state.show_menu = !state.show_menu;
                        state.menu_cursor = 0;
                    } else {
                        state.sort_mode = state.sort_mode.next();
                        state.message = Some(format!("Sort: {}", state.sort_mode.label()));
                        entry_mutation = EntryMutation::ResortPaneEntries(state.sort_mode);
                    }
                }
                // Shift+F3: page file with bat
                browser_input::BrowserRoute::PageWithBat => {
                    let pane = state.active_pane();
                    if let Some(entry) = visible_rows
                        .get(cursor)
                        .and_then(VisiblePaneRow::listed_entry)
                        && entry.kind != EntryKind::Directory
                    {
                        let path = match &pane.location {
                            Location::Local(dir) => dir.join(&entry.name),
                            _ => {
                                return Ok(InputDispatchOutcome {
                                    flow: InputFlow::ContinueLoop,
                                    entry_mutation: EntryMutation::None,
                                });
                            }
                        };
                        let _ = DesktopService::page_with_bat(&path).await;
                    }
                }
                // Ctrl+C: copy filename to clipboard
                browser_input::BrowserRoute::CopyPathToClipboard => {
                    let pane = state.active_pane();
                    if let Some(entry) = visible_rows
                        .get(cursor)
                        .and_then(VisiblePaneRow::listed_entry)
                    {
                        let name = &entry.name;
                        if let Location::Local(dir) = &pane.location {
                            let full = dir.join(name);
                            let path = full.to_string_lossy().into_owned();
                            if let Err(error) = DesktopService::copy_to_clipboard(&path).await {
                                state.message = Some(format!("Clipboard failed: {error}"));
                                return Ok(InputDispatchOutcome {
                                    flow: InputFlow::ContinueLoop,
                                    entry_mutation: EntryMutation::None,
                                });
                            }
                        }
                        state.message = Some(format!("Copied: {name}"));
                    }
                }
                // Ctrl+S: save workspace
                browser_input::BrowserRoute::SaveWorkspace => {
                    match crate::workspace::save_workspace(state) {
                        Ok(()) => state.message = Some("Workspace saved".into()),
                        Err(e) => state.message = Some(format!("Save failed: {e}")),
                    }
                }
                // Ctrl+Y: toggle Transfer Center
                browser_input::BrowserRoute::ToggleTransferCenter => {
                    state.toggle_overlay(OverlayKind::TransferCenter);
                }
                // Type in tree filter (when tree is shown) — Esc to close
                browser_input::BrowserRoute::TreeClose => {
                    state.show_tree = false;
                    state.tree_filter.clear();
                }
                browser_input::BrowserRoute::CloseInfrastructure => {
                    state.close_overlay(OverlayKind::Infrastructure);
                }
                browser_input::BrowserRoute::TreeFilterChar(c) => {
                    state.tree_filter.push(c);
                    let location = state.active_pane().location.clone();
                    let id = effect_dispatcher.dispatch(
                        EffectLane::Tree,
                        EffectScope::Location(location.clone()),
                        Effect::TreeSnapshot {
                            location,
                            filter: state.tree_filter.clone(),
                        },
                    );
                    state.register_effect(EffectLane::Tree, id);
                }
                // Ctrl+X D: toggle directory compare
                // Alt+T: toggle panel mode (Full ↔ Brief)
                browser_input::BrowserRoute::TogglePanelMode => {
                    state.panel_mode = match state.panel_mode {
                        PanelMode::Full => PanelMode::Brief,
                        PanelMode::Brief => PanelMode::Full,
                    };
                }
                // :: command input
                browser_input::BrowserRoute::BeginCommand => {
                    state.pending_mkdir_location = None;
                    state.pending_quick_action_prompt = None;
                    state.cmd.clear();
                    state.cmd_input = true;
                }
                // Alt+1-9: switch to tab N
                browser_input::BrowserRoute::SwitchTabNumber(idx) => {
                    let pane = state.active_pane_mut();
                    if idx < pane.tabs.len() + 1 {
                        if idx != 0 {
                            // ponytail: swap current tab (implicit idx 0) with target tab
                            // Current is at position 1..N; saved tabs are at 0..N-1; total N+1 entries.
                            // To go to tab N: if N==0 (current), no-op; else swap current with saved[idx-1]
                            pane.switch_tab(idx - 1);
                        }
                        let n = pane.tabs.len() + 1;
                        state.message = Some(format!("Tab {}/{n}", idx + 1));
                        state.clear_selection();
                        state.remote_workspace.disable();
                        state.show_diff = false;
                        schedule_active_pane_load(pane_loader, &mut *state);
                    }
                }
                // Ctrl+T: new tab in active pane
                browser_input::BrowserRoute::NewTab => {
                    state.active_pane_mut().new_tab();
                    let tabs = state.active_pane().tabs.len() + 1;
                    state.clear_selection();
                    state.remote_workspace.disable();
                    state.show_diff = false;
                    schedule_active_pane_load(pane_loader, &mut *state);
                    state.message = Some(format!("Tab {tabs}/{tabs}"));
                }
                // Ctrl+W: close tab in active pane
                browser_input::BrowserRoute::CloseTab => {
                    state.active_pane_mut().close_tab();
                    let tabs = state.active_pane().tabs.len() + 1;
                    state.clear_selection();
                    state.remote_workspace.disable();
                    state.show_diff = false;
                    schedule_active_pane_load(pane_loader, &mut *state);
                    state.message = Some(format!("Tab {}/{}", tabs.min(1), tabs));
                }
                // Ctrl+Left: previous tab
                browser_input::BrowserRoute::PreviousTab => {
                    let tabs_len = state.active_pane().tabs.len();
                    if tabs_len > 0 {
                        state.active_pane_mut().switch_tab(tabs_len - 1);
                        state.clear_selection();
                        state.remote_workspace.disable();
                        state.show_diff = false;
                        schedule_active_pane_load(pane_loader, &mut *state);
                        state.message = Some("Tab ←".into());
                    }
                }
                // Ctrl+Right: next tab
                browser_input::BrowserRoute::NextTab => {
                    state.active_pane_mut().switch_tab(0);
                    state.clear_selection();
                    state.remote_workspace.disable();
                    state.show_diff = false;
                    schedule_active_pane_load(pane_loader, &mut *state);
                    state.message = Some("Tab →".into());
                }
                browser_input::BrowserRoute::Unhandled => {}
            }
        }
        _ => {}
    }
    Ok(InputDispatchOutcome {
        flow: InputFlow::Proceed,
        entry_mutation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_outcomes_cover_flow_and_entry_mutations() {
        let outcomes = [
            InputDispatchOutcome {
                flow: InputFlow::ContinueLoop,
                entry_mutation: EntryMutation::None,
            },
            InputDispatchOutcome {
                flow: InputFlow::Proceed,
                entry_mutation: EntryMutation::SwapPaneEntries,
            },
            InputDispatchOutcome {
                flow: InputFlow::Proceed,
                entry_mutation: EntryMutation::ResortPaneEntries(SortMode::NameDesc),
            },
        ];
        assert_eq!(outcomes[0].flow, InputFlow::ContinueLoop);
        assert_eq!(outcomes[1].flow, InputFlow::Proceed);
        assert_eq!(outcomes[1].entry_mutation, EntryMutation::SwapPaneEntries);
        assert_eq!(
            outcomes[2].entry_mutation,
            EntryMutation::ResortPaneEntries(SortMode::NameDesc)
        );
    }

    #[test]
    fn input_dispatch_owns_no_event_source() {
        let production = include_str!("input_dispatch.rs")
            .split_once("#[cfg(test)]")
            .expect("production/test seam")
            .0;
        assert!(!production.contains(".recv()"));
        assert!(!production.contains("tokio::select!"));
    }

    #[test]
    fn event_loop_has_no_input_routing_matches() {
        let source = include_str!("../tui.rs");
        let event_loop = source
            .split_once("async fn event_loop(")
            .expect("event_loop start")
            .1
            .split_once("fn normalize_entries(")
            .expect("event_loop end")
            .0;
        for forbidden in [
            "MouseRoute::",
            "BrowserRoute::",
            "KeyResolution::",
            "match key.code",
            "match mouse",
        ] {
            assert!(
                !event_loop.contains(forbidden),
                "event_loop contains {forbidden}"
            );
        }
    }
}
