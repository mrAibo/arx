# ARX Architecture

ARX is a Linux terminal commander for local and remote workspaces. Local filesystems,
SFTP, archives, S3-compatible object storage, and WebDAV are represented as locations
behind provider interfaces instead of being special-cased directly in the TUI.

## Architectural rules

1. The TUI does not own filesystem, SSH/SFTP, S3, WebDAV, rsync, archive, or typed Quick
   Action mutation semantics.
2. Long-running work is represented as a Job or a correlated Effect; the render state
   is never a second source of lifecycle truth.
3. When a system tool already owns a mature operation, ARX may use it behind a typed
   service/executor boundary rather than reimplementing it in the UI.
4. Destructive workspace synchronization requires a frozen Preview before execution.
5. SSH host-key verification and user SSH policy remain with OpenSSH/user
   configuration; ARX does not invent a parallel trust model.
6. VFS/provider identity, transfer planning, execution, and UI presentation remain
   separate layers.
7. Unknown, partial, unavailable, and ambiguous states remain distinct from zero or
   success.
8. Storage inspection is observability-only: it does not imply cleanup, mount, quota,
   resize, or delete authority.
9. Process/session environment is authoritative. ARX inherits variables such as
   `DISPLAY` and never synthesizes SSH/X11 forwarding state.
10. User extensibility stays narrow: `arx.menu` may define admin commands, but ARX does
    not carry an embedded Lua/WASM runtime unless a future concrete product contract
    justifies one.
11. Built-in Quick Actions are typed product actions, not shell aliases: provider
    availability, frozen targets, cancellation, mutation boundaries, and terminal
    outcomes are explicit.

## Layering

```text
Terminal events
      │
      ▼
Input / Keymap ──▶ Action availability
      │
      ▼
   AppState  ─────────────── render snapshots / overlay state
      │
      ▼
 Dispatcher / Controllers
      │
 ┌────┼─────────────┬────────────────────┐
 ▼    ▼             ▼                    ▼
Effect Service      JobManager           ProviderRegistry
 │      │             │                    │
 ▼      ▼             ├─ TransferQueue     ├─ Local
lanes  Process/       ├─ Workspace Sync    ├─ SFTP
      system tools    └─ Storage Scan      ├─ Archive
                                            ├─ S3
                                            └─ WebDAV
```

`AppState.jobs` is a render snapshot. The runtime `JobManager` is lifecycle truth.
Transfer Center, Jobs, Storage Inspector, and Remote Workspace observe or control that
same runtime rather than maintaining independent job state machines.

## Input, actions, and availability

Terminal events enter through the async event loop. `KeyRouter` resolves raw input to
product actions or overlay-owned controls. Availability is derived from the active
location, focused entry, provider capabilities, provider policy, and current UI state.
An unavailable operation must fail closed before provider mutation code is reached.

Some overlays intentionally own keyboard input while open. For example, Transfer
Center, Storage Inspector, and Filesystems consume their navigation/control keys so
browser actions do not fall through underneath them.

## Effects, services, and jobs

| Pattern | Used for | Lifecycle truth |
|---|---|---|
| **Effects** | Correlated interactive async work such as Preview, Remote Edit, and Quick Action requests | Effect lane / request id |
| **Services** | Process/system integration and bounded request/result operations, including `QuickActionService` | Typed service result |
| **Jobs** | Transfers, workspace sync, storage scans, and other long-running job work | Single runtime `JobManager` |

`EffectDispatcher` uses dedicated lanes with monotonically changing request identity,
cancellation ownership, and stale-response rejection. Superseded interactive work
cannot silently overwrite newer UI intent.

Remote Edit lifecycle integration with the runtime JobManager is implemented. It is
not a deferred architecture item: observers publish into the same manager/channel that
drives Jobs and the render snapshot.

## Typed local Quick Actions

PACK O adds three built-in local-only actions through the same shared
`Action` / `ActionId` / `ACTION_CATALOG` / availability model used by Command Center:

- **Compute SHA-256** — focused/selected regular files are hashed in Rust with `sha2`
  inside a blocking worker. Filenames remain data, never shell syntax.
- **Touch file** — the prompt freezes the Local directory, accepts exactly one child
  name, rejects traversal/absolute paths, opens with `O_NOFOLLOW`, verifies the opened
  target is a regular file, and applies `futimens` to that exact descriptor. Cancellation
  is honored before the create/open mutation boundary; after that boundary ARX finishes
  and reports the real terminal outcome.
- **Compress to tar.gz** — the prompt freezes the Local directory and selected/focused
  names. `QuickActionService` invokes system `tar` through typed argv, places `--`
  before user filenames, sets `kill_on_drop(true)`, races execution against the
  cancellation token with `tokio::select!`, stages in the destination directory, and
  finalizes with `persist_noclobber`.

