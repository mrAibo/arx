# ARX

Terminal commander for local ↔ remote workspaces on Linux.

![ARX Remote Workspace — Compare, Preview, Sync, Verify](docs/assets/remote-workspace-update.gif)

**Current release: [v0.25.0](https://github.com/mrAibo/arx/releases/tag/v0.25.0)**

Linux x86_64 · Rust MSRV 1.88 · MIT

Compare before touching anything. Preview the exact consequences. Execute. Verify the real result.

## Why ARX?

- **Local ↔ remote as one workspace.** Work with Local, SFTP, S3, WebDAV, and archives from one dual-pane TUI.
- **Compare before execution.** Workspace diff is a separate fact; a comparison never silently becomes a mutation.
- **Preview exact consequences.** Destructive workspace sync requires a frozen Preview before execution.
- **Truthful background work.** Queue, pause-pending, paused, retry-waiting, cancelled, failed, completed, and verification remain distinct states.
- **Safe remote semantics.** ARX prefers noclobber/staged operations and never blindly replays ambiguous remote mutations.
- **Provider-native identity.** Existing remote resources are addressed by provider truth, not reconstructed display names.
- **Read-only storage intelligence.** `Alt+U` provides truthful Local usage analysis and exact S3 object plus bounded bucket/prefix inspection without inventing filesystem-capacity semantics.

Published **v0.25.0** adds the accepted WebDAV transaction surface from [#275](https://github.com/mrAibo/arx/issues/275) / [#276](https://github.com/mrAibo/arx/pull/276), [#277](https://github.com/mrAibo/arx/issues/277) / [#278](https://github.com/mrAibo/arx/pull/278), and [#279](https://github.com/mrAibo/arx/issues/279) / [#280](https://github.com/mrAibo/arx/pull/280): recursive same-target/cross-target WebDAV Copy to a new root, multi-root recursive WebDAV Delete, and verified WebDAV → WebDAV Move through copy → verify → frozen-source delete semantics.

**v0.25.0 retains SFTP → SFTP Workspace Sync from v0.24.0** and the read-only S3 Object & Bucket Inspector shipped in v0.23.0. The same singular `ProviderRegistry`, Transfer Queue / Job lifecycle, retry/recovery authority, and provider-native identity model remain authoritative.

## Platform support

ARX is intentionally a **Linux application**. The published release target is **Linux x86_64**. Native Windows support is not planned.

When ARX runs inside SSH, the established session environment is authoritative. ARX preserves an existing `DISPLAY` but never invents one; X11 forwarding must already be provided by the SSH client/session.

## Install v0.25.0

Published binaries and packages live under **GitHub Releases**, not in the source tree. The repository intentionally does not track a current `bin/arx` copy.

Download your preferred artifact and `SHA256SUMS` from the [v0.25.0 release](https://github.com/mrAibo/arx/releases/tag/v0.25.0).

Verify downloaded artifacts:

```bash
sha256sum --ignore-missing -c SHA256SUMS
```

### Debian / Ubuntu

```bash
sudo apt install ./arx_0.25.0_amd64.deb
```

### Fedora / RHEL family

```bash
sudo dnf install ./arx-0.25.0-1.x86_64.rpm
```

### Portable tarball

```bash
tar xzf arx-v0.25.0-x86_64-unknown-linux-gnu.tar.gz
sudo install -m 755 arx-v0.25.0-x86_64-unknown-linux-gnu/arx /usr/local/bin/arx
arx --version
```

Expected output:

```text
arx 0.25.0
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
| Workspace Sync | Compare → Preview → Execute → Verify; v0.25.0 retains Local→Local, Local↔SFTP, and same-host/cross-host SFTP→SFTP sync |
| Storage Inspector (`Alt+U`) | Local: read-only `du++`-style logical/allocated usage, drill-down/top files/cancellation. S3: exact object inspection plus bounded paginated bucket/prefix LiveScan analytics |
| Filesystems | Read-only Linux `df++`-style capacity/inode view; S3 never receives fake `df` semantics |
| Effective keymap | Conflict-safe user overrides + `arx --print-keymap` |
| Quick Actions | Typed local SHA-256, Touch, Compress to tar.gz; existing mkdir/chmod/symlink surface retained |
| Embedded terminal | Built-in terminal plus hardened tmux / GNU Screen lifecycle |
| S3 | AWS S3 + MinIO physically accepted; v0.25.0 retains the exact Object & Bucket/Prefix Inspector shipped in v0.23.0; Moto emulated; R2/Wasabi best-effort/unverified |
| WebDAV | Apache mod_dav + Nextcloud 34.0.2 + ownCloud 11.0.0 physically accepted; v0.25.0 adds recursive WebDAV→WebDAV Copy, multi-root recursive Delete, and verified WebDAV→WebDAV Move |
| Packages | tar.gz + `.deb` + `.rpm` + one `SHA256SUMS`, all published from one validated ELF |
| Extension surface | `arx.menu`; no embedded Lua/WASM/native plugin runtime |

## File operations

| Key | Action |
|---|---|
| **F3** | View — Local full preview; SFTP bounded text; S3 bounded object preview; WebDAV bounded one-file preview |
| **F4** | Edit — Local editor; SFTP conflict-safe text edit; S3/WebDAV disabled |
| **F5** | Copy — Local↔Local, Local↔SFTP, Local↔S3 single object, Local↔WebDAV file/tree copy; WebDAV↔Local supports multiple selected sibling roots; WebDAV→WebDAV supports one exact recursive collection copied to one new same-target/cross-target root |
| **F6** | Move — Local↔Local product path; S3 disabled; WebDAV→WebDAV supports one exact collection through verified copy → revalidate → frozen-source delete, same-target or cross-target |
| **F7** | Create directory — Local/SFTP, S3 prefix marker, WebDAV MKCOL |
| **F8** | Delete — Local trash; confirmed SFTP delete; exact S3 object / proven-empty marker delete; WebDAV supports safe bounded recursive delete for one or more exact selected collection roots |
| Shift+F6 | Rename |
| Ctrl+U | Swap panes |
| Ctrl+X C / L / O / S | chmod / hardlink / chown / symlink |
| Ctrl+\\ | Toggle split pane |

## WebDAV transaction surface in v0.25.0

v0.25.0 publishes the post-v0.24 WebDAV work as one coherent safety model rather than as server-side operation claims that do not hold across unrelated targets.

### Recursive WebDAV → WebDAV Copy

One exact current-listed WebDAV collection can be copied to one new WebDAV destination root on the same configured target or another target. ARX freezes and bounds the source manifest, revalidates it before mutation, streams file data through bounded WebDAV → ARX → WebDAV mediation, and independently verifies the destination before reporting success. Ambiguous destination mutation is never blindly replayed.

### Multi-root recursive WebDAV Delete

F8 can delete one or more exact selected current WebDAV collection roots as one mutation job. ARX completes aggregate bounded planning and whole-batch revalidation before the first DELETE, executes roots deterministically and sequentially, keeps descendants child-first/root-last, proves each collection empty immediately before deleting it, and reports truthful global item progress. Definitive later failure/cancellation preserves partial truth; ambiguous DELETE becomes `RecoveryRequired` without replay.

### Verified WebDAV → WebDAV Move

F6 Move for one exact WebDAV collection uses the explicit transaction:

`freeze source → bounded copy → independently verify destination → revalidate source + destination → delete exact already-frozen source manifest`

Copy success alone is never Move success. Source drift before the first source DELETE leaves source untouched and only cleans a still-proven attempt-owned destination. Destination drift or unresolved mutation certainty requires recovery. Once source deletion starts, the verified destination is committed; later definitive failure or cancellation remains truthful partial state and is never rolled back or blindly retried.

### URI identity interoperability

Apache physical acceptance exposed that valid URI percent-escape hex digits may differ only in case. Canonical matching therefore treats `%C3%A9` and `%c3%a9` as equivalent while never percent-decoding path bytes, collapsing separators, or dropping query identity. Provider/server-returned href identity remains authoritative.

Physical acceptance for the published WebDAV path includes Apache mod_dav destructive/fault matrices plus portable Nextcloud 34.0.2 and ownCloud 11.0.0 interoperability. The implementation reuses the existing `ProviderRegistry`, Transfer Queue / Job lifecycle, mutation seams, retry/recovery model, and secret authority.

Still intentionally unsupported: multi-root WebDAV→WebDAV Move, overwrite/merge/update/mirror semantics for recursive WebDAV→WebDAV Copy/Move, file or mixed file/directory WebDAV Move, Digest/Bearer auth without concrete interoperability need, and metadata/property mutation without a demonstrated admin use case.

## S3 Object & Bucket Inspector in v0.23.0

v0.23.0 adds read-only S3 storage intelligence without treating object storage like a POSIX filesystem.

### Exact object inspection

For one exact provider-native S3 object, ARX uses `HeadObject` and shows only facts the backend actually returns: target/bucket/key identity, size, last modified, ETag, content type, storage class, metadata, endpoint override, and version ID when available. Missing values remain unavailable rather than being synthesized.

### Bounded bucket / prefix inspection

Bucket and prefix inspection uses paginated `ListObjectsV2` LiveScan evidence with progress and cooperative cancellation. It reports observed object count, logical bytes, bounded largest-object ranking, bounded immediate-prefix ranking, age distribution, and storage-class distribution. Prefix and storage-class cardinality are explicitly bounded so large or unusual S3-compatible backends cannot turn the inspector into an unbounded in-memory inventory.

The implementation reuses the existing `ProviderRegistry`, per-target `S3Provider`/AWS client, `JobManager`, and UI authority. It does not create a second scheduler, registry, client cache, or mutation path.

Physical acceptance on the merged implementation includes a real MinIO path proving exact object inspection and prefix inspection. AWS S3 remains the supported product path; Cloudflare R2 and Wasabi remain best-effort/unverified until separately evidenced.

## WebDAV retained from v0.22.0

v0.25.0 retains the recursive Local↔WebDAV surface introduced in v0.22.0 while keeping the same planner/queue/provider authorities and exact-identity safety model.

### Recursive WebDAV → Local download

One selected WebDAV collection can be copied to one new Local directory tree. ARX uses exact server-returned href identities, bounded authenticated `PROPFIND Depth: 1`, complete manifest validation before Local mutation, staged noclobber downloads, and truthful cleanup/recovery outcomes.

### Recursive Local → WebDAV upload

One Local directory can be copied as one new remote tree. ARX pre-scans the Local tree, rejects symlinks/special/unsafe names, uses bounded depth/count limits, root-relative no-follow reads, destination-root noclobber semantics, and recovery-required classification for ambiguous remote mutation outcomes.

### Safe recursive WebDAV delete foundation

The original one-root recursive delete path builds and revalidates a complete bounded manifest, deletes deepest-first/root-last, performs a fresh exact empty proof before each collection DELETE, and never retries ambiguous destructive requests automatically. v0.25.0 extends this authority to aggregate multi-root deletion rather than introducing a second delete engine.

### Multi-root Local ↔ WebDAV F5 Copy

Multiple selected current sibling roots can be copied as one queued job in both Local → WebDAV and WebDAV → Local directions. Mixed files/directories are supported. Roots execute sequentially, progress is reported at root granularity, and earlier completed roots remain truthful if a later root fails or is cancelled.

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
| Alt+U | Storage Inspector — Local usage or S3 object/bucket/prefix inspection |
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
- S3 remains object storage; ARX does not fabricate POSIX filesystem-capacity semantics for it.

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
- [docs/releases/v0.25.0.md](docs/releases/v0.25.0.md) — v0.25.0 release notes
- [GitHub Releases](https://github.com/mrAibo/arx/releases) — published binaries and checksums

## License

MIT
