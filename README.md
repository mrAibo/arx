# ARX

A keyboard-driven dual-pane terminal file manager. Local, SFTP, archives,
background transfers, quick actions, and tmux integration — all from one
terminal. Midnight Commander parity, built in Rust.

## What's new in v0.14

Transfer Stack replaces the old F5/F6 handlers. When you hit copy or move,
ARX probes what tools are available, picks the best method (native, rsync,
or SFTP streaming), and runs the transfer through the job manager. SFTP
copies are transactional: temp file, backup existing, commit, rollback on
failure. 42 tests, clippy-clean.

## Quick start

```bash
cargo install --git https://github.com/mrAibo/arx
arx
```

## Keybindings

### Navigation

|| Key | Action |
||---|---|
|| j / ↓ / k / ↑ | Move cursor |
|| Enter | Enter directory / archive / content diff |
|| Backspace | Go to parent directory / exit archive |
|| Tab | Switch active pane |
|| Ctrl+G | Go to path |
|| Ctrl+H | Toggle hidden files |
|| Alt+Down | Back in directory history |

### Selection

|| Key | Action |
||---|---|
|| Space | Toggle selection |
|| * | Invert selection |
|| / | Filter files by name |
|| + | Select by glob |
|| Right-click | Context menu (Copy/Move/Delete/View/Edit) |
|| Left-drag | Multi-select |

### File operations

|| Key | Action |
||---|---|
|| F3 | View (read-only preview) |
|| F4 | Edit in configured editor |
|| F5 | Copy — planner picks native/rsync/SFTP |
|| F6 | Move — planner picks native/rsync/SFTP |
|| F7 | Create directory |
|| F8 | Delete (trash) |
|| Shift+F6 | Rename |
|| Ctrl+U | Swap panes |
|| Ctrl+X C / L / O / S | chmod / hardlink / chown / symlink |
|| Ctrl+\\\\ | Toggle split pane |

### View & Preview

|| Key | Action |
||---|---|
|| F3 | Preview (chafa for images, pdftotext, ffprobe, 7z, bat) |
|| Ctrl+I | File attributes |

### Tools & Overlays

|| Key | Action |
||---|---|
|| Ctrl+P | Command Center — fuzzy search hosts, bookmarks, history, quick actions |
|| F9 | Tmux sessions |
|| Ctrl+B | Bookmarks |
|| Ctrl+D | Directory diff |
|| Ctrl+J | Job queue with progress bars |
|| Ctrl+O | Drop to subshell |
|| Ctrl+R | Refresh panes |
|| : | Run shell command |

### Tabs

|| Key | Action |
||---|---|
|| Ctrl+T | New tab |
|| Ctrl+W | Close tab |
|| Ctrl+← / → | Previous / next tab |
|| Alt+1…9 | Jump to tab |

### Misc

|| Key | Action |
||---|---|
|| ? | Help overlay |
|| q | Quit |
|| Esc | Close any popup |

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

Hosts resolve through `~/.ssh/config` — aliases, ProxyJump, IdentityFile, custom ports.

### `~/.config/arx/arx.menu`

```toml
# t  "Label"  command
t  "Tar home"  tar czf /tmp/home.tgz ~/
t  "Disk usage"  df -h
```

Menu entries appear in Command Center (Ctrl+P).

## Features at a glance

