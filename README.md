# ARX

Terminal commander for local ↔ remote workspaces on Linux.

![ARX Remote Workspace — Compare, Preview, Sync, Verify](docs/assets/remote-workspace-update.gif)

Compare before touching anything.
Preview the exact consequences.
Execute.
Verify the real result.

## Why ARX?

- **Local ↔ remote as one workspace.** Put a project on one side and an
  SFTP location on the other, then compare the recursive trees directly.
- **Compare before execution.** The current workspace diff is an explicit
  fact. ARX does not turn a comparison into a mutation.
- **Preview exact consequences.** Sync Preview shows direction,
  update/mirror mode, planned copies, deletes, conflicts, and transfer
  size before execution.
- **Safe default.** Update mode preserves destination-only entries.
  Mirror is distinct and requires explicit confirmation.
- **Truthful background jobs.** Queued, running, pause-pending, paused,
  cancelling, cancelled, retry-waiting, failed, completed, and verification
  remain separate states.
- **Read-only storage visibility.** Inspect local directory usage and Linux
  mounted filesystems without turning observability into cleanup or mount mutation.
- **Verification after execution.** A completed job does not automatically
  mean the two roots are synchronized. ARX rescans both and reports
  `Synchronized`, `DifferencesRemain`, or `Inconclusive`.
- **Progressive discovery.** Command Center, contextual hints, and the
  footer derive actions and shortcuts from runtime truth.

Current Remote Workspace execution supports local → local, local → SFTP,
and SFTP → local. SFTP → SFTP synchronization is intentionally blocked.

## Platform support

ARX is intentionally a **Linux application**. The published binary target is
Linux x86_64. Native Windows support is not planned; the project stays focused
on Linux terminal workflows instead of broadening the platform surface.

When ARX runs inside an SSH session, the session environment is authoritative.
ARX preserves an existing `DISPLAY` but never synthesizes one. X11 forwarding must
already be established by the SSH client/session (`ssh -X/-Y` or the client-specific
equivalent); ARX does not create an X11 tunnel after login.

## Quick start

### From source

Rust 1.88+ and system OpenSSH are required. The built-in **Compress to tar.gz** Quick
Action additionally requires a system `tar` executable; SHA-256 and Touch do not use a
shell command.

```bash
cargo install --git https://github.com/mrAibo/arx
arx
```

### Release packages (Linux x86_64)