All three actions fail closed outside `ProviderId::Local`; there is no attempt to map
local shell/filesystem semantics onto SFTP, Archive, S3, or WebDAV. Touch and Compress
prompts reuse the existing command-input buffer but own a typed frozen prompt state, so
pane/selection changes cannot silently retarget an operation after it starts.

`EffectLane::QuickAction` owns correlated execution. A second Quick Action is rejected
while one is pending. Terminal results are accepted even if the user navigated away;
Touch/Compress completion paths refresh only panes that are still at the frozen origin.
Quit requests cancellation and remains blocked until the lane reports a safe terminal
outcome. SHA-256 is cooperatively cancellable; the tar subprocess is killed on dropped
execution. Control characters in filenames, paths, and tool errors are escaped before
presentation while printable Unicode remains intact.

This is a typed built-in feature boundary, not a plugin or arbitrary command runtime.
The only external tool in the three actions is system `tar`; if it is unavailable ARX
returns a typed `ToolUnavailable` failure.

## Environment and extension boundaries

ARX does not establish an SSH X11 forwarding channel and does not infer one from
terminal-brand or `SSH_CLIENT` heuristics. If an SSH client/server session established
forwarding, the resulting `DISPLAY` is inherited unchanged by ARX and child processes.
If `DISPLAY` is absent, ARX leaves it absent.

The supported lightweight admin extension surface is `~/.config/arx/arx.menu`, whose
entries are exposed through Command Center. PACK N removed the previously unwired Lua
prototype and `mlua` runtime. There is no supported Lua or WASM plugin API in the
current architecture.

## Provider layer

`VfsProvider` plus `ProviderRegistry` is the provider boundary. Current providers are:

- **Local** — native filesystem operations.
- **SFTP** — SSH/SFTP browsing and mutation semantics, including transactional copy
  and conflict-safe text Remote Edit.
- **Archive** — archive browsing/preview semantics.
- **S3** — AWS-shaped object-storage provider with provider-native bucket/prefix/object
  identity.
- **WebDAV** — HTTP/WebDAV provider using real DAV methods and href identity.

Providers expose capabilities, but capability alone is not always sufficient for an
operation. Provider policy is also authoritative. For example, S3 and WebDAV may have
Read+Write capability while F4 remote editing remains intentionally disabled.

Operational identity is never reconstructed from display text. S3 object keys and
WebDAV hrefs retain provider-native identity; `Entry.name` is presentation.

## Transfer planning and execution

`TransferPlanner` builds an immutable/frozen `TransferPlan` from a typed request. It
selects an executor according to the source/destination pair and proven capabilities,
including native local paths, rsync where appropriate, SFTP streaming, S3, and
WebDAV-supported paths. A queued plan does not mutate underneath the executor.

The persistent `TransferQueueRuntime` is the single transfer scheduler/executor owner.
It provides bounded FIFO concurrency and integrates with the same JobManager used by
the rest of the product.

Transfer safety rules include:

- cooperative Pause/Resume at safe checkpoints;
- cancellation of one transfer does not cancel unrelated jobs;
- automatic retry is bounded to at most three total attempts;
- automatic replay is allowed only for failures explicitly classified
  `SafeToRetry`;
- ambiguous remote mutations and `RecoveryRequired` outcomes are never blindly
  replayed;
- queue workers/retry timers are owned and joined during shutdown.

Transfer Center v2 (`Ctrl+Y`) is a control/presentation surface over that runtime. Its
Active / History / All views do not create a second scheduler. Pause/Resume/Cancel call
the runtime; terminal history remains terminal JobManager truth.

## Remote Workspace

Remote Workspace keeps four separate truths:

1. **Compare** (`Ctrl+D`) — recursively scan both roots and produce a workspace diff.
2. **Preview** (`Ctrl+X P`) — freeze direction, update/mirror mode, copies, deletes,
   conflicts, and known transfer size before mutation.
3. **Execute** — queue the frozen work through the existing job/transfer runtime.
4. **Verify** — independently rescan after execution and report `Synchronized`,
   `DifferencesRemain`, or `Inconclusive`.

A successful executor return is not automatically a synchronization verdict.
Verification remains a separate evidence step.

## Local Storage Inspector (`Alt+U`)

The Linux Storage Inspector is read-only and runs expensive traversal outside the UI
thread. Its core uses `dua-core` for parallel enumeration while ARX owns product
semantics around the scan.

Important invariants:

- logical/apparent bytes and allocated/on-disk bytes remain separate;
- sparse-file truth is preserved;
- symlinks are reported but never followed;
- hard links are de-duplicated by `(dev, ino)` where applicable;
- top-file/subtree ordering is deterministic;
- traversal/stat/permission errors are evidence and make the result partial rather
  than silently exact;
- progress is observed entries/bytes with unknown total — no fabricated percentage or
  ETA;