|| Feature | Status |
||---|---|
|| Dual-pane TUI with tabs, history, swap | ✅ |
|| Local + SFTP + archive browsing | ✅ |
|| Transfer planner — native / rsync / SFTP streaming | ✅ |
|| Transactional SFTP copy with rollback | ✅ |
|| ~/.ssh/config parsing (aliases, ProxyJump, keys) | ✅ |
|| Command Center (Ctrl+P) — fuzzy search all targets | ✅ |
|| Quick Actions — compress, chmod, touch, mkdir, symlink, sha256 | ✅ |
|| Preview engine — chafa, pdftotext, ffprobe, 7z, bat | ✅ |
|| Background jobs with progress bars | ✅ |
|| tmux session attach (F7) | ✅ |
|| Mouse — right-click menu, drag multi-select, scroll | ✅ |
|| Directory diff + content diff (Ctrl+D) | ✅ |
|| Split pane toggle (Ctrl+\\) | ✅ |
|| MC-style Ctrl+X prefix (symlink, hardlink, chmod, chown) | ✅ |
|| User menu with custom scripts | ✅ |
|| Host Center (F9) | ✅ |
|| Hotlist, tab switcher, batch rename | ✅ |
|| Extension colors, heatmap, git status bar | ✅ |
|| X11 DISPLAY auto-detect for Windows SSH clients | ✅ |
|| S3/MinIO + WebDAV backends | stubs |
|| Lua/WASM plugin system | stubs |

## Architecture

```
arx/
├── src/
│   ├── main.rs              # entry point, DISPLAY auto-detect
│   ├── tui.rs               # ratatui event loop, all keybindings, rendering
│   ├── app/mod.rs           # AppState, PaneState, Job queue
│   ├── vfs/
│   │   ├── mod.rs           # VfsOps trait, Entry/Location, ProviderId, Capabilities
│   │   ├── local.rs         # std::fs backend
│   │   ├── sftp.rs          # OpenSSH SFTP via russh (~/.ssh/config)
│   │   ├── archive.rs       # tar.gz/zip as directories
│   │   ├── s3.rs            # S3/MinIO stub
│   │   └── webdav.rs        # WebDAV stub
│   ├── transfer/
│   │   ├── mod.rs           # TransferPlanner, TransferPlan, TransferIntent/Method
│   │   ├── executor.rs      # async Native/rsync/SFTP executors
│   │   ├── probe.rs         # local & remote tool capability detection
│   │   └── sftp_copy.rs     # transactional SFTP copy with staging + rollback
│   ├── remote/
│   │   ├── mod.rs           # HostInventory, HostConfig
│   │   ├── hosts_config.rs  # hosts.toml parser
│   │   ├── openssh_sftp.rs  # OpenSSH → SFTP transport
│   │   ├── ssh_config.rs    # ~/.ssh/config parser
│   │   └── watch.rs         # inotify → rsync daemon (Linux-only)
│   ├── jobs/mod.rs          # Job, JobEvent, progress tracking
│   ├── plugins/mod.rs       # Plugin hook stubs (Lua/WASM)
│   ├── config.rs            # arx.toml loader
│   ├── terminal.rs          # PTY + subshell
│   ├── keyring.rs           # system keychain for SSH passphrases
│   └── lib.rs
└── tests/                   # 42 tests
```

**VFS:** `VfsOps` trait + `Location` enum + `ProviderId` + `CapabilitySet`.
Backends implement `list()`, `read_head()`, `copy_files()`, `move_files()`, `delete_files()`.

**Transfer Stack:** F5/F6 builds a `TransferRequest` (source and destination
locations, provider IDs, capability sets, what executors are available).
The planner picks a method — Native, rsync, or SFTP — and the executor runs
it through the job manager. Progress, cancellation, and errors come back
through `tokio::sync::mpsc`.

**Event loop:** `tokio::select!` multiplexes keyboard events, job
completions, and PTY output in one async loop.

## MC parity

All Midnight Commander features are present: dual-pane, tabs, F3–F8,
directory diff, user menu, bookmarks, SFTP (via ~/.ssh/config), archive
browsing, background jobs, mkdir/rename, file info, drop-to-shell,
symlink/hardlink/chmod/chown, extension colors, shortcut bar.
ARX adds a few things MC doesn't have: content diff (Ctrl+D+Enter runs
`diff -u`), Quick Actions, and a fuzzy Command Center.

## Development

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo run
```

42 tests, clippy-clean, CI green on ubuntu-latest.

## License

MIT.