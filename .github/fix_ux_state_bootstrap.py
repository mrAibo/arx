from pathlib import Path

path = Path('.github/ux_state_migrate.py')
text = path.read_text()
old = '''for enum_name in ('ActionId', 'Action'):
    old = '    CloseWorkspaceSyncPreview,\\n}'
    new = ''' + "'''" + '''    CloseWorkspaceSyncPreview,
    ExecuteWorkspaceSync,
    ConfirmWorkspaceSync,
    CancelWorkspaceSync,
    ShowWorkspaceSyncDetails,
    ReturnToWorkspaceSyncPreview,
}''' + "'''" + '''
    # first replacement hits ActionId, second hits Action
    text = replace_once(text, old, new, f'{enum_name} sync action variants')
'''
new = '''old = '    CloseWorkspaceSyncPreview,\\n}'
new = ''' + "'''" + '''    CloseWorkspaceSyncPreview,
    ExecuteWorkspaceSync,
    ConfirmWorkspaceSync,
    CancelWorkspaceSync,
    ShowWorkspaceSyncDetails,
    ReturnToWorkspaceSyncPreview,
}''' + "'''" + '''
if text.count(old) != 2:
    raise SystemExit(f'sync action enum variants: expected 2 matches, found {text.count(old)}')
text = text.replace(old, new, 2)
'''
if text.count(old) != 1:
    raise SystemExit('UX state bootstrap patch mismatch')
path.write_text(text.replace(old, new, 1))
