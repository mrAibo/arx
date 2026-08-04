# ARX Architecture

## Purpose

ARX is a terminal resource commander for local files, remote SSH/SFTP namespaces, archives, transfers, synchronization, saved hosts, asynchronous jobs, and terminal sessions.

The architecture is intentionally provider-neutral. Local filesystems, SFTP, archives, and future providers are infrastructure implementations behind stable domain contracts. The TUI must not become coupled to any concrete backend or transfer mechanism.

The detailed decisions are recorded in `docs/adr/`.

## Non-negotiable rules

1. TUI code never invokes filesystem APIs, SSH/SFTP implementations, rsync, archive programs, or subprocesses directly.
2. Domain contracts do not depend on Ratatui, Crossterm, process handles, SSH library types, or command-line syntax.
3. A location identifies a path inside a registered VFS namespace; it does not encode a provider technology enum.
4. The UI asks for capabilities instead of branching on backend type.
5. VFS operations and transfer execution are separate concerns.
6. Transfer requests describe semantics; the planner chooses an executor.
7. Long-running and cancellable work runs through the Job Manager.
8. External tools are adapters with centralized argument construction, parsing, cancellation, redaction, and error mapping.
9. OpenSSH remains authoritative for SSH connection configuration whenever practical.
10. Persisted configuration is versioned and contains no runtime handles or secrets.
11. Destructive synchronization always has a preview stage and explicit confirmation.
12. Expected I/O, network, authentication, process, and capacity failures are typed errors, not panics.

## Layering

```text
┌──────────────────────────────────────────────┐
│ TUI / Presentation                           │
│ panels, dialogs, menus, jobs, hosts, terminal│
└──────────────────────┬───────────────────────┘
                       │ actions / queries
                       ▼
┌──────────────────────────────────────────────┐
│ Application layer                            │
│ orchestration, policy, confirmation, state   │
└───────────────┬───────────────┬──────────────┘
                │               │
                ▼               ▼
       ┌──────────────┐   ┌───────────────┐
       │ VFS domain   │   │ Transfer domain│
       └──────┬───────┘   └───────┬───────┘
              │                   │
              └─────────┬─────────┘
                        ▼
                 ┌────────────┐
                 │ Job Manager│
                 └─────┬──────┘
                       │
      ┌────────────────┼─────────────────────┐
      ▼                ▼                     ▼
 Local/provider    Remote/provider      Process/tool adapters
 adapters          adapters             rsync, ssh, tar, zstd…
```

Dependencies point inward. Infrastructure translates external systems into domain contracts.

## VFS model

The stable location shape is:

```text
Location
├── NamespaceId   opaque registered namespace
└── VfsPath       provider-native path identity
```

Examples of namespace registrations:

```text
local                 -> LocalFs
host:prod-db-01       -> SftpFs for a saved host
archive:<runtime-id>  -> ArchiveFs mounted over a container resource
```

The namespace registry owns provider instances. Presentation code does not derive behavior from namespace names.

### Path identity

Path identity must not require lossy conversion for display. Local native paths and byte-oriented remote/archive paths are represented separately where needed. Display formatting is a presentation concern.

### Capabilities

A provider reports supported operations such as list, read, write, mkdir, rename, remove, metadata, permissions, symlink handling, free-space queries, and server-side copy/move.

Archive providers may be read-only or transactionally writable. They are not forced to claim ordinary filesystem mutation semantics.

## Provider async boundary

The namespace registry must hold heterogeneous providers. Provider interfaces therefore need a dyn-compatible asynchronous boundary.

Rust async traits are not assumed to be dynamically dispatchable by themselves. When provider implementation starts, use either an explicit boxed-future boundary or one narrowly scoped compatibility helper. Do not replace the registry with a giant provider enum merely to avoid this boundary.

Pure domain logic remains synchronous. Tokio belongs at I/O, process, networking, scheduling, and event boundaries.

## Transfer architecture

A `TransferRequest` describes intent and policy:

```text
source / destination
operation: copy | move | synchronize
overwrite policy
verification policy
preservation policy
resume / bandwidth policy (when introduced)
```

The planner combines the request with VFS capabilities, host capabilities, tool availability, and user preference. It selects an executor using an opaque `ExecutorId`.

Initial executor families:

- native local operations
- rsync adapter
- SFTP streaming
- SCP compatibility fallback where useful
- archive transaction/stream adapter

