# ARX

A keyboard-driven dual-pane terminal commander for local and remote files,
transfers, and background jobs. SSH/SFTP-first, respects `~/.ssh/config`.

## Features

- Dual-pane TUI with tabs
- Local filesystem browsing, selection, filtering
- SFTP remote browsing with key or password auth
- Host manager — load hosts from `~/.config/arx/hosts.toml`
- File operations: copy, move, delete
- Directory compare (Ctrl+D)
- Internal file viewer
- External editor integration (`$EDITOR`)
- User menu with custom scripts
- Bookmarks
- Job queue
- Command line shell (`:!cmd`)
- Tab support per pane

## Quick start

```bash
cargo run
```

## Keybindings

### Navigation

| Key | Action |
|---|---|
| j / ↓ | Move down |
| k / ↑ | Move up |
| Enter | Enter directory / SFTP host |
| Backspace | Go to parent directory |
| Tab | Switch pane |
| Ctrl+G | Go to path |
| Ctrl+H | Toggle hidden files |

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
| F5 | Copy selected to other pane |
| F6 | Move selected to other pane |
| F8 | Delete selected |

### Tools

| Key | Action |
|---|---|
| F2 | User menu (if `arx.menu` exists) |
| F3 | View file |
| F4 | Edit in `$EDITOR` |
| F9 | Host panel |
| Ctrl+B | Bookmarks |
| Ctrl+D | Directory compare |
| Ctrl+J | Job queue |
| : | Run shell command |

### Tabs

| Key | Action |
|---|---|
| Ctrl+T | New tab |
| Ctrl+W | Close tab |
| Ctrl+← / Ctrl+→ | Switch tab |

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

```
# format: t  "Label"  command
t  "Tar home"  tar czf /tmp/home.tgz ~/
t  "Disk usage"  df -h
```

## Development

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo run
```

## License

MIT.
