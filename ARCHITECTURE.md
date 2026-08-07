# ARX Architecture

## Purpose

ARX is a terminal commander that treats local filesystems, remote SSH/SFTP
filesystems, and archives as locations behind common contracts. Long-running
work is performed as jobs. Transfer implementation is selected by a planner
rather than by the TUI. **This architecture is now fully implemented.**

## Core rules

1. The TUI never calls filesystem, SSH, rsync, or archive implementations directly.
2. `Location` identifies where a resource lives; UI panels operate on locations, not raw local paths.
3. VFS operations and transfer operations are separate concerns.
4. Long-running operations execute through the Job Manager.
5. Prefer mature system tools when they provide stronger behavior than a custom implementation.
6. Destructive synchronization requires a preview/dry-run stage.
7. Authentication should reuse OpenSSH configuration, ssh-agent, and known_hosts; ARX does not invent a password vault.
8. Dependencies are added only when a concrete module needs them.

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

```text
file:///home/user
sftp://host/etc
archive:///tmp/data.tar.zst!/etc
```

A panel switches between those locations without changing its navigation model.

### ProviderId

Each `Location` variant maps to a `ProviderId` (added to `Location` via `provider_id()`):

| Location | ProviderId |
|----------|-----------|
| `Local(path)` | `Local` |
| `Sftp { host, path }` | `Sftp` |
| `Archive { ... }` | `Archive` |

`ProviderRegistry` tracks registered capability sets per provider. The TUI
queries the registry for source/destination capabilities when building a
`TransferRequest`.

## VFS responsibilities

VFS handles filesystem-like operations:

- list
- stat
- read/open
- write/create
- mkdir
- rename
- remove
- symlink metadata
- permissions/capabilities where supported

Backends: LocalFs, SftpFs, ArchiveFs. S3 and WebDAV are stubbed.

## Transfer planner (implemented)

When F5 (copy) or F6 (move) is pressed, the TUI builds a `TransferRequest`:

```text
TransferRequest {
    source: Location,
    destination: Location,
    source_provider: ProviderId,
    destination_provider: ProviderId,
    source_capabilities: CapabilitySet,
    destination_capabilities: CapabilitySet,
    intent: TransferIntent (Copy | Move | Synchronize),
    executors: ExecutorAvailability (native/rsync/sftp booleans),
    delete_extraneous: bool,
}
```

`TransferPlanner::plan()` chooses a `TransferMethod`:

- **Native** — local ↔ local when both providers support the intent
- **Rsync** — available externally and remote tool detection confirms it
- **SFTP** — remote rsync unavailable, falls back to 64 KiB streaming SFTP with transactional staging
- **SCP** — compatibility fallback (stubbed, returns "not implemented")

The resulting `TransferPlan` is handed to `execute_transfer()`, which
dispatches to the matching executor. Each executor reports progress events
back to the Job Manager via `tokio::sync::mpsc`.

### Transactional SFTP copy

SFTP file copy uses a three-phase commit:

1. **Stage** — upload/download to `.arx-part-<id>` temp file
2. **Backup** — if destination exists, rename to `.arx-bak-<id>`
3. **Commit** — rename temp to destination; on failure, restore backup

Streaming uses 64 KiB buffers. File name validation rejects `..`, `/`, `NUL`.

## Jobs

All long-running operations become jobs. Minimum states:

- Queued
- Starting
- Running
- Paused (only where the implementation supports it)
- Cancelling
- Cancelled
- Failed
- Finished

Jobs emit structured events: progress, completion, failure. The TUI consumes
those events; it does not parse child-process output itself.

## Remote hosts

ARX host metadata refers to an OpenSSH alias wherever possible. Connection
details remain in `~/.ssh/config`.

ARX-specific metadata may include: favorite, groups, tags, default path,
transfer preference, notes. Sensitive credentials must not be stored in
ordinary ARX configuration.

## OpenSSH integration

Prefer existing OpenSSH behavior for: aliases, IdentityFile, ProxyJump /
ProxyCommand, agent authentication, known_hosts, connection multiplexing.

When effective OpenSSH configuration is needed, prefer `ssh -G <alias>` over
implementing the entire ssh_config language ourselves.

### OpenSSH → SFTP transport

`OpenSshSftpConnection` wraps a system `ssh` subprocess into a `russh-sftp`
client session. This reuses the user's OpenSSH binary, agent, and config
without ARX needing its own SSH implementation.

## External tools

Expected adapters: rsync, ssh, tar, gzip/pigz, bzip2/pbzip2, xz, zstd,
zip/unzip, 7z/7zz, user editor/pager/shell.

Adapters own command construction, lifecycle, output parsing, error mapping,
and capability detection.

## Safety

- no silent overwrite — SFTP uses staging + backup + rollback
- no implicit rsync `--delete`
- cancellation must preserve source data and clean incomplete destinations where safe
- host-key verification is enabled by default (via system OpenSSH)
- logs must not expose passwords, private keys, or sensitive command arguments
- remote/network interruption is an expected failure mode and must be testable

## File tree (implemented modules)

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

42 tests across the transfer planner, executors, VFS, and SFTP transactional copy.
CI runs `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
and `cargo test --all-features` on ubuntu-latest.