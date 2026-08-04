# ARX Architecture

## Purpose

ARX is a terminal commander that treats local filesystems, remote SSH/SFTP filesystems, and archives as locations behind common contracts. Long-running work is performed as jobs. Transfer implementation is selected by a planner rather than by the TUI.

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

The public model must support at least:

```text
file:///home/user
sftp://host/etc
archive:///tmp/data.tar.zst!/etc
```

A panel should be able to switch between those locations without changing its navigation model.

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

Initial backends:

- LocalFs
- SftpFs
- ArchiveFs

## Transfer planner

The planner receives source, destination, requested semantics, and detected capabilities. It chooses an implementation.

Typical policy:

- local → local, simple rename: native
- local → local, complex directory copy: native or rsync based on policy
- local ↔ remote, rsync available on both ends: rsync over SSH
- local ↔ remote, remote rsync unavailable: SFTP streaming
- archive boundaries: archive backend/streaming

`scp` is a compatibility transfer method, not the primary remote filesystem backend.

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

Jobs emit structured events such as progress, output, warnings, completion, and failure. The TUI consumes those events; it does not parse child-process output itself.

## Remote hosts

ARX host metadata refers to an OpenSSH alias wherever possible. Connection details remain in `~/.ssh/config`.

A host can belong to multiple groups. Groups may be nested for UI organization. Example dimensions include project, role, environment, and location.

ARX-specific metadata may include:

- favorite
- groups
- tags
- default path
- transfer preference
- notes

Sensitive credentials must not be stored in ordinary ARX configuration.

## OpenSSH integration

Prefer existing OpenSSH behavior for:

- aliases
- IdentityFile
- ProxyJump / ProxyCommand
- agent authentication
- known_hosts
- connection multiplexing

When effective OpenSSH configuration is needed, prefer `ssh -G <alias>` over implementing the entire ssh_config language ourselves.

## External tools

Expected adapters include:

- rsync
- ssh
- tar
- gzip/pigz
- bzip2/pbzip2
- xz
- zstd
- zip/unzip
- 7z/7zz
- user editor/pager/shell

Adapters own command construction, lifecycle, output parsing, error mapping, and capability detection.

## Terminal strategy

Phase 1: suspend the TUI and launch the user's local shell or OpenSSH client.

Phase 2: embedded local/remote PTY component, only after the remote and job layers are stable.

## Safety

- no silent overwrite
- no implicit rsync `--delete`
- cancellation must preserve source data and clean incomplete destinations where safe
- host-key verification is enabled by default
- logs must not expose passwords, private keys, or sensitive command arguments
- remote/network interruption is an expected failure mode and must be testable
