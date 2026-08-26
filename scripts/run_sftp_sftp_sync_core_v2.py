from pathlib import Path

helper = Path("scripts/apply_sftp_sftp_sync_core.py").read_text()
old = '''    if count != 1:\n        raise SystemExit(f"{path}: expected exactly one anchor, found {count}: {old[:120]!r}")\n    file.write_text(text.replace(old, new, 1))\n'''
new = '''    if count < 1:\n        raise SystemExit(f"{path}: expected patch anchor, found none: {old[:120]!r}")\n    file.write_text(text.replace(old, new, 1))\n'''
if helper.count(old) != 1:
    raise SystemExit("helper guard function shape changed unexpectedly")
helper = helper.replace(old, new, 1)

# The only intentionally repeated generic anchors in the baseline compiler.
compiler = Path("src/workspace_sync_executor/compiler.rs").read_text()
if compiler.count("require_local_file_mutation(&target, path)?;") != 2:
    raise SystemExit("unexpected require_local_file_mutation occurrence count")
if compiler.count("require_local_directory_mutation(&target, path)?;") != 3:
    raise SystemExit("unexpected require_local_directory_mutation occurrence count")

exec(compile(helper, "scripts/apply_sftp_sftp_sync_core.py", "exec"))
