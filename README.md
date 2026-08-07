# ARX

A keyboard-driven dual-pane terminal file manager. Local, SFTP, archives,
background transfers, quick actions, and tmux integration — all from one
terminal. Midnight Commander parity, built in Rust.

## What's new in v0.14

Transfer Stack — Detection → Planner → Executor replaces old F5/F6 logic.
Transactional SFTP file copy with staged temp files, backup, and rollback.
Native copy/move, rsync, and SFTP streaming selected by the planner based
on detected capabilities. 42 tests, clippy-clean.

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
|| F5 | Copy — planner selects native/rsync/SFTP |
|| F6 | Move — planner selects native/rsync/SFTP |
|| F7 | Connect to tmux session |
|| F8 | Delete selected |
|| Shift+F6 | Rename |
|| Ctrl+U | Swap panes |
|| Ctrl+X C / L / O / S | chmod / hardlink / chown / symlink |
|| Ctrl+\\ | Toggle split pane |

### View & Preview

|| Key | Action |
||---|---|
|| F3 | Preview (images/chafa, PDFs/pdftotext, media/ffprobe, archives/7z, code/bat) |
|| Shift+F3 | bat with paging |
|| F4 | Edit in $EDITOR |
|| Ctrl+I | File attributes |

### Tools & Overlays

|| Key | Action |
||---|---|
|| Ctrl+P | Command Center — fuzzy search hosts, bookmarks, history, quick actions |
|| F7 | Attach tmux session |
|| F9 | Host panel (SFTP) |
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

**Transfer Stack:** When F5/F6 is pressed, the TUI builds a `TransferRequest`
(source/dest `Location`, `ProviderId`, `CapabilitySet`, `ExecutorAvailability`).
The `TransferPlanner` picks a method (Native / rsync / SFTP), and the matching
executor runs the transfer through the Job Manager. Progress, cancellation, and
errors are delivered via `tokio::sync::mpsc`.

**Event loop:** `tokio::select!` — keyboard events, background job completions,
and PTY output on one async loop.

## MC parity

All Midnight Commander features are present: dual-pane, tabs, F3–F8,
directory diff, user menu, bookmarks, SFTP (with ~/.ssh/config), archive
browsing, background jobs, mkdir/rename, file info, drop-to-shell,
symlink/hardlink/chmod/chown, extension colors, shortcut bar.
ARX adds content diff (Ctrl+D+Enter runs `diff -u`), Quick Actions,
and a fuzzy Command Center not in MC.

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