# ARX Architecture

## What it does

ARX is a terminal commander. It treats local filesystems, remote SSH/SFTP
connections, and archives as locations behind a common API. Long work runs
as background jobs. The TUI never touches filesystems or SSH directly — it
goes through the planner, which picks the right executor for each transfer.

## Rules we follow

1. The TUI never calls filesystem, SSH, rsync, or archive code directly.
2. `Location` tells you where something lives; panels work with locations, not raw paths.
3. VFS (listing, reading, stat) and transfers (copy, move, sync) are separate layers.
4. Anything that takes time runs through the Job Manager.
5. When a system tool does the job better than custom code, use the system tool.
6. Destructive syncs need a preview before they run.
7. Authentication reuses OpenSSH config, ssh-agent, and known_hosts. ARX doesn't invent its own password vault.
8. Dependencies get added only when a concrete module needs them.

## Layering

```text
TUI components
      │
      ▼
App actions / state
      │
      ▼
Services ───────────────┐
      │                 │
      ▼                 ▼
VFS                Transfer Planner
      │                 │
      │                 ▼
      │             Job Manager
      │                 │
      ├──────────┬───────┼──────────┐
      ▼          ▼       ▼          ▼
   Local FS    SFTP    rsync     Archives
                    
Remote Manager
 ├─ Host inventory
 ├─ Host groups/tags
 ├─ OpenSSH config
 ├─ Connections
 └─ Capability cache
```

## Location model

A panel can switch between these without changing how navigation works:

```text
file:///home/user
sftp://host/etc
archive:///tmp/data.tar.zst!/etc
```

### ProviderId

Every `Location` variant maps to a `ProviderId`:

| Location | ProviderId |
|----------|-----------|
| `Local(path)` | `Local` |
| `Sftp { host, path }` | `Sftp` |
| `Archive { ... }` | `Archive` |

The `ProviderRegistry` knows what capabilities each provider has. When you
hit F5 or F6, the TUI asks the registry for source and destination
capabilities and passes them into the transfer request.

## What VFS does

VFS handles filesystem operations: list, stat, read, write, mkdir, rename,
remove, symlink metadata, permissions.

Backends: LocalFs, SftpFs, ArchiveFs. S3 and WebDAV are stubbed — ready for
someone to implement them.

## Transfer planner

When you press F5 (copy) or F6 (move), the TUI builds a `TransferRequest`:

```text
TransferRequest {
    source: Location,
    destination: Location,
    source_provider: ProviderId,
    destination_provider: ProviderId,
    source_capabilities: CapabilitySet,
    destination_capabilities: CapabilitySet,
    intent: TransferIntent (Copy | Move | Synchronize),
    executors: ExecutorAvailability (which of native/rsync/sftp are usable),
    delete_extraneous: bool,
}
```

`TransferPlanner::plan()` picks a method:

- **Native** — local to local, when both providers support the operation
- **Rsync** — the tool is available and remote detection confirms it
- **SFTP** — remote rsync isn't available, so we stream over SFTP with transactional staging
- **SCP** — compatibility fallback, stubbed for now

The resulting `TransferPlan` goes to `execute_transfer()`, which dispatches
to the right executor. Each executor sends progress events back through
`tokio::sync::mpsc`.

### Transactional SFTP copy

SFTP copies use three phases so we don't leave broken files behind:

1. **Stage** — upload/download to `.arx-part-<id>` temp file
2. **Backup** — if the destination already exists, rename it to `.arx-bak-<id>`
3. **Commit** — rename temp to destination; if that fails, restore the backup

Streaming uses 64 KiB buffers. File names get validated: `..`, `/`, and
`NUL` are rejected.

## Jobs

Any operation that takes time becomes a job. States: Queued, Starting,
Running, Paused (only when the executor supports it), Cancelling,
Cancelled, Failed, Finished.

Jobs emit structured events — progress, completion, failure. The TUI reads
those events. It doesn't parse child process output itself.

## Remote hosts

ARX host metadata points at an OpenSSH alias whenever possible. Connection
details stay in `~/.ssh/config`.

Per-host metadata can include: favorite, groups, tags, default path,
transfer preference, notes. Credentials don't go in ARX config.

## OpenSSH integration

We lean on existing OpenSSH behavior for: aliases, IdentityFile, ProxyJump,
agent auth, known_hosts, connection multiplexing.

When we need to know what SSH would do for a given alias, we run `ssh -G
<alias>` instead of parsing `ssh_config` ourselves.

### OpenSSH to SFTP transport

`OpenSshSftpConnection` wraps a system `ssh` subprocess into a
`russh-sftp` client session. This means ARX gets SFTP through the user's
actual OpenSSH binary, their agent, and their config — without needing its
own SSH implementation.

## External tools

Adapters exist for: rsync, ssh, tar, gzip/pigz, bzip2/pbzip2, xz, zstd,
zip/unzip, 7z/7zz, and the user's editor/pager/shell.

Each adapter owns its own command construction, lifecycle, output parsing,
error mapping, and capability detection.

## Safety

- SFTP copies stage to a temp file first. No silent overwrites.
- Rsync never runs with `--delete` unless explicitly asked.
- Cancelling a transfer leaves source files untouched and cleans up partial destinations.
- Host key verification is on by default (through system OpenSSH).
- Logs don't leak passwords, private keys, or sensitive command arguments.
- Remote disconnects are an expected failure mode, not a crash.

## File tree (what's actually implemented)

```
src/transfer/
├── mod.rs          TransferPlanner, TransferPlan, TransferRequest
├── executor.rs     execute_transfer() — dispatches Native/rsync/SFTP
├── probe.rs        detect_local_tools(), local_executors()
└── sftp_copy.rs    Transactional SFTP copy (stage → backup → commit → rollback)

src/remote/
├── mod.rs          HostInventory, HostConfig
├── hosts_config.rs hosts.toml parser
├── openssh_sftp.rs OpenSSH → SFTP transport
├── ssh_config.rs   ~/.ssh/config parser
└── watch.rs        inotify → rsync daemon (Linux-only)

src/vfs/
├── mod.rs          Location, Entry, VfsOps, ProviderId, Capability
├── local.rs        LocalFs
├── sftp.rs         SftpFs (uses OpenSshSftpConnection)
├── archive.rs      ArchiveFs (tar/zip)
├── s3.rs           Stub
└── webdav.rs       Stub
```

## Testing

42 tests cover the transfer planner, executors, VFS, and SFTP transactional
copy. CI runs `cargo fmt --check`, `cargo clippy --all-targets
--all-features -- -D warnings`, and `cargo test --all-features` on
ubuntu-latest.