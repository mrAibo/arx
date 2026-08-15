# ARX Architecture

ARX is a terminal commander. It treats local filesystems, remote SSH/SFTP connections, and archives as locations behind a common API.

## Rules

1. The TUI never calls filesystem, SSH, rsync, or archive code directly.
2. Long-running operations are represented as jobs or named effects.
3. When the system tool does the job, use it.
4. Destructive syncs need a frozen preview.
5. SSH connections go through OpenSSH — ARX never invents its own auth.
6. VFS and transfer layers stay separate. Host grouping is many-to-many.

## Layering

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
 (Local/SFTP/Archive)
```

### Keymap / Input

Terminal events enter through `tokio::select!`. Keymap resolves raw input to `Action` values. Handler dispatch respects availability gates — an action that isn't available for the current pane never reaches its handler.

### Availability

`Availability` maps actions to available target panes. Local vs SFTP, source vs destination, mutual exclusion — each gets an availability rule. The TUI checks availability before every handler so disabled actions never activate.

### Dispatcher

Async effects (Preview, RemoteEdit) travel through `EffectDispatcher` over dedicated lanes. Each lane owns a unique effect ID that increments on every dispatch, a cancellation map that can cancel in-flight work, and ordered response delivery. The TUI receives responses through `effect_rx`. Stale responses from superseded dispatches are dropped by ID.

### Effects, Services, Jobs

| Pattern | Used for | Lifecycle |
|---|---|---|
| **Effects** | Asynchronous interactive operations (Preview, Remote Edit) | Correlated request/response lanes, explicit cancellation |
| **Services** | Batch/automated operations (process execution, script invocation) | Request/result with metadata |
| **Jobs** | Long-running transfers, sync, mutations | Queued → Progress → Finished/Failed |

Remote preview and remote edit use Effect lanes. Transfers, syncs, and job-oriented mutations use the Job Manager. Remote Edit Job Manager integration is deferred (#51).

### Provider layer

`VfsProvider` trait + `ProviderRegistry`. Providers implement: listing, metadata, bounded reads, exact-length reads, write-back via immutable revision. Capability sets declare what a provider supports. F3 shows when `Read` is present; F4 shows when `Read` **and** `Write` are present **and** the provider policy allows editing — S3 has Read+Write but F4 stays intentionally disabled, so availability is capability **and** provider-policy gated, not capability-only.

### Transfer planners

`TransferPlanner` builds a frozen plan from a `TransferRequest`. The planner picks native, rsync, or SFTP streaming based on what's available. Rsync gets `--delete` only after explicit Preview approval. No plan mutates after it's dispatched.

### Remote Workspace

The workspace flow is deliberately split into independent truths:

- **Compare** (Ctrl+D) — recursive scan of both roots, provider-neutral diff
- **Preview** (Ctrl+X P) — frozen plan showing direction, copies, deletes, conflicts, size
- **Execute** — background job through Job Manager
- **Verify** — post-sync rescan reporting `Synchronized`, `DifferencesRemain`, or `Inconclusive`

### Safety

- SFTP copies stage to temp first. No silent overwrites.
- Destructive sync requires Preview.
- Cancel leaves source files untouched, cleans partial destinations.
- Host key verification uses the user's OpenSSH.
- Logs don't leak credentials.
- Remote disconnects are expected failure, not a crash.
- Remote Edit: atomic write-back with stage → backup → commit, NUL/binary refusal, transport-ambiguous failure retains recovery evidence.

### Connection pooling

SFTP connections are pooled per host. Ambiguous transport failures invalidate the session. Definitive protocol-level errors (status codes) don't trigger invalidation.

### S3 (object storage, AWS-shaped)

Implemented provider, physically accepted against MinIO and AWS-shaped-emulated
against Moto. Exact architectural distinctions:

- **Identity:** `S3BucketRef` / `S3PrefixRef` / `S3ObjectRef` are provider-native
  strings stored verbatim; `Entry.name` is presentation only and never an
  operational identity.
- **Prefixes are not POSIX directories:** `foo/bar` is an object-key namespace,
  not a filesystem path — no normalization, `//` collapse, `.`/`..` resolution, or
  canonicalization.
- **Transfer:** `TransferPlanner` builds a frozen `S3TransferSpec` with exact refs;
  no plan mutates after dispatch.
- **Runtime:** target-scoped `S3Provider` + lazy per-target client; bucket-bound
  targets can never reach `ListAllMyBuckets`.
- **Multipart:** sequential part PUT/GET, SDK retries disabled, cancellation truth
  (pre-create → clean interrupt; post-create → Abort attempted, outcome classified
  truthfully).
- **Verification:** physical outcome is separate from verification evidence; `ETag`
  is not a universal content hash.

S3 capability surface (current MVP): `List`, `Read`, `Write`, `Mkdir`, `Delete`.
`Copy`/`Move`/`Rename`/`Symlink`/`Chmod`/`ServerSideCopy` remain off. F4 edit is
intentionally disabled despite Read+Write (provider-policy gated, not
capability-only). No recursive prefix delete, no bucket create/delete.

## File tree (current runtime)

```
src/
├── main.rs
├── tui.rs              # event loop, rendering, keybindings
├── app/
│   ├── mod.rs          # AppState, PaneState, Availability
│   └── availability.rs # Action availability rules
├── vfs/
│   ├── mod.rs          # Entry, Location, VfsProvider, Capability, RemoteEditRevision
│   ├── local.rs
│   ├── sftp.rs         # full SFTP provider incl. atomic write
│   ├── archive.rs
│   ├── s3.rs          # implemented S3 object-storage provider (AWS-shaped; MinIO + Moto-emulated acceptance)
│   └── webdav.rs (stub)
├── transfer/           # TransferPlanner, executors, transactional SFTP copy
├── remote/             # HostConfig, OpenSSH transport, ssh_config
├── process/            # ProcessService, remote edit lifecycle, job dispatch
├── jobs/               # Job manager
├── input/              # Keymap, hints
├── effects.rs          # Effect lane definitions
├── effect_dispatcher.rs
├── config.rs
├── terminal.rs         # PTY, embedded terminal
└── lib.rs
```

## Testing

Full Rust test suite + `cargo fmt --check` + `cargo clippy --all-targets --all-features -- -D warnings`. CI on ubuntu-latest.

Integration tests for SFTP use disposable test hosts. Physical acceptance for remote editing includes E1–E12 scenarios covering download, conflict, binary gate, mode preservation, cancellation, and recovery.