Typical preference:

```text
same-filesystem move     -> native rename
simple local copy        -> native
large/complex local tree -> native or rsync by policy
local ↔ remote           -> rsync over SSH when available
local ↔ remote fallback  -> SFTP streaming
archive boundary         -> archive transaction/stream
```

Move is not universally copy + delete. Source deletion is allowed only after destination completion satisfies the requested verification contract.

Synchronization is a separate operation. `rsync --delete` is never implicit.

## Job architecture

The Job Manager is operation-neutral.

```text
JobId       stable runtime identity
JobSpec     immutable operation description
JobRecord   lifecycle + progress + result
JobCommand  cancel / retry / supported controls
JobEvent    structured event stream
```

Minimum lifecycle:

```text
Queued -> Starting -> Running -> Finished
                         ├─────> Failed
                         └─────> Cancelling -> Cancelled
```

Pause is capability-driven and not a universal state.

Progress may be indeterminate, byte-based, item-based, or phase-based. The TUI receives structured progress and does not parse rsync/SFTP/tool output itself.

Cancellation is translated by each executor into safe implementation-specific behavior. A cancellation must never silently destroy a valid source.

## Remote host model

A saved host has a stable ARX `HostId` and references an OpenSSH alias. Display names are mutable metadata, not identity.

Hosts support many-to-many groups and free-form tags. A host may simultaneously belong to, for example:

```text
Database
Project A
Production
Hannover
```

Groups may be nested for presentation. Parent cycles are invalid. Deleting a group never deletes a host.

ARX may store favorites, groups, tags, default path, transfer preference, and notes. Connection secrets do not belong in ordinary ARX configuration.

## OpenSSH integration

Prefer existing OpenSSH behavior for aliases, IdentityFile, ProxyJump/ProxyCommand, ssh-agent, known_hosts, and connection multiplexing.

When OpenSSH-effective configuration is required, prefer `ssh -G <alias>` rather than implementing the entire `ssh_config` language.

Native SSH/SFTP code consumes a resolved connection-profile abstraction rather than reading presentation config directly.

## Persistence

Persisted data is schema-versioned and migrated explicitly. Runtime handles are never serialized.

Expected logical documents include:

```text
config      user settings and schema version
hosts       host/group metadata
state       optional non-critical session/UI state
```

Writes that replace durable configuration must be atomic where the platform permits it.

Saved locations are separate entities from hosts, allowing multiple useful paths per machine.

## Error model

Infrastructure maps errors to stable domain categories such as not-found, permission-denied, unsupported, unavailable, authentication, host-key verification, network, timeout, interrupted, no-space, conflict, invalid configuration, external-tool failure, integrity failure, and internal invariant failure.

The UI reacts to structured categories rather than parsing error strings or exit codes.

## External processes

Adapters own:

- executable/argument construction without shell-string concatenation
- environment filtering
- process groups and lifecycle
- stdout/stderr parsing
- progress conversion
- cancellation and signal escalation
- exit-code/error mapping
- logging redaction

Expected tools include rsync, ssh, tar, gzip/pigz, bzip2/pbzip2, xz, zstd, zip/unzip, 7z/7zz, and the user's editor/pager/shell.

## Terminal strategy

Phase 1: suspend ARX safely and launch the user's local shell, OpenSSH, editor, pager, or another terminal application.

Phase 2: embedded terminal/PTY support only after VFS, remote connection management, and the Job Manager are stable.

Terminal restoration must survive ordinary errors and panic paths as far as reasonably possible.

## Security and safety

- no silent overwrite
- no implicit destructive synchronization
- no disabled SSH host-key verification by default
- no credentials/private keys/tokens in normal logs
- no shell command interpolation for adapter arguments
- transactional temporary files use safe permissions
- cancellation preserves valid source data
- interrupted networks and disk-full conditions are normal tested failure modes
- user-visible diagnostics are redacted

## Architecture decision records

Accepted ADRs:

- `0001-core-boundaries.md`
- `0002-vfs-location-provider.md`
- `0003-jobs-events-cancellation.md`
- `0004-transfer-planning-execution.md`
- `0005-remote-hosts-and-persistence.md`
- `0006-errors-security-observability.md`

Any future change that reverses one of these foundational decisions should add a superseding ADR rather than silently mutating the contract.
