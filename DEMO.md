# ARX Killer Demo Storyboard

This is the recording plan for the hero GIF/video used by the README.

The goal is not to show every feature. The goal is to make one product idea obvious in 20–30 seconds:

> ARX compares two workspaces, previews the exact consequences, executes a frozen plan in the background, and verifies the result afterwards.

## Demo 1 — Safe Update

This is the hero demo for #38.

Use **Update**, not Mirror. The first public impression should demonstrate the Remote Workspace model without starting with destructive confirmation.

### Recording target

- duration: 20–30 seconds;
- one terminal window, no desktop chrome if possible;
- approximately 100–120 columns so both panes and overlays remain readable;
- stable font size large enough for GitHub README playback;
- no production credentials, IP addresses, usernames, tokens, or real customer paths;
- use a disposable/test OpenSSH/SFTP target;
- no mouse unless it materially clarifies the flow;
- no artificial success state: the final “verified” frame must come from a real `Synchronized` verification verdict.

A localhost disposable OpenSSH/SFTP environment is preferable because it makes the recording reproducible and removes network variance.

### Fixture

Prepare two workspace roots with a small, deterministic non-destructive difference set.

Suggested shape:

```text
local:  ~/arx-demo/app
remote: demo-prod:/srv/arx-demo/app
```

For the hero capture, keep the fixture to **regular files**. PR #39 proves canonical mtime evidence for Local/SFTP regular files; directory and symlink equality is still intentionally conservative where the current fingerprint model lacks enough comparable evidence. Do not design the first GIF around metadata cases that the verifier cannot yet prove equal.

For the planned changes, prefer **source-only files** that do not yet exist at the destination. Optional baseline files may already exist on both sides only when they are genuinely identical. Avoid overwriting an existing remote file in the hero fixture: the current rsync executor deliberately uses `--backup` with an `.arx-bak-*` suffix, so an overwrite can leave a truthful destination-only backup entry that prevents the final workspace from verifying as `Synchronized`.

Aim for a visually interesting but readable plan — roughly 5–10 planned copies, zero deletes, and a moderate transfer size. This still demonstrates Update mode and real transfer progress without making the final verification dependent on cleanup outside ARX.

The recording must use the already-proven **rsync archive path** for Local → remote execution. Ensure rsync is available on both sides and confirm ARX selects that path. Workspace physical file transfers are planned as copy operations, for which the transfer planner prefers rsync but can use the SFTP copy path when rsync is unavailable. The #39 acceptance smoke demonstrated that `rsync -a` preserves the canonical file mtimes needed for a truthful post-sync `Synchronized` verdict. If the dry run does not use rsync, do not record the hero GIF.

Do not hardcode “7 changes / 18 MB” into documentation before recording. The GIF should display whatever the real current ARX plan truth reports for the prepared fixture.

### Pre-capture dry-run gates

Before starting the recorder, prove that the fixture itself is compatible with the product-truth requirements.

1. **No backup artifacts before ARX.** After fixture setup, the remote workspace must contain no `.arx-bak-*` files:

```bash
ssh "$ALIAS" \
  "find '$REMOTE' -name '.arx-bak-*' -print"
```

The output must be empty.

2. **Baseline metadata matches independently of ARX.** For any files intentionally present on both sides before the demo, compare size and whole-second modification time:

```bash
stat -c '%n %s %Y' \
  "$SRC/README.txt" \
  "$SRC/project.conf"

ssh "$ALIAS" \
  "stat -c '%n %s %Y' \
    '$REMOTE/README.txt' \
    '$REMOTE/project.conf'"
```

The matching baseline files must report the same size and Unix-second mtime on both sides. If they do not, fix the fixture before launching ARX.

3. **Recording hygiene.** Before capture, use a neutral terminal title/prompt and a disposable alias such as `demo-prod`. Do not show real usernames, IP addresses, production hostnames, credentials, customer paths, or identifying infrastructure. This changes only the recording environment, not ARX state.

Run ARX once without a recorder and accept the dry run only if the visible product path is:

```text
Compare
  ↓
5–10 planned copies
0 deletes
  ↓
Preview UPDATE
  ↓
Execute
  ↓
visible real progress
  ↓
Execution completed
  ↓
Verifying current workspace…
  ↓
✓ VERIFIED
```

If execution tracing is used to prove transport selection, the trace must contain a real `rsync` invocation. After the dry run, repeat the remote backup-artifact check above; it must still produce no output. Any `Inconclusive`, `DifferencesRemain`, unexpected warning/error, wrong route, Mirror flow, or non-rsync transfer path rejects the capture fixture.

### Shot list

#### 0–3 s — Establish the workspace

Show ARX already open with the two roots visible:

