from pathlib import Path

path = Path('.github/ux_contracts_migrate.py')
text = path.read_text()

old = 'assert!(sync_ui.contains("controller.launch("));'
new = 'assert!(sync_ui.contains(".launch("));'
if text.count(old) != 2:
    raise SystemExit(f'launch contract count mismatch: {text.count(old)}')
text = text.replace(old, new, 2)

old = '''        "TransferPlanner",
        "SyncConfirmationToken",
        "execute_transfer",
'''
new = '''        "TransferPlanner",
        "MutationService",
        "SyncConfirmationToken",
        "execute_transfer",
'''
if text.count(old) != 2:
    raise SystemExit(f'forbidden contract count mismatch: {text.count(old)}')
text = text.replace(old, new, 2)

path.write_text(text)
