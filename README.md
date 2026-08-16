# ARX

Terminal commander for local ↔ remote workspaces.

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
- **Truthful background jobs.** Queued, running, cancelling, cancelled,
  failed, completed, and verification remain separate states.
- **Verification after execution.** A completed job does not automatically
  mean the two roots are synchronized. ARX rescans both and reports
  `Synchronized`, `DifferencesRemain`, or `Inconclusive`.
- **Progressive discovery.** Command Center, contextual hints, and the
  footer derive actions and shortcuts from runtime truth.

Current Remote Workspace execution supports local → local, local → SFTP,
and SFTP → local. SFTP → SFTP synchronization is intentionally blocked.

## Quick start

### From source

Rust 1.88+ and system OpenSSH are required.

```bash
cargo install --git https://github.com/mrAibo/arx
arx
```

### Binary release (Linux x86_64)

Download the archive and SHA256SUMS from the [latest GitHub Release](https://github.com/mrAibo/arx/releases/latest).

```bash
# Verify checksum
sha256sum -c SHA256SUMS

# Extract
tar xzf arx-v0.17.0-x86_64-unknown-linux-gnu.tar.gz
cd arx-v0.17.0-x86_64-unknown-linux-gnu

# Place on PATH
sudo install -m 755 arx /usr/local/bin/arx
arx --version
```

Preview features use `bat`, `chafa`, `pdftotext`, `ffprobe`, and archive utilities when available. No other packages are required for the published binary.

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

### Selection

| Key | Action |
|-----|--------|
| Space | Toggle selection / advance |
| * | Invert selection |
| / | Filter by name |
| + | Select by glob |
| Right-click | Context menu |

### File operations

| Key | Action |
|-----|--------|
| **F3** | View — Local: full preview; SFTP: bounded text (1 MiB / 500 lines); **S3: bounded object preview (where supported)** |
| **F4** | Edit — Local: configured editor; SFTP: conflict-safe UTF-8 text edit, full-file only, binary/NUL refused; **S3: disabled** |
| **F5** | Copy — Local↔Local, Local↔SFTP (SFTP→SFTP unsupported); **S3: Local↔S3 and S3→Local single-object copy (no S3→S3, no SFTP↔S3)** |
| **F6** | Move — Local↔Local only; **S3: disabled** |
| **F7** | Create directory — Local + SFTP; **S3: creates a prefix marker (not a POSIX directory / bucket)** |
| **F8** | Delete — Local: trash; SFTP: permanent confirmed delete (no recursive remote delete); **S3: exact single-object or proven-empty marker delete (no recursive prefix delete, no bucket delete)** |
| Shift+F6 | Rename |
| Ctrl+U | Swap panes |
| Ctrl+X C / L / O / S | chmod / hardlink / chown / symlink |
| Ctrl+\\\\ | Toggle split pane |

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
| Ctrl+P | Command Center — fuzzy search hosts, bookmarks, history, quick actions |
| F9 | Remote Hosts |
| Ctrl+B | Bookmarks |
| Ctrl+J | Job queue with progress |
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
```

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

Menu entries appear in Command Center (Ctrl+P).

## Features

| Feature | Status |
|---------|--------|
| Dual-pane TUI with tabs, history, swap | ✅ |
| Local + SFTP + archive browsing | ✅ |
| Remote Workspace — compare → preview → execute → verify | ✅ |
| Transfer planner — native / rsync / SFTP streaming | ✅ |
| Transactional SFTP copy with rollback | ✅ |
| SFTP F3 bounded text preview | ✅ |
| SFTP F4 conflict-safe text editing | ✅ |
| ~/.ssh/config parsing (aliases, ProxyJump, keys) | ✅ |
| Managed SSH Host Manager (F12: add/edit/delete/test/identity/open) | ✅ |
| Command Center (Ctrl+P) — fuzzy search | ✅ |
| Quick Actions — compress, chmod, touch, mkdir, symlink, sha256 | ✅ |
| Preview engine — chafa, pdftotext, ffprobe, 7z, bat | ✅ |
| Background jobs with progress | ✅ |
| Embedded Terminal (Ctrl+X T) | ✅ |
| tmux sessions (Command Center) | ✅ |
| Mouse — right-click menu, drag multi-select, scroll | ✅ |
| Directory diff + content diff (Ctrl+D) | ✅ |
| Split pane toggle (Ctrl+\\\\) | ✅ |
| MC-style Ctrl+X prefix (symlink, hardlink, chmod, chown) | ✅ |
| User menu with custom scripts | ✅ |
| Host Center (F9) | ✅ |
| Extension colors, heatmap, git status bar | ✅ |
| S3 object-storage backend | ✅ AWS + MinIO PHYSICAL PASS (SUPPORTED MVP); Moto EMULATED PASS; R2/Wasabi UNVERIFIED (best-effort) |
| WebDAV backend | stub |

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

Capabilities: List / Read / Write / F3 (mkdir as a zero-byte prefix marker). Local↔S3
single-object copy only — no S3→S3, no SFTP↔S3. F7 creates a zero-byte prefix marker (not
a POSIX directory / bucket). F8 deletes an exact single object or a proven-empty marker (no
recursive prefix delete, no bucket delete). F4 and F6 are disabled. No S3 rename, no
recursive delete, no bucket delete.

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

Async interactive work uses correlated Effects (Preview, RemoteEdit).
Transfers, sync, and job-oriented mutations use the Job Manager.

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
disabled (no generic edit path yet), so availability is capability **and**
provider-policy gated, not capability-only.

**Transfer Stack:** `TransferPlanner` builds a frozen plan from a
`TransferRequest`. The planner picks native, rsync, or SFTP streaming.
No plan mutates after dispatch.

**SFTP:** Connection pooling per host. Ambiguous transport failures
invalidate the session; definitive protocol errors don't. SFTP copies
use transactional staging (temp → backup → commit → rollback).

**Safety:** SFTP copies stage to temp first. Destructive sync requires
Preview. Cancel leaves source untouched. Host key verification uses the
user's OpenSSH. Logs don't leak credentials.

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
