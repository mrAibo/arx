from pathlib import Path

path = Path('src/tui.rs')
text = path.read_text()


def once(old: str, new: str, label: str) -> None:
    global text
    count = text.count(old)
    if count != 1:
        raise SystemExit(f'{label}: expected 1 match, found {count}')
    text = text.replace(old, new, 1)

once(
    'lines.push(Line::from(format!("{error}")));',
    'lines.push(Line::from(error.to_string()));',
    'verification error rendering',
)
once(
    '''    focused: Option<&Entry>,
    _left_entries: &[Entry],
    _right_entries: &[Entry],
    workspace_scanner: &WorkspaceScanner,
''',
    '''    focused: Option<&Entry>,
    workspace_scanner: &WorkspaceScanner,
''',
    'dispatcher signature',
)
once(
    '''    focused: Option<&Entry>,
    left_entries: &[Entry],
    right_entries: &[Entry],
    workspace_scanner: &WorkspaceScanner,
    pane_loader: &PaneLoader,
''',
    '''    focused: Option<&Entry>,
    workspace_scanner: &WorkspaceScanner,
    pane_loader: &PaneLoader,
''',
    'command target signature',
)
once(
    '''                            dispatch_ui_action(
                                &mut state,
                                action,
                                entries.get(cursor).copied(),
                                &left_entries,
                                &right_entries,
                                &workspace_scanner,
                                &sync_runtime,
                            );
''',
    '''                            dispatch_ui_action(
                                &mut state,
                                action,
                                entries.get(cursor).copied(),
                                &workspace_scanner,
                                &sync_runtime,
                            );
''',
    'key router dispatcher call',
)
once(
    '''                                    if let Some(effect) = execute_command_target(
                                        &mut state,
                                        item.target,
                                        focused_entry.copied(),
                                        &left_entries,
                                        &right_entries,
                                        &workspace_scanner,
                                        &pane_loader,
                                        &sync_runtime,
                                    ) {
''',
    '''                                    if let Some(effect) = execute_command_target(
                                        &mut state,
                                        item.target,
                                        focused_entry.copied(),
                                        &workspace_scanner,
                                        &pane_loader,
                                        &sync_runtime,
                                    ) {
''',
    'command center target call',
)
once(
    '''            dispatch_ui_action(
                state,
                action,
                focused,
                left_entries,
                right_entries,
                workspace_scanner,
                sync,
            );
''',
    '''            dispatch_ui_action(state, action, focused, workspace_scanner, sync);
''',
    'command target dispatcher forwarding',
)

path.write_text(text)
