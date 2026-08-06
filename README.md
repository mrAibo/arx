# ARX

A keyboard-driven dual-pane terminal file manager. Local, SFTP, archives,
background transfers, quick actions, and tmux integration — all from one
terminal. Midnight Commander parity, built in Rust.

## What's new in v0.11

Command Center (Ctrl+P) with fuzzy search, Quick Actions (compress/chmod),
mouse gestures, job progress bars, ~/.ssh/config parsing, tmux attach (F7).
S3 and WebDAV backends stubbed, plugin hooks ready.

## Quick start

```bash
cargo install --git https://github.com/mrAibo/arx
arx
```

## Keybindings

### Navigation

| Key | Action |
|---|---|
| j / ↓ / k / ↑ | Move cursor |
| Enter | Enter directory / archive / content diff |
| Backspace | Go to parent directory / exit archive |
| Tab | Switch active pane |
| Ctrl+G | Go to path |
| Ctrl+H | Toggle hidden files |
| Alt+Down | Back in directory history |

### Selection

| Key | Action |
|---|---|
| Space | Toggle selection |
| * | Invert selection |
| / | Filter files by name |
| + | Select by glob |
| Right-click | Context menu (Copy/Move/Delete/View/Edit) |
| Left-drag | Multi-select |

### File operations

| Key | Action |
|---|---|
| F5 | Copy to other pane (rsync or copy_files) |
| F6 | Move to other pane |
| F7 | Connect to tmux session |
| F8 | Delete selected |
| Shift+F6 | Rename |
| Ctrl+U | Swap panes |
| Ctrl+X C / L / O / S | chmod / hardlink / chown / symlink |
| Ctrl+\ | Toggle split pane |

### View & Preview

| Key | Action |
|---|---|
| F3 | Preview (images/chafa, PDFs/pdftotext, media/ffprobe, archives/7z, code/bat) |
| Shift+F3 | bat with paging |
| F4 | Edit in $EDITOR |
| Ctrl+I | File attributes |

### Tools & Overlays

| Key | Action |
|---|---|
| Ctrl+P | Command Center — fuzzy search hosts, bookmarks, history, quick actions |
| F7 | Attach tmux session |
| F9 | Host panel (SFTP) |
| Ctrl+B | Bookmarks |
| Ctrl+D | Directory diff |
| Ctrl+J | Job queue with progress bars |
| Ctrl+O | Drop to subshell |
| Ctrl+R | Refresh panes |
| : | Run shell command |

### Tabs

| Key | Action |
|---|---|
| Ctrl+T | New tab |
| Ctrl+W | Close tab |
| Ctrl+← / → | Previous / next tab |
| Alt+1…9 | Jump to tab |

### Misc

| Key | Action |
|---|---|
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

Hosts resolve through `~/.ssh/config` — aliases, ProxyJump, IdentityFile, custom ports.
SSH agent support is documented; full integration deferred (russh API constraint).

### `~/.config/arx/arx.menu`

```toml
# t  "Label"  command
t  "Tar home"  tar czf /tmp/home.tgz ~/
t  "Disk usage"  df -h
```

Menu entries appear in Command Center (Ctrl+P).

## Features at a glance

| Feature | Status |
|---|---|
| Dual-pane TUI with tabs, history, swap | ✅ |
| Local + SFTP + archive browsing | ✅ |
| ~/.ssh/config parsing (aliases, ProxyJump, keys) | ✅ |
| Command Center (Ctrl+P) — fuzzy search all targets | ✅ |
| Quick Actions — compress, chmod, touch, mkdir, symlink, sha256 | ✅ |
| Preview engine — chafa, pdftotext, ffprobe, 7z, bat | ✅ |
| Background jobs with progress bars | ✅ |
| Smart rsync (F5, falls back to VfsOps copy) | ✅ |
| tmux session attach (F7) | ✅ |
| Mouse — right-click menu, drag multi-select, scroll | ✅ |
| Directory diff + content diff (Ctrl+D) | ✅ |
| Split pane toggle (Ctrl+\) | ✅ |
| MC-style Ctrl+X prefix (symlink, hardlink, chmod, chown) | ✅ |
| User menu with custom scripts | ✅ |
| Host Center (F9) | ✅ |
| Hotlist, tab switcher, batch rename | ✅ |
| Extension colors, heatmap, git status bar | ✅ |
| X11 DISPLAY auto-detect for Windows SSH clients | ✅ |
| S3/MinIO + WebDAV backends | stubs |
| Lua/WASM plugin system | stubs |
| Windows native build | stubs |

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

50 commits, 10 tests, clippy-clean.

## License

MIT.
