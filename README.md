# ARX

**Terminal commander for local ↔ remote workspaces.**

Compare before touching anything. Preview the exact sync consequences. Run the frozen plan as a background job. Verify the workspace afterwards.

```text
Local project                         remote workspace
~/code/app                 ↔         prod:/srv/app
      │                                  │
      └──────────── Compare ──────────────┘
                         ↓
                    Sync Preview
                         ↓
                  Confirm when needed
                         ↓
                   Background sync
                         ↓
                      Verify
```

<!-- The real 20–30 second Remote Workspace GIF will be inserted here before #38 leaves Draft. -->

ARX is also a fast keyboard-driven dual-pane file manager for local files, SFTP locations, archives, background transfers, tabs, previews, shell workflows, and tmux. Its defining workflow, however, is **Remote Workspace**: treat two locations as one operational pair and make synchronization observable from comparison through post-execution verification.

## Why ARX?

- **Local ↔ remote as one workspace.** Put a project on one side and an SFTP location on the other, then compare the recursive trees directly.
- **Compare before execution.** ARX does not turn a compare into a mutation. The current workspace diff is an explicit fact of its own.
- **Preview exact consequences.** Sync Preview shows direction, update/mirror mode, planned copies, deletes, conflicts, and transfer size before execution.
- **Safe default.** Update mode preserves destination-only entries. Mirror is distinct and requires explicit confirmation when the frozen plan is destructive.
- **Truthful background jobs.** Queue, running, cancelling, cancelled, failed, completed, and verification stages remain separate states instead of being flattened into one optimistic progress message.
- **Verification after execution.** A completed job does not automatically mean the two roots are synchronized. ARX rescans both roots and reports `Synchronized`, `DifferencesRemain`, or `Inconclusive` from the available evidence.
- **Progressive discovery.** Command Center, contextual hints, and the footer derive actions and shortcuts from the same runtime Action/Keymap truth.

Current Remote Workspace execution supports local → local, local → SFTP, and SFTP → local. SFTP → SFTP synchronization is intentionally blocked rather than hidden behind an implicit relay.

## Quick start

Install from the current source tree:

```bash
cargo install --git https://github.com/mrAibo/arx
arx
```

ARX requires Rust 1.88+ when building from source. Remote connections use the system OpenSSH client. Tools such as `rsync`, `bat`, `chafa`, `pdftotext`, `ffprobe`, and archive utilities are used when the corresponding feature needs them and the tool is available.

The currently published binary release artifact is Linux x86_64. Broader release packaging is intentionally deferred to the release-readiness milestone.

## 60-second Remote Workspace workflow

Default bindings are shown below. Discoverability surfaces in ARX use the runtime Keymap, so the UI remains the source of truth if bindings change.

1. Put the source workspace in one pane and the destination in the other, for example `~/code/app` and an SFTP host path.
2. Press `Ctrl+D` — **Compare panes**. ARX recursively scans both roots and builds a provider-neutral workspace diff.
3. Press `Ctrl+Shift+S` — **Preview workspace sync**. Review `ROUTE`, `PLAN`, and `SAFETY` before anything is queued.
4. Keep the default **Update** mode for a non-destructive first run. Press `Enter` to freeze and execute the current plan when it is safe.
5. The sync runs through JobManager in the background. `Esc` can hide the overlay without cancelling the job; `Ctrl+J` opens Jobs.
6. After execution reaches `Completed`, ARX performs a separate post-sync verification scan.
7. Trust the verification verdict, not the optimistic assumption: `Synchronized`, `DifferencesRemain`, `Inconclusive`, or a verification failure.

For a deterministic 20–30 second recording plan, see [DEMO.md](DEMO.md).

## Core workflows

### Remote Workspace

The Remote Workspace flow is deliberately split into independent truths:

```text
Workspace scan
      ↓
WorkspaceDiff
      ↓
Sync Preview
      ↓
Frozen Plan
      ↓
Compiled execution
      ↓
JobManager / Executor
      ↓
Post-sync verification
```

Two invariants matter:

> **Preview ≠ execution.** Looking at a plan does not mutate either workspace.

> **Execution completed ≠ workspace verified.** Verification is a separate result produced after the executor finishes.

ARX also refuses to call two entries identical when the provider metadata cannot prove equality. Equal hashes are proof; where hashes are unavailable, the current comparison requires matching provider-neutral fingerprint evidence rather than treating equal size alone as equality.

### Normal file management

Remote Workspace does not replace the commander workflow. ARX still provides:

- dual panes, tabs, history, bookmarks, selection, filters, and mouse support;
- local, SFTP, and archive browsing;
- F5/F6 copy and move through the transfer planner;
- transactional SFTP copy with staging and rollback protection;
- previews, `$EDITOR`, shell commands, quick actions, and tmux integration;
- background jobs with progress and explicit cancellation.

### Command Center and discoverability

`Ctrl+P` opens Command Center. With an empty query it recommends the next useful action from the current application state rather than presenting an alphabetic action dump. Typed search covers actions, hosts, bookmarks, history, and configured commands.