```text
~/arx-demo/app      |      demo-prod:/srv/arx-demo/app
```

Pause briefly so the viewer can understand “local on the left, remote on the right”.

#### 3–7 s — Compare

Press the runtime binding for **Compare panes** (default: `Ctrl+D`).

Let the recursive scan finish and keep the first-success compare callout visible long enough to read.

Desired visual truth:

```text
✓ Workspace compared · N changes found · … planned
```

If the fixture unexpectedly produces unproven equality/conflicts, fix the fixture rather than editing the recording around the result.

#### 7–12 s — Preview

Open **Preview workspace sync** (default: `Ctrl+Shift+S`).

Hold on the preview long enough to show the hierarchy:

```text
ROUTE
PLAN
SAFETY
```

The plan should remain in **Update** mode and should contain no delete operations.

The important message is that the viewer can see the consequences before execution.

#### 12–17 s — Execute

Press `Enter` to freeze and launch the safe Update plan.

Show the stage transition into the JobManager-backed execution view. If the fixture is too small and execution is visually instantaneous, increase the transfer payload just enough to make progress observable without making the GIF slow.

Capture real progress such as physical steps and/or transferred bytes as provided by the current executor. Do not overlay a fake percentage or ETA.

#### 17–22 s — Verification

After execution completes, keep the overlay open while ARX enters post-sync verification.

The viewer should see that verification is a separate phase rather than an optimistic completion message.

#### 22–27 s — Verified result

End on the real finished overlay with:

```text
POST-SYNC VERIFICATION
✓ VERIFIED
```

If this is the first verified sync in the ARX process, also capture the session milestone section:

```text
FIRST SUCCESS THIS SESSION
✓ Remote Workspace workflow completed end-to-end.
```

Hold the final frame for roughly two seconds before the GIF loops.

## Story arc

The finished demo should read without narration:

```text
Local ↔ Remote
      ↓
Compare
      ↓
Preview
      ↓
Execute
      ↓
Background progress
      ↓
Verify
      ↓
✓ Synchronized
```

If a viewer only watches once, they should understand that ARX is not merely copying files between panes.

## What not to show in the hero demo

Do not dilute the first recording with:

- Mirror mode;
- delete confirmation;
- Command Center browsing;
- tabs;
- archive browsing;
- tmux;
- plugins;
- a long feature tour;
- synthetic benchmark numbers;
- a production host.

Those can become secondary demos later.

## Demo 2 — Destructive Mirror safety

Record this only after the Update hero demo exists.

Target duration: 15–20 seconds.

Story:

```text
Compare
  ↓
Preview Mirror
  ↓
planned DELETE entries visible
  ↓
Enter
  ↓
explicit confirmation of exact frozen plan
  ↓
execute
  ↓
post-sync verification
```

This second demo should emphasize that destructive intent is visible and separately confirmed. It should not imply that Mirror is the default mode.

## Capture acceptance checklist

The hero asset is ready when all of these are true:

- [ ] real ARX build from the #38 branch after it includes the #39 evidence fix from `main`;
- [ ] disposable/test local + SFTP roots;
- [ ] fixture uses regular files for the proven hero evidence path;
- [ ] planned changes are source-only copies; no overwrite-created `.arx-bak-*` files remain;
- [ ] remote `.arx-bak-*` scan is empty both before ARX and after the dry run;
- [ ] any pre-existing baseline files independently match on size + whole-second mtime;
- [ ] ARX selects the rsync archive execution path for the hero Update;
- [ ] dry-run transport evidence confirms a real rsync invocation when tracing is used;
- [ ] no secrets or identifying infrastructure details;
- [ ] neutral/disposable recording hostname, paths, title, and prompt are used;
- [ ] Update mode, no planned deletes;
- [ ] compare is visibly separate from preview;
- [ ] preview is visibly separate from execution;
- [ ] execution is shown as a background JobManager lifecycle;
- [ ] verification is visibly separate from execution completion;
- [ ] final verdict is a real `Synchronized` result;
- [ ] no hardcoded shortcut text added to the recording itself;
- [ ] 20–30 seconds total;
- [ ] readable at GitHub README width;
- [ ] loop transition is not distracting;
- [ ] master capture is preserved before GIF optimization;
- [ ] README references the final asset only after it exists in the repository.

## Recommended asset layout

When the real recording exists, keep public demo assets under a predictable path such as:

```text
docs/assets/
└── remote-workspace-update.gif
```

Preserve the unoptimized master capture separately while producing a README-sized derivative. The README version is accepted only when `Sync Preview`, the verification transition, and `✓ VERIFIED` remain readable at normal GitHub README width without opening the image separately.

Then place the hero GIF directly below the README positioning/diagram so the first screen tells the same story in text and motion.
