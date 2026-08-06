# ARX

A keyboard-driven dual-pane terminal commander for local and remote files,
transfers, background jobs, and daily file operations. SSH/SFTP-first,
Midnight Commander parity with extra Rust-native features.

## Features

- **Dual-pane TUI** with tabs, directory history, and panel swap
- **Local + SFTP** remote browsing (respects `~/.ssh/config`, known_hosts)
- **Archive browsing** — enter `.tar.gz`/`.zip` as directories
- **Background jobs** — copy/move in background, queue with progress
- **Directory compare** — unique files highlighted, content diff with `diff -u`
- **Content diff** — compare file contents across panes (Ctrl+D + Enter)
- **bat** integration for syntax-highlighted file preview (Shift+F3)
- **User menu** with custom scripts (`~/.config/arx/arx.menu`)
- **Configurable editor** — `arx.toml` > `$EDITOR` > `$VISUAL` > `vi`
- **Content diff** across panes (Ctrl+D + Enter runs `diff -u`)
- **Extension colors** — 30+ file types color-coded in Full mode
- **F-key shortcut bar** always visible at bottom (MC-style)
- 10 tests, clippy-clean, MIT licensed

## Quick start

```bash
cargo install --git https://github.com/mrAibo/arx
arx
```

## Keybindings

### Navigation

| Key | Action |
|---|---|
| j / ↓ | Move down |
| k / ↑ | Move up |
| Enter | Enter directory / open archive / content diff |
| Backspace | Go to parent directory / exit archive |
| Tab | Switch active pane |
| Ctrl+G | Go to path |
| Ctrl+H | Toggle hidden files |
| Alt+Down | Go back in directory history |
| Ctrl+\\ | Open dir in file explorer |

### Selection

| Key | Action |
|---|---|
| Space | Toggle selection |
| * | Invert selection |
| / | Filter by substring |
| + | Select by glob |

### File operations

| Key | Action |
|---|---|
| F5 | Copy selected to other pane (background) |
| F6 | Move selected to other pane (background) |
| Shift+F6 | Rename file under cursor |
| F7 | Create directory |
| F8 | Delete selected |
| Ctrl+U | Swap panes |
| Alt+O | Sync other pane to active |

### View & Edit

| Key | Action |
|---|---|
| F3 | Quick viewer (Esc to close) |
| Shift+F3 | bat — syntax-highlighted preview |
| F4 | Edit in configured editor |
| Ctrl+I | File info (stat) |

### Tools

| Key | Action |
|---|---|
| F2 | User menu (if `arx.menu` exists) |
| F9 | Host panel (SFTP) |
| Ctrl+B | Bookmarks |
| Ctrl+D | Toggle directory diff (unique files green) |
| Ctrl+J | Job queue |
| Ctrl+O | Drop to subshell |
| Ctrl+R | Refresh panels |
| : | Shell command line (`:!cmd`) |

### Tabs

| Key | Action |
|---|---|
| Ctrl+T | New tab |
| Ctrl+W | Close tab |
| Ctrl+← / Ctrl+→ | Previous / next tab |
| Alt+1 … 9 | Jump to tab N |

### Ctrl+X prefix (MC-style)

| Key | Action |
|---|---|
| Ctrl+X S | Create symlink (`ln -s`) |
| Ctrl+X L | Create hard link (`ln`) |
| Ctrl+X C | chmod |
| Ctrl+X O | chown |

### Misc

| Key | Action |
|---|---|
| ? | Help |
| q | Quit |

## Configuration

### `~/.config/arx/arx.toml`

```toml
[ui]
show_hidden = false
editor = "hx"       # optional, overrides $EDITOR/$VISUAL
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

### `~/.config/arx/arx.menu`

```toml
# format: t  "Label"  command
t  "Tar home"  tar czf /tmp/home.tgz ~/
t  "Disk usage"  df -h
```

## MC parity

| MC feature | ARX |
|---|---|
| Dual-pane, tabs | ✅ |
| F3/F4/F5/F6/F8 | ✅ |
| Directory diff | ✅ (+ content diff) |
| User menu | ✅ |
| Bookmarks | ✅ |
| SFTP | ✅ (+ known_hosts) |
| Archive browsing | ✅ |
| Background jobs | ✅ |
| bat integration | ✅ |
| Mkdir (F7), rename (Shift+F6) | ✅ |
| File info (Ctrl+I) | ✅ |
| Drop to shell (Ctrl+O) | ✅ |
| Symlink, hardlink, chmod, chown | ✅ (Ctrl+X prefix) |
| Recursive find, mouse | ✅ |
| Panel modes (Full/Brief) | ✅ (Alt+T) |
| Extension colors | ✅ |
| Shortcut bar, full help | ✅ |
| **MC parity** | ✅ **complete** |

## Development

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo run
```

## License

MIT.
