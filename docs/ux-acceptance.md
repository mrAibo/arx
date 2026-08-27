# ARX UX Acceptance

This is a repeatable human-operability gate. Verify both the rendered state and the filesystem/provider state; “no crash” is not a pass.

## Setup

```bash
python3 scripts/setup_ux_acceptance.py
cargo build --locked --all-features
cd /tmp/arx-ux-acceptance/source
/path/to/arx/target/debug/arx
```

Record: tested SHA, OS, terminal/emulator, `$TERM`, dimensions, keyboard layout, and whether PuTTY itself was physically used. Start at 160×45, then repeat at 100×28 and at least 200 columns wide.

For every action record `PASS`, `FAIL`, `UX ISSUE`, or `NOT_RUN`, plus visible result and physical result. Never upgrade `NOT_RUN` to `PASS`.

## Generic truth checks

For each exposed operation answer:

- visible, enabled, executable;
- keyboard, command-bar mouse, and context-menu behavior;
- focused row/pane equals the operational target;
- success is visible and physical;
- disabled state has a specific, readable reason;
- Command Center, command bar, keyboard, and context menu agree.

Right-click a row in each provider. Execute every enabled item. Click at least one disabled item and verify the menu remains fail-closed while the reason stays visible. Click the command bar equivalents. Resize and repeat one operation. Test `..`, single selection, multi-selection, spaces, Unicode, empty files, and nested directories.

## Local control journey

1. Put the source pane at `/tmp/arx-ux-acceptance/source` and passive pane at `destination`.
2. F3 `README.md`; verify exact text, then close viewer.
3. F4 only when a disposable editor is configured; verify the intended file is targeted.
4. F5 `README.md`; verify pane refresh and physical destination contents.
5. Repeat Copy with an existing destination; verify the documented conservative overwrite/noclobber behavior.
6. Exercise F6/F7/F8 only inside the disposable fixture and verify physical state.
7. Select multiple files, copy, and verify exact selected set.

## Archive journey

Repeat for `fixture.tar`, `fixture.tar.gz`, and `fixture.zip`.

1. Open archive; navigate `..` and `nested`.
2. F3 `README.md`; verify exact fixture text and bounded/truncation presentation for an oversized fixture when supplied.
3. F4; verify: `Archive editing is not supported; extract the file first`.
4. F5 `README.md` to the empty Local pane; verify physical bytes.
5. Repeat with `file with spaces.txt` and `Юникод.txt`.
6. Repeat onto an existing destination; verify noclobber and unchanged original.
7. Verify F6/F7/F8 remain disabled with specific reasons.
8. Right-click a regular member and execute each enabled item; disabled items must explain themselves.
9. Use command-bar mouse clicks for View and Copy.

Security fixtures belong in deterministic integration tests: absolute paths, `..`, symlinks, devices, duplicate members, and member-count/depth/byte bounds. Manual acceptance must never extract an untrusted archive.

## Split panes and embedded terminal

- Open vertical and horizontal splits; verify Tab/focus and clicks target the highlighted subview.
- Right-click secondary rows and verify the frozen target does not change when the cursor moves.
- Open/close embedded terminal; verify Commander input resumes and pane state survives.
- Resize with viewer, context menu, split, and terminal open; verify no stale hitboxes or wrong-target actions.

## Remote-provider follow-up matrix

Provision each provider only with its existing project acceptance scripts and disposable credentials. Record `NOT_RUN` otherwise.

- **SFTP:** bounded F3; Local↔SFTP F5; unavailable destructive operations; keyboard/mouse parity.
- **S3:** exact object identity F3/F5; prefixes/buckets; noclobber; no display-name reconstruction.
- **WebDAV:** bounded F3; Local↔WebDAV F5; exact href identity; ambiguous mutation remains failure.

Provider-specific physical suites remain authoritative for protocol safety; this UX suite supplements them with discoverability, feedback, focus, mouse, resize, and filesystem/provider observation.

## Evidence table

| SHA | Journey/action | Terminal/size | Input | Visible result | Physical result | Verdict | Evidence |
|---|---|---|---|---|---|---|---|
| | | | keyboard/mouse/right-click | | | | screenshot/log/path |