Contextual hints and the footer use the same Action Catalog, `ActionAvailability`, and runtime Keymap as input routing. ARX does not maintain a second hardcoded shortcut truth table for those surfaces.

## Safety model

ARX is conservative where a terminal file manager can cause damage:

- **Update is the default sync mode.** Destination-only entries are preserved.
- **Mirror is explicit.** Destination-only entries become planned deletes and destructive execution requires confirmation tied to the exact frozen plan.
- **Frozen-plan revalidation.** The workspace roots are rescanned before a frozen sync is queued; stale destructive confirmation does not create a job.
- **Conflicts block silent overwrite.** Differences that cannot be safely ordered are not silently converted into copies.
- **Cancellation is a runtime fact.** `Running → Cancelling → Cancelled` is observable and partial/cancelled execution does not receive a success milestone.
- **Post-sync verification is independent.** `DifferencesRemain` and `Inconclusive` are not rewritten as success.
- **Host-key verification stays enabled.** Remote transport uses the user's OpenSSH behavior rather than disabling trust checks.

See [ARCHITECTURE.md](ARCHITECTURE.md) for the full ownership and execution model.

## Essential bindings

| Binding | Action |
|---|---|
| `Ctrl+D` | Compare panes as one workspace |
| `Ctrl+Shift+S` | Preview workspace sync |
| `D` in Sync Preview | Reverse left → right / right → left |
| `M` in Sync Preview | Toggle Update / Mirror |
| `Enter` in Sync Preview | Freeze and execute when safe |
| `Enter` in confirmation | Confirm the exact destructive frozen plan |
| `C` in active sync | Request cancellation |
| `V` in finished sync | Show verification differences when available |
| `Ctrl+P` | Command Center |
| `F9` | Hosts |
| `Ctrl+J` | Jobs |
| `?` | Contextual help |

The full application retains familiar commander bindings for navigation, selection, copy/move, preview/edit, tabs, shell commands, bookmarks, and file operations.

## Remote hosts

Remote hosts are configured in `~/.config/arx/hosts.toml`:

```toml
[[hosts]]
id = "prod"
name = "Production"
ssh_alias = "prod"
hostname = "prod.example.net"
user = "deploy"
default_path = "/srv/app"
groups = ["production", "project-a"]
tags = ["linux"]
transfer_preference = "auto"
```

The Host Center (`F9`) displays this ARX inventory. It does **not** automatically import every `Host` entry from `~/.ssh/config`.

Connections use OpenSSH configuration and resolution, including aliases, `ProxyJump`, `IdentityFile`, agent authentication, host-key verification, and custom ports where supported. ARX keeps host metadata separate from SSH credentials.

## Configuration

### `~/.config/arx/arx.toml`

```toml
[ui]
show_hidden = false
editor = "hx"       # overrides $EDITOR/$VISUAL
```

### `~/.config/arx/arx.menu`

```text
# key  "Label"  command
t  "Tar home"  tar czf /tmp/home.tgz ~/
t  "Disk usage"  df -h
```

Configured menu entries are available through Command Center.

## Architecture at a glance

User input is routed through typed actions rather than being owned by renderer code:

```text
Input / Keymap
      ↓
    Action
      ↓
Controller / Dispatcher
      ↓
Effect / Service
      ↓
Provider / JobManager
```

Remote Workspace adds a safety pipeline on top of the same provider/job foundations:

```text
WorkspaceScanner
      ↓
WorkspaceDiff
      ↓
Frozen Plan
      ↓
Execution Compiler
      ↓
JobManager / Executor
      ↓
Verification
```

Presentation observes accepted backend truth. Hints, callouts, overlays, and first-success milestones do not start jobs or alter verification semantics.

Read [ARCHITECTURE.md](ARCHITECTURE.md) for module ownership, data flow, transfer execution, Remote Workspace invariants, and OpenSSH integration.

## Roadmap

The product-building phase is complete enough to shift the next milestones toward packaging and release:

1. **#38 — Product truth + killer demo.** README/architecture/roadmap alignment and a real Remote Workspace recording.
2. **#39 — Release readiness.** Align version metadata for the intended next release, expand the artifact matrix deliberately, add checksums, and gate packaging on tests/smoke evidence.
3. **#40 — Next release.** Publish the Remote Workspace release with focused release notes and explicit known limitations.

Package-manager distribution and promotion come after that release, not before it. See [ROADMAP.md](ROADMAP.md).

## Current limitations

- SFTP → SFTP workspace sync is intentionally unsupported.
- Verification can be `Inconclusive` when available metadata cannot prove equality.
- Host Center inventory comes from `~/.config/arx/hosts.toml`; it does not auto-discover all OpenSSH hosts.
- Current published release packaging is Linux x86_64 only.
- Retry, richer sync rate/ETA reporting, onboarding persistence, and automatic host discovery are not claimed as current features.

## Development

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo run
```

CI runs formatting, clippy with warnings denied, and the full feature test suite on Ubuntu. Avoid static test-count claims in documentation; the suite changes as the product evolves.

## License

MIT.