- cancellation uses the token owned by the created `StorageScan` JobManager job;
- completed typed scan snapshots back the drill-down UI instead of serializing the
  result into generic status text.

The overlay exposes drill-down, size basis, sorting, and top-file views only. It does
not expose cleanup/delete actions.

## Filesystems (`Alt+D`)

The Linux Filesystems view is also read-only.

- `/proc/self/mountinfo` is the mount-topology source; ARX does not shell out to `df`,
  `findmnt`, or `mount` for core truth.
- `statvfs` supplies block and inode capacity statistics.
- byte arithmetic uses wide integer semantics and keeps total/free/available/reserved
  distinct.
- autofs entries are not probed in the default snapshot, avoiding observation-triggered
  automount side effects.
- inaccessible/broken mounts remain visible with typed unavailable evidence instead of
  failing the entire snapshot.
- sorting/filtering is deterministic and refresh is explicit/manual only.

This is local Linux filesystem evidence. ARX does not project POSIX capacity semantics
onto S3, SFTP, or WebDAV when those providers cannot prove them.

## SFTP safety and connection pooling

SFTP connections are pooled per host. Ambiguous transport failures invalidate the
session; definitive protocol-level errors do not automatically invalidate a healthy
session.

Transactional SFTP copy stages before commit and retains rollback/recovery evidence.
Remote Edit uses bounded download, binary/NUL refusal, immutable revision/conflict
checks, and atomic-style stage → backup → commit behavior where the provider path
supports it.

## S3 architecture

S3 is object storage, not a POSIX filesystem.

- `S3BucketRef`, `S3PrefixRef`, and `S3ObjectRef` preserve provider-native strings.
- prefixes are key namespaces, not directories; ARX does not normalize `//`, `.`, or
  `..` as filesystem path components.
- target-scoped providers use lazy clients; bucket-bound targets do not gain broader
  bucket-list authority.
- multipart/cancellation/retry behavior preserves mutation ambiguity rather than
  claiming success without evidence.
- ETag is not treated as a universal content hash.

Current supported MVP evidence covers AWS S3 and MinIO physically; Moto is emulated.
Cloudflare R2 and Wasabi remain unverified best-effort targets.

## WebDAV architecture

WebDAV is a production provider, not a stub. It uses real DAV semantics over HTTP,
including PROPFIND / GET / PUT / DELETE / MKCOL and provider server-side COPY/MOVE
where the product path permits it.

Key boundaries:

- authoritative raw-href identity is preserved;
- bounded parsing and preview avoid unbounded response handling;
- HTTP Basic secrets come from OS keyring/environment, never plaintext config;
- overwrite-forbid uses `If-None-Match: *` where supported;
- local downloads stage in the destination directory and use noclobber finalization;
- automatic HTTP mutation retry is disabled so ambiguous mutations are not replayed
  blindly.

Apache `mod_dav` is the physically accepted MVP target. Nextcloud/ownCloud remain
unverified and Digest/Bearer authentication is deferred.

## Release and packaging architecture

GitHub Release is the single publication path for the Linux x86_64 binary. Release
validation builds `target/release/arx` exactly once, then packages that exact ELF into:

- portable tar.gz;
- Debian `.deb`;
- RPM `.rpm`.

The workflow extracts all three formats and verifies that each packaged executable
hash matches the original release ELF. Exact payload manifests reject unexpected files
or symlinks. `cargo-about` generates third-party license notices, and one
`SHA256SUMS` covers all three package artifacts. The publish job reuses validated
artifacts without rebuilding Rust code.

## Current source areas

```text
src/
├── main.rs / tui.rs
├── app/                    # AppState, action availability, typed prompt/overlay state
├── input/                  # Keymap, command/hint discovery
├── effects.rs / effect_dispatcher.rs
├── services/               # typed service/controller boundaries, QuickActionService
├── jobs/                   # runtime JobManager and typed job truth
├── transfer/               # plans and transfer executors
├── transfer_queue_runtime.rs
├── transfer_center_ui.rs
├── storage_inspector.rs
├── storage_inspector_ui.rs
├── filesystem_usage.rs
├── filesystem_usage_ui.rs
├── vfs/                    # Local / SFTP / Archive / S3 / WebDAV providers
├── remote/                 # SSH host/config transport integration
├── process/                # process / Remote Edit / Effect service routing
├── config.rs
└── terminal.rs             # embedded PTY terminal
```

## Testing

The release quality contract includes:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --locked --all-features
cargo +1.88 check --locked --all-features
```

PACK O additionally exercises typed Quick Action availability/discovery, SHA-256 known
vectors, Touch no-follow/pre-mutation cancellation behavior, control-safe presentation,
and real system-`tar` archive/noclobber behavior on Linux.

Physical CI additionally exercises the accepted WebDAV path and Transfer Queue real
MinIO safe-read retry path. Release CI separately validates third-party licensing,
package metadata/payload, binary identity, and checksums.
