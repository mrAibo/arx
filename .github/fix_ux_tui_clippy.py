from pathlib import Path

path = Path('.github/ux_tui_migrate.py')
text = path.read_text()

replacements = [
    ('lines.push(Line::from(format!("{error}")));', 'lines.push(Line::from(error.to_string()));', 'error rendering'),
    (
        '''    focused: Option<&Entry>,\n    _left_entries: &[Entry],\n    _right_entries: &[Entry],\n    workspace_scanner: &WorkspaceScanner,''',
        '''    focused: Option<&Entry>,\n    workspace_scanner: &WorkspaceScanner,''',
        'dispatcher unused entry slices',
    ),
    (
        '''    focused: Option<&Entry>,\n    left_entries: &[Entry],\n    right_entries: &[Entry],\n    workspace_scanner: &WorkspaceScanner,\n    pane_loader: &PaneLoader,''',
        '''    focused: Option<&Entry>,\n    workspace_scanner: &WorkspaceScanner,\n    pane_loader: &PaneLoader,''',
        'command target entry slices',
    ),
    (
        '''                focused,\n                left_entries,\n                right_entries,\n                workspace_scanner,\n                sync,''',
        '''                focused,\n                workspace_scanner,\n                sync,''',
        'command target dispatcher forwarding',
    ),
    (
        '''                                entries.get(cursor).copied(),\n                                &left_entries,\n                                &right_entries,\n                                &workspace_scanner,\n                                &sync_runtime,''',
        '''                                entries.get(cursor).copied(),\n                                &workspace_scanner,\n                                &sync_runtime,''',
        'key-router dispatcher call',
    ),
    (
        '''                                        focused_entry.copied(),\n                                        &left_entries,\n                                        &right_entries,\n                                        &workspace_scanner,\n                                        &pane_loader,\n                                        &sync_runtime,''',
        '''                                        focused_entry.copied(),\n                                        &workspace_scanner,\n                                        &pane_loader,\n                                        &sync_runtime,''',
        'command-center target call',
    ),
]

for old, new, label in replacements:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f'{label}: expected 1 match, found {count}')
    text = text.replace(old, new, 1)

path.write_text(text)
