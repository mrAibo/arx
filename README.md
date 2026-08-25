# ARX

Terminal commander for local ↔ remote workspaces on Linux.

![ARX Remote Workspace — Compare, Preview, Sync, Verify](docs/assets/remote-workspace-update.gif)

**Current release: [v0.21.0](https://github.com/mrAibo/arx/releases/tag/v0.21.0)**  
Linux x86_64 · Rust MSRV 1.88 · MIT

Compare before touching anything. Preview the exact consequences. Execute. Verify the real result.

## Why ARX?

- **Local ↔ remote as one workspace.** Work with Local, SFTP, S3, WebDAV, and archives from one dual-pane TUI.
- **Compare before execution.** Workspace diff is a separate fact; a comparison never silently becomes a mutation.
- **Preview exact consequences.** Destructive workspace sync requires a frozen Preview before execution.
- **Truthful background work.** Queue, pause-pending, paused, retry-waiting, cancelled, failed, completed, and verification remain distinct states.
- **Safe remote semantics.** ARX prefers noclobber/staged operations and does not blindly replay ambiguous remote mutations.
- **Read-only storage visibility.** Inspect local directory usage and Linux filesystem capacity without turning observability into cleanup.
- **Progressive discovery.** Command Center, effective keymap output, contextual availability, and the footer derive from runtime truth.

Remote Workspace synchronization currently supports Local → Local, Local → SFTP, and SFTP → Local. SFTP → SFTP workspace synchronization remains intentionally unsupported.

## Platform support

ARX is intentionally a **Linux application**. The published release target is **Linux x86_64**. Native Windows support is not planned.

When ARX runs inside SSH, the established session environment is authoritative. ARX preserves an existing `DISPLAY` but never invents one; X11 forwarding must already be provided by the SSH client/session.

## Install v0.21.0

Download your preferred package and `SHA256SUMS` from the [v0.21.0 release](https://github.com/mrAibo/arx/releases/tag/v0.21.0).

Verify downloaded artifacts:

```bash
sha256sum --ignore-missing -c SHA256SUMS
```

### Debian / Ubuntu

```bash
sudo apt install ./arx_0.21.0_amd64.deb
```

### Fedora / RHEL family

```bash
sudo dnf install ./arx-0.21.0-1.x86_64.rpm
```

### Portable tarball

```bash
tar xzf arx-v0.21.0-x86_64-unknown-linux-gnu.tar.gz
sudo install -m 755 arx-v0.21.0-x86_64-unknown-linux-gnu/arx /usr/local/bin/arx
arx --version
```

Expected output:

```text
arx 0.21.0
```

Release bundles include the MIT license and generated third-party license notices. Native packages install documentation under `/usr/share/doc/arx`.

### From source

Rust 1.88+ and system OpenSSH are required. The built-in **Compress to tar.gz** Quick Action additionally requires a system `tar` executable.

```bash
cargo install --git https://github.com/mrAibo/arx
arx
```

## 60-second Remote Workspace workflow

1. Put the source workspace in one pane and the destination in the other, for example `~/code/app` and an SFTP host path.
2. Press **Ctrl+D** to compare both roots.
3. Press **Ctrl+X P** to open Workspace Sync Preview.
4. Review the frozen plan and choose Update or Mirror deliberately.
5. Press **Enter** to execute through the existing Job Manager / Transfer Queue.
6. After completion, ARX performs a separate verification scan.
7. Trust the final verdict: `Synchronized`, `DifferencesRemain`, or `Inconclusive`.

## Key capabilities

| Area | Current product truth |
|---|---|
| Dual-pane TUI | Tabs, history, swap, mouse, split panes |
| Split panes | Vertical + horizontal, explicit close, keyboard ratio resize (20–80), section-aware same-location mouse behavior |
| Local / SFTP | Browsing, transactional file copy, SFTP bounded preview, conflict-safe text Remote Edit |
| Transfer Queue | Bounded FIFO, concurrency 1..=8, progress/rate/ETA where known, Pause/Resume/Cancel, safe retry ≤3 attempts |
| Transfer Center | Active / History / All views with runtime-owned controls |
| Workspace Sync | Compare → Preview → Execute → Verify for supported Local/SFTP directions |
| Local Storage Inspector | Read-only `du++`-style logical/allocated usage, drill-down, top files, cancellation |
| Filesystems | Read-only Linux `df++`-style capacity/inode view |
| Effective keymap | Conflict-safe user overrides + `arx --print-keymap` |
| Quick Actions | Typed local SHA-256, Touch, Compress to tar.gz; existing mkdir/chmod/symlink surface retained |
| Embedded terminal | Built-in terminal plus hardened tmux / GNU Screen lifecycle |
| S3 | AWS S3 + MinIO physically accepted; Moto emulated; R2/Wasabi best-effort/unverified |
| WebDAV | Apache mod_dav + Nextcloud 34.0.2 + ownCloud 11.0.0 physically accepted with Basic auth |
| Packages | tar.gz + `.deb` + `.rpm` + one `SHA256SUMS`, all from one validated ELF |
| Extension surface | `arx.menu`; no embedded Lua/WASM plugin runtime |

## File operations

| Key | Action |
|---|---|
| **F3** | View — Local full preview; SFTP bounded text; S3 bounded object preview; WebDAV bounded one-file preview |
| **F4** | Edit — Local editor; SFTP conflict-safe text edit; S3/WebDAV disabled |
| **F5** | Copy — Local↔Local, Local↔SFTP, Local↔S3 single object, Local↔WebDAV one file; one exact selected WebDAV collection may be recursively downloaded to one new Local tree |
| **F6** | Move — Local↔Local product path; S3 disabled; cross-target WebDAV move unsupported |
| **F7** | Create directory — Local/SFTP, S3 prefix marker, WebDAV MKCOL |
| **F8** | Delete — Local trash; confirmed SFTP delete; exact S3 object / proven-empty marker delete; WebDAV non-recursive resource delete |
| Shift+F6 | Rename |
| Ctrl+U | Swap panes |
| Ctrl+X C / L / O / S | chmod / hardlink / chown / symlink |
| Ctrl+\\ | Toggle split pane |

### WebDAV recursive download in v0.21.0

ARX can copy **one selected WebDAV collection → one new Local directory tree** through the existing F5 → `TransferPlanner` → Transfer Queue → WebDAV executor path.

The implementation keeps provider-native `WebDavCollectionRef` / `WebDavObjectRef` href identity authoritative, uses bounded authenticated `PROPFIND Depth: 1`, builds the complete manifest before Local mutation, rejects unsafe/duplicate/cyclic identities and Local path components, stages files with noclobber semantics, and removes the attempt-owned Local root on failure/cancellation when possible.

Physically accepted targets for this same product path:

| Backend | Status |
|---|---|
| Apache mod_dav | ✅ W1–W18 + recursive download |
| Nextcloud 34.0.2-apache | ✅ I1–I12 + recursive download |
| ownCloud 11.0.0 | ✅ I1–I12 + recursive download |

Intentionally not shipped yet: Local directory → WebDAV recursive upload, recursive WebDAV delete, WebDAV→WebDAV recursive/cross-target operations, multiple recursive roots, Digest/Bearer auth, and metadata/property mutation. These remain tracked under [#13](https://github.com/mrAibo/arx/issues/13).

## Navigation and discovery

| Key | Action |
|---|---|
| j / ↓ / k / ↑ | Move cursor |
| Enter | Enter directory / archive / content diff |
| Backspace | Parent / exit archive |
| Tab | Switch active pane |
| Ctrl+G | Go to path |
| Ctrl+H | Toggle hidden files |
| Alt+Down / Alt+Up | Directory history |
| Space | Toggle selection |
| Shift+Left-click | Inclusive real-row range selection |
| Right-click | Provider-aware typed context menu |
| Ctrl+P | Command Center |
| Ctrl+J | Jobs |
| Ctrl+Y | Transfer Center |
| Alt+U | Local Storage Inspector |
| Alt+D | Filesystems |
| Ctrl+X T | Embedded Terminal |
| ? | Help |

The runtime keymap is authoritative. Inspect the effective bindings with:

```bash
arx --print-keymap
```

## Configuration

### `~/.config/arx/arx.toml`

```toml
[ui]
show_hidden = false
editor = "hx"

[transfer]
concurrency = 2
```

Transfer concurrency defaults to 2 and must remain within `1..=8`.

### Configurable keybindings

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

Exactly one of `keys` or `disabled = true` is required per entry. Duplicate or prefix-conflicting sequences inside one input context fail configuration instead of silently choosing a winner. Only KeyRouter-managed action/context pairs are configurable.

### `~/.config/arx/hosts.toml`

```toml
[[hosts]]
id = "nuc"
name = "Headless NUC"
hostname = "192.168.1.10"
user = "aibo"
default_path = "/home/aibo"
```

Hosts use the user's OpenSSH configuration, including aliases, ProxyJump, IdentityFile, custom ports, host-key policy, and agent behavior.

### `~/.config/arx/arx.menu`

```text
t  "Tar home"  tar czf /tmp/home.tgz ~/
t  "Disk usage"  df -h
```

`arx.menu` is the supported lightweight admin extension surface. ARX does not ship an embedded Lua, WASM, or native plugin runtime.

## Safety model

- Destructive workspace sync requires Preview.
- Provider-native identity is authoritative; display names do not reconstruct remote addresses.
- Transfer retries are phase-aware and bounded; ambiguous remote mutations and recovery-required outcomes are not blindly replayed.
- WebDAV automatic HTTP mutation retries remain disabled.
- Secrets stay in OS keyring/environment rather than plaintext target config.
- Long-running operations reuse the existing Effect / Job / Transfer Queue authorities instead of creating parallel schedulers or lifecycle models.

## Development

```bash
cargo fmt --check
cargo check --locked --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-features
cargo +1.88 check --locked --all-features
git diff --check
```

Provider behavior changes additionally require the relevant physical acceptance lanes.

## Project docs

- [ROADMAP.md](ROADMAP.md) — current product truth and future roadmap
- [ARCHITECTURE.md](ARCHITECTURE.md) — architecture contracts and authority boundaries
- [docs/DEVELOPMENT_HANDOFF.md](docs/DEVELOPMENT_HANDOFF.md) — continuation rules for development sessions
- [docs/releases/v0.21.0.md](docs/releases/v0.21.0.md) — v0.21.0 release notes
- [GitHub Releases](https://github.com/mrAibo/arx/releases) — published binaries and checksums

## License

MIT
