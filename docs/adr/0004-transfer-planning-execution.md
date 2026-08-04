# ADR 0004: Transfer requests, planning, and executors

Status: Accepted

## Context

ARX must copy and move between local filesystems, remote SFTP namespaces, archives, and future providers. `rsync` should be used where it is the strongest available implementation, but it cannot be the filesystem abstraction and may be unavailable remotely.

The initial model stored `Native`, `Rsync`, `Sftp`, or `Scp` directly in `TransferPlan`, which risks making implementation technology part of the public domain contract.

## Decision

A user/application request describes semantics, not a mechanism:

```text
TransferRequest
  source
  destination
  operation: copy | move | synchronize
  overwrite policy
  preservation policy
  verification policy
  resume preference
  bandwidth policy
```

The Transfer Planner combines the request with source/destination capabilities, host capabilities, user preferences, safety policy, and available executors.

The planner produces an executable plan containing ordered steps and a selected executor by opaque executor ID.

Built-in executors initially include:

- native local operations
- rsync process adapter
- SFTP streaming adapter
- SCP compatibility adapter only where needed
- archive transaction/stream adapters

Executor IDs are implementation registry identifiers, not variants that the TUI switches on.

### Selection policy

Typical preferences:

- same-filesystem rename for move when semantics allow
- native local operations for simple local work
- rsync for complex or resumable directory transfers when both endpoints support it
- rsync over OpenSSH for local/remote transfers when remote rsync is available
- SFTP fallback when remote rsync is unavailable
- archive-specific transaction or streaming when crossing archive namespace boundaries

### Move semantics

Move is not universally `copy + delete`. The planner may use atomic rename, server-side rename, rsync plus verified delete, or another safe sequence depending on capabilities. Source deletion occurs only after the destination satisfies the configured completion/verification contract.

### Sync semantics

Synchronization is separate from ordinary copy. Destructive sync requires a preview plan and explicit confirmation. `rsync --delete` is never added implicitly.

### External process adapters

The rsync/SCP/SSH adapters own command construction, argument handling, environment filtering, output parsing, exit-code mapping, process groups, cancellation, and redaction. Callers never build shell command strings.

## Consequences

- Users always invoke the same Copy/Move/Sync actions while ARX chooses the best implementation.
- rsync can be replaced or supplemented without changing UI contracts.
- safety policy is centralized in planning instead of duplicated across executors.
- future executors such as rclone/restic can be added without redefining what a transfer means.
