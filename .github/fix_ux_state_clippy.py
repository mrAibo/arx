from pathlib import Path

path = Path('.github/ux_state_migrate.py')
text = path.read_text()
old = '''        let mut state = RemoteWorkspaceState::default();
        state.preview_open = true;
'''
new = '''        let mut state = RemoteWorkspaceState {
            preview_open: true,
            ..RemoteWorkspaceState::default()
        };
'''
if text.count(old) != 1:
    raise SystemExit(f'UX state Clippy patch mismatch: {text.count(old)}')
path.write_text(text.replace(old, new, 1))
