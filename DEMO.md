# ARX Killer Demo Storyboard

Recording plan for the hero GIF used by the README.

Goal: make one product idea obvious in 20–30 seconds.

> ARX compares two workspaces, previews the exact consequences, executes a frozen plan in the background, and verifies the result afterwards.

## Demo — Safe Update

Use **Update**, not Mirror. The first public impression demonstrates the Remote Workspace model without starting with destructive confirmation.

### Recording target

- duration: 20–30 seconds
- one terminal window, no desktop chrome
- ~100–120 columns so both panes and overlays are readable
- stable font size large enough for GitHub README playback
- no production credentials, IPs, usernames, tokens, or real customer paths
- disposable OpenSSH/SFTP target
- no artificial success state — final "verified" frame must come from a real `Synchronized` verdict

A localhost disposable OpenSSH/SFTP environment is preferable for reproducibility.

### Fixture

Two workspace roots with a small, deterministic non-destructive difference set.

```
local:  ~/arx-demo/app
remote: arx-demo:/tmp/arx-demo/app
```

Use regular files. Directory and symlink equality is intentionally conservative where the current fingerprint model lacks comparable evidence.

### Required preconditions

- fixture local and remote roots correct
- 0 `.arx-bak-*` artifacts before start
- source-only update: ~5–10 real copies, 0 deletes

### Storyboard

| Second | What |
|--------|------|
| 0–3 | Local ↔ Remote both panes visible with provider badges `[LOCAL]` / `[SSH]` |
| 3–7 | Workspace Ribbon shows `Not compared · Ctrl+D Compare` |
| 7–12 | Ctrl+D — Compare runs, Ribbon shows `Scanning...` |
| 12–17 | Preview: 3 matches, 1 diff, 0 conflicts |
| 17–22 | Execute UPDATE — real rsync with progress |
| 22–27 | Verifying phase, then `Synchronized` verdict |
| 27–30 | Ribbon summary: workspace healthy, badges visible |

### Gate script

If `arx-hero-capture-gate-v6.sh` exists, audit it against current main. Update only: branch/head assertions, shortcut expectations (Ctrl+X P not Ctrl+Shift+S), and current visible labels. Do not weaken its rsync/verification checks.

Run dry-run first. Required:

- fixture local/remote correct
- 0 `.arx-bak-*` before
- Compare works
- Preview shows UPDATE
- Real source-only copies (~5–10), 0 deletes
- Real rsync processes
- Visible progress
- Separate Verifying phase
- Final Synchronized
- 0 `.arx-bak-*` after

### Asset

Store the captured GIF at `docs/assets/remote-workspace-update.gif`. README references the GIF only after the file exists.