Download the artifact for your system and `SHA256SUMS` from the
[latest GitHub Release](https://github.com/mrAibo/arx/releases/latest). When a
release contains a native `.deb` or `.rpm`, prefer that package; the tarball
remains the universal Linux x86_64 fallback.

Verify the release files you downloaded:

```bash
sha256sum --ignore-missing -c SHA256SUMS
```

Set `VERSION` to the release version (without the leading `v`), then install:

```bash
VERSION=x.y.z

# Debian / Ubuntu
sudo apt install "./arx_${VERSION}_amd64.deb"

# Fedora / RHEL-family systems
sudo dnf install "./arx-${VERSION}-1.x86_64.rpm"
```

Or use the portable tarball:

```bash
VERSION=x.y.z
DIR="arx-v${VERSION}-x86_64-unknown-linux-gnu"
tar xzf "${DIR}.tar.gz"
sudo install -m 755 "${DIR}/arx" /usr/local/bin/arx
arx --version
```

Release bundles include the ARX MIT license and a generated third-party license
report. Native packages install the documentation under `/usr/share/doc/arx`.

Preview features use `bat`, `chafa`, `pdftotext`, `ffprobe`, and archive utilities when
available. These helpers are optional. **Compress to tar.gz** specifically requires
system `tar`; if it is unavailable ARX reports that fact instead of falling back to a
shell-string implementation.

## 60-second Remote Workspace workflow

1. Put the source workspace in one pane and the destination in the other,
   e.g. `~/code/app` and an SFTP host path.
2. Press **Ctrl+D** — Compare panes. ARX recursively scans both roots.
3. Press **Ctrl+X P** — Preview workspace sync. Review the frozen plan
   before anything is queued.
4. Keep the default Update mode. Press **Enter** to execute.
5. The sync runs through the Job Manager in the background. Esc hides
   the overlay without cancelling; Ctrl+J opens Jobs.
6. After execution reaches Completed, ARX performs a separate
   post-sync verification scan.
7. Trust the verdict: `Synchronized`, `DifferencesRemain`, or
   `Inconclusive`.

## Keybindings

Bindings shown are the primary discoverable shortcuts. The Command Center
(Ctrl+P) surfaces additional actions. The UI footer is the runtime source
of truth.

### Navigation

| Key | Action |
|-----|--------|
| j / ↓ / k / ↑ | Move cursor |
| Enter | Enter directory / archive / content diff |
| Backspace | Go to parent / exit archive |
| Tab | Switch active pane |
| Ctrl+G | Go to path |
| Ctrl+H | Toggle hidden files |
| Alt+Down / Alt+Up | Directory history |
| Mouse wheel | Scroll the active file pane by 3 visible rows; passive panes do not steal focus |

### Selection

| Key | Action |
|-----|--------|
| Space | Toggle selection / advance |
| * | Invert selection |
| / | Filter by name |
| + | Select by glob |
| Shift+Left-click | Add the inclusive real-row range in the clicked pane/location; synthetic Parent/LoadMore rows are skipped |
| Right-click | Provider-aware typed context menu using canonical action availability; stale exact targets fail closed |

### File operations

| Key | Action |
|-----|--------|
| **F3** | View — Local: full preview; SFTP: bounded text (1 MiB / 500 lines); S3: bounded object preview where supported; **WebDAV: bounded one-file preview** |
| **F4** | Edit — Local: configured editor; SFTP: conflict-safe UTF-8 text edit, full-file only, binary/NUL refused; **S3 and WebDAV: disabled** |
| **F5** | Copy — Local↔Local, Local↔SFTP (SFTP→SFTP unsupported); S3: Local↔S3 single-object copy; **WebDAV: Local↔WebDAV one-file copy**. No SFTP↔cloud or cross-target WebDAV transfer claim |
| **F6** | Move — Local↔Local product path. **S3 disabled; cross-target WebDAV move unsupported** |
| **F7** | Create directory — Local + SFTP; S3: prefix marker (not a POSIX directory / bucket); **WebDAV: MKCOL** |
| **F8** | Delete — Local: trash; SFTP: permanent confirmed delete (no recursive remote delete); S3: exact single-object or proven-empty marker delete; **WebDAV: permanent non-recursive resource delete** |
| Shift+F6 | Rename |
| Ctrl+U | Swap panes |
| Ctrl+X C / L / O / S | chmod / hardlink / chown / symlink |
| Ctrl+\\ | Toggle split pane |

### View & Preview

| Key | Action |
|-----|--------|
| Ctrl+D | Directory diff (compare panes) |
| Ctrl+X P | Workspace Sync Preview (primary) |
| Ctrl+X T | Embedded Terminal |
| Ctrl+I | File attributes |

### Tools & Overlays

| Key | Action |
|-----|--------|
| Ctrl+P | Command Center — fuzzy search actions, hosts, bookmarks, history, and user-menu commands |
| F9 | Remote Hosts |
| Ctrl+B | Bookmarks |
| Ctrl+J | Job queue with progress and transfer Pause/Resume/Cancel |
| Ctrl+Y | Transfer Center — Active/History/All views plus runtime Pause/Resume/Cancel |
| Alt+U | Local Storage Inspector — read-only directory usage scan and drill-down |
| Alt+D | Filesystems — read-only Linux mount/capacity/inode view |
| Ctrl+O | Drop to subshell |
| Ctrl+R | Refresh panes |
| : | Run shell command |

### Tabs

| Key | Action |
|-----|--------|
| Ctrl+T | New tab |
| Ctrl+W | Close tab |
| Ctrl+← / → | Previous / next tab |
| Alt+1…9 | Jump to tab |

### Misc

| Key | Action |
|-----|--------|
| ? | Help overlay |
| q | Quit |
| Esc | Close any popup |

## Configuration

### `~/.config/arx/arx.toml`

```toml
[ui]
show_hidden = false
editor = "hx"       # overrides $EDITOR/$VISUAL

[transfer]
concurrency = 2      # default 2; valid range 1..=8
```

Transfer concurrency bounds simultaneous transfer jobs. Invalid values are rejected
by configuration validation; the scheduler and configuration layer share the same
bounds.

### `~/.config/arx/hosts.toml`

```toml
[[hosts]]
id = "nuc"
name = "Headless NUC"
hostname = "192.168.1.10"
user = "aibo"
default_path = "/home/aibo"
```

Hosts resolve through `~/.ssh/config` — aliases, ProxyJump, IdentityFile,
custom ports.

### `~/.config/arx/arx.menu`

```toml
# t  "Label"  command
t  "Tar home"  tar czf /tmp/home.tgz ~/
t  "Disk usage"  df -h
```

Menu entries appear in Command Center (Ctrl+P). This is ARX's supported lightweight
extension mechanism. ARX does not ship an embedded Lua or WASM plugin runtime.

## Features

| Feature | Status |
|---------|--------|
| Dual-pane TUI with tabs, history, swap | ✅ |
| Local + SFTP + archive browsing | ✅ |
| Remote Workspace — compare → preview → execute → verify | ✅ |
| Transfer planner — native / rsync / SFTP streaming | ✅ |
| Transfer Queue — bounded FIFO, default N=2 (1..=8), progress/rate/ETA, Pause/Resume/Cancel, safe retry ≤3 total attempts | ✅ |
| Transfer Center v2 — Active/History/All, selected-job detail, runtime-owned controls | ✅ |
| Local Storage Inspector (`Alt+U`) — logical/allocated usage, drill-down, top files, partial/cancel truth | ✅ Linux read-only |
| Filesystems (`Alt+D`) — mount usage, capacity/inodes, sort/filter, unavailable/autofs truth | ✅ Linux read-only |
| Transactional SFTP copy with rollback | ✅ |
| SFTP F3 bounded text preview | ✅ |
| SFTP F4 conflict-safe text editing | ✅ |
| ~/.ssh/config parsing (aliases, ProxyJump, keys) | ✅ |
| Managed SSH Host Manager (F12: add/edit/delete/test/identity/open) | ✅ |
| Command Center (Ctrl+P) — fuzzy search | ✅ |
| Quick Actions — typed local SHA-256 / Touch / Compress to tar.gz plus mkdir/chmod/symlink; user menu remains the separate extension surface | ✅ |
| Preview engine — chafa, pdftotext, ffprobe, 7z, bat | ✅ |
| Background jobs with progress | ✅ |
| Embedded Terminal (Ctrl+X T) | ✅ |
| tmux/screen integration — tmux discovery/attach shipped; screen discovery/lifecycle remain #7 | ⚠️ Partial |
| Mouse — visible-row-correct clicks, drag selection, active-pane wheel, Shift+Click ranges, and provider-aware typed context menu | ✅ (#10 / PR #236) |
| Directory diff + content diff (Ctrl+D) | ✅ |
| Split panes — vertical split/focus shipped; horizontal/resize/explicit close remain #16 | ⚠️ Partial |
| MC-style Ctrl+X prefix (symlink, hardlink, chmod, chown) | ✅ |
| User extensions — `arx.menu`; no embedded Lua/WASM runtime | ✅ Lean |
| Host Center (F9) | ✅ |
| Extension colors, heatmap, git status bar | ✅ |
| S3 object-storage backend | ✅ AWS + MinIO PHYSICAL PASS (SUPPORTED MVP); Moto EMULATED PASS; R2/Wasabi UNVERIFIED (best-effort) |
| WebDAV backend | ✅ **SUPPORTED MVP** — Apache mod_dav PHYSICAL PASS (W1–W18); Basic auth only; Nextcloud/ownCloud UNVERIFIED |
| Native Linux release packages | ✅ tar.gz + `.deb` + `.rpm`, one validated ELF + SHA256SUMS |

The partial rows above are intentionally explicit. They reflect the current canonical
action-registration/runtime truth rather than old issue titles or prototype code.

## Configurable keybindings

Key bindings for the KeyRouter-managed contexts (`browser`, `sync_preview`,
`sync_confirmation`, `sync_job`) can be overridden in the config file:

```toml
[[keybindings]]
context = "browser"
action = "open_storage_inspector"
keys = "F11"

[[keybindings]]
context = "browser"
action = "open_smart_tree"
disabled = true
```

- Exactly one of `keys` (non-empty sequence, e.g. `"F11"`, `"Ctrl+X P"`) or
  `disabled = true` is required per entry.
- Overrides replace all built-in bindings (including compatibility aliases) for that
  context/action pair and appear as `user` bindings in `arx --print-keymap`.
- Only KeyRouter-managed action/context pairs are configurable; navigation keys owned
  by legacy browser routes (Tab, Enter, arrows, …) cannot be rebound and conflicts with
  them fail startup clearly instead of silently misbehaving.
- Duplicate or prefix-conflicting sequences inside one context are configuration
  errors; reusing a sequence across different contexts is fine.
- Inspect the effective map any time with:

```
arx --print-keymap
```

`--config <path>` loads exactly that file — a missing or malformed explicit config is
an error, never silently replaced by defaults.

## Typed local Quick Actions

Quick Actions are built-in typed actions discovered through **Ctrl+P Command Center**;
they do not consume a new global shortcut namespace and they fail closed outside a
Local pane.

- **Compute SHA-256** hashes the focused or selected regular file(s) in Rust and keeps
  each digest associated with its exact filename.
- **Touch file** prompts for one child name, rejects traversal/absolute names and uses
  `O_NOFOLLOW` plus `futimens` on the exact opened regular file.
- **Compress to tar.gz** freezes the selected/focused entry names, invokes system `tar`
  with typed argv and `--` before filenames, stages in the destination directory, and
  finalizes with noclobber semantics so an existing archive is never replaced.

Long work runs through a dedicated correlated Effect lane rather than blocking the TUI.
Quit requests cancellation and waits for a terminal outcome. Filenames and tool errors
are escaped for control characters before presentation while printable Unicode is kept.

## Transfer Queue

Copy/Move work on supported transfer paths runs through one persistent bounded FIFO
runtime instead of creating a second lifecycle. The default concurrency is 2 and can
be configured from 1 through 8. The status bar and Transfer Center report the primary
running transfer's percentage, byte rate, and ETA only when those facts are known;
an unknown total is never rendered as zero.

Pause/Resume is cooperative and resumes the same JobId/execution attempt at a safe
checkpoint. Cancel affects the selected transfer without cancelling unrelated work.
Automatic retry is bounded to at most 3 total attempts and is allowed only for errors
classified `SafeToRetry`; ambiguous remote mutations and recovery-required outcomes
are never blindly replayed.

Transfer Center v2 is a keyboard-first control surface over that same runtime. Its
Active, History, and All views do not create a second scheduler or lifecycle model;
controls call the existing queue runtime and terminal jobs remain immutable history.

## Storage inspection

ARX v0.20 adds two Linux-native, read-only storage views. Neither one performs cleanup,
delete, mount, remount, quota, or resize operations.

**Alt+U — Local Storage Inspector (`du++`).** Starts a background StorageScan job for
the active local directory. The scan keeps logical/apparent bytes separate from
allocated/on-disk bytes, avoids following symlinks, de-duplicates hard links, supports
drill-down and top-file views, and exposes partial/error/cancelled outcomes instead of
pretending an incomplete scan is exact. Expensive traversal stays off the UI thread and
uses the existing JobManager cancellation path.

**Alt+D — Filesystems (`df++`).** Reads Linux mount topology and filesystem statistics
into a sortable/filterable overlay with total/used/available capacity and inode views.
Autofs is not probed by the default snapshot, inaccessible mounts remain visible with an
unavailable state, and refresh is explicit rather than periodic background polling.

These views are local-Linux observability features. They do not claim SFTP/S3/WebDAV
capacity semantics where the provider cannot prove them.

## S3 object storage

Real AWS S3 and MinIO are physically accepted (20/20 physical tests against AWS account
`715844024414`, immutable SHA `b5f0ee6`); Moto is emulated-accepted. Cloudflare R2 and
Wasabi are **unverified — best-effort only, not claimed supported.**

| Backend | Status |
|---------|--------|
| AWS S3 (account 715844024414) | ✅ PHYSICAL PASS — SUPPORTED MVP |
| MinIO | ✅ PHYSICAL PASS — SUPPORTED MVP |
| Moto (emulator) | ✅ EMULATED PASS |
| Cloudflare R2 / Wasabi | ⚠️ UNVERIFIED — best-effort only |

## WebDAV backend

Production `VfsProvider` over `reqwest` + `quick-xml`. The WebDAV MVP introduced in
v0.18.0 uses real DAV semantics, authoritative raw-href identity, bounded
parsing/preview, no plaintext password in config, and no blind retry of ambiguous
mutations.

Physical support status:

| Backend | Status |
|---------|--------|
| Apache mod_dav | ✅ PHYSICAL PASS — W1–W18, SUPPORTED MVP |
| Nextcloud | ⚠️ UNVERIFIED — no physical-certification claim |
| ownCloud | ⚠️ UNVERIFIED — no physical-certification claim |

```toml
# ~/.config/arx/arx.toml
[[webdav.targets]]
id = "dav"
name = "WebDAV server"
url = "https://dav.example.com/files/"
username = "me"
# password resolved from OS keyring (id "dav") or ARX_WEBDAV_DAV_PASSWORD;
# never stored in this file
```

Capabilities in the MVP: List / Read / F3 bounded preview / Write through one-file
F5 Local↔WebDAV / Mkdir (F7 MKCOL) / Delete (F8) / server-side COPY/MOVE within the
provider seam where the product path permits it. **F4 remote edit is unsupported.**
Cross-target WebDAV move is unsupported, and recursive WebDAV transfer/delete is not
claimed.

Auth: **HTTP Basic only** for the MVP. Digest/Bearer are deferred. Secrets come from
the OS keyring keyed by target `id`, or the `ARX_WEBDAV_<ID>_PASSWORD` environment
variable; config holds neither. URLs are redacted in diagnostics and passwords never
reach logs.

Transfer safety: remote overwrite-forbid uses `If-None-Match: *`; local downloads
stage into a temporary file in the destination directory and finalize with real
noclobber semantics. Automatic HTTP retries are disabled so an ambiguous mutation is
reported as an error rather than blindly replayed.

## SSH Host Manager (F12)

ARX owns a dedicated managed config at `~/.ssh/arx_hosts.conf` (installed via at most one
`Include` in your `~/.ssh/config`); your `~/.ssh/config` stays user-owned.

- **Add / Edit / Delete / Test / Open** managed hosts.
- **IdentityFile** — attach an existing key by **path only**; ARX never stores private-key bytes.
- **Generate** an unencrypted Ed25519 key **only after explicit confirmation**.
- **Passwords** remain OS-keyring-backed.

## Architecture

```
TUI ──▶ Input / Keymap
           │
           ▼
      AppState ──▶ Availability
           │
           ▼
      Dispatcher
           │
    ┌──────┼────────┐
    ▼      ▼        ▼
 Effect  Service   Job
    │      │        │
    ▼      ▼        ▼
 Provider  │  TransferPlanner
    │      │        │
    ▼      ▼        ▼
 VFS    Process  Executors
```

Async interactive work uses correlated Effects (Preview, RemoteEdit, QuickAction).
Transfers, sync, storage scans, and job-oriented mutations use the Job Manager.

```
arx/
├── src/
│   ├── main.rs
│   ├── tui.rs               # event loop, rendering, keybindings
│   ├── app/
│   │   ├── mod.rs           # AppState, PaneState
│   │   └── availability.rs  # action availability rules
│   ├── vfs/                 # VfsProvider trait, Location, Capability
│   │   ├── local.rs, sftp.rs, archive.rs, s3.rs, webdav.rs
│   ├── transfer/            # TransferPlanner, executors, SFTP copy
│   ├── remote/              # HostConfig, OpenSSH transport, ssh_config
│   ├── process/             # ProcessService, remote edit lifecycle
│   ├── jobs/                # Job manager
│   ├── input/               # Keymap, hints
│   ├── effects.rs, effect_dispatcher.rs
│   ├── config.rs, terminal.rs, lib.rs
└── tests/
```

**VFS:** `VfsProvider` trait + `Location` + `CapabilitySet`. Backends
implement listing, metadata, bounded reads, exact-length reads, and
write-back via immutable revision. Capability sets gate action
availability — F4 only shows when both Read and Write are present **and** the
provider policy allows editing. S3 has Read+Write but F4 stays intentionally
disabled; WebDAV F4 is likewise intentionally unsupported in the MVP.

**Transfer Stack:** `TransferPlanner` builds a frozen plan from a
`TransferRequest`. The planner picks native, rsync, SFTP streaming, S3, or WebDAV
execution according to the source/destination pair and capabilities. No plan mutates
after dispatch. A persistent Transfer Queue schedules supported plans in the
background; it does not create new provider combinations.

**SFTP:** Connection pooling per host. Ambiguous transport failures
invalidate the session; definitive protocol errors don't. SFTP copies
use transactional staging (temp → backup → commit → rollback).

**Safety:** SFTP copies stage to temp first. Destructive sync requires
Preview. Cancel leaves source untouched. Host key verification uses the
user's OpenSSH. WebDAV overwrite-forbid is atomic at the HTTP/file-finalization
boundaries. Typed Quick Actions do not interpolate filenames into shell strings;
archive creation stages and finalizes noclobber. Transfer retries are phase-aware and
never blindly replay ambiguous mutations. Logs don't leak credentials.

## Development

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo run
```

Full Rust test suite, clippy-clean, CI on ubuntu-latest.

## License

MIT