# ARX

ARX is a Rust-based terminal commander for local and remote files, archives, transfers, synchronization, and asynchronous jobs.

The project is a ground-up redesign of the earlier `arx-universal-packer` codebase. The new architecture treats local filesystems, SFTP locations, and archives as backends behind a common VFS contract, while transfer operations are planned independently and may use native I/O, `rsync`, or SFTP depending on capabilities.

## Goals

- Fast keyboard-driven dual-pane TUI
- Local and remote filesystem browsing
- SSH/SFTP integration
- `rsync` as the preferred optimized transfer engine when available
- Automatic transfer fallback when remote `rsync` is unavailable
- Background jobs with progress, cancellation, retry, and logs
- Saved remote hosts with nested groups/tags and favorites
- Reuse of `~/.ssh/config`, ssh-agent, known_hosts, and OpenSSH tooling
- Archive browsing and manipulation through the same location model
- Directory synchronization with preview before destructive actions
- External terminal/editor/pager integration first; embedded terminal later

## Architecture

The core model is:

```text
TUI
 ↓
Actions
 ↓
Services / Planner
 ↓
VFS + Transfer + Jobs + Remote
 ↓
Local FS / SFTP / rsync / archive tools
```

The TUI must not call `std::fs`, `ssh`, `rsync`, or archive tools directly.

See [ARCHITECTURE.md](ARCHITECTURE.md) and [ROADMAP.md](ROADMAP.md).

## Foundation stack

- Rust 2024 edition
- Ratatui
- Crossterm
- Tokio
- Serde + TOML
- tracing
- thiserror

Remote crates such as `russh` and `russh-sftp` will be introduced only when their modules are implemented.

## Status

Project foundation phase. The previous Go/Bash implementation remains available in `mrAibo/arx-universal-packer` until this repository reaches feature parity.

## Development

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo run
```

## License

MIT.
