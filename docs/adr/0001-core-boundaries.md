# ADR 0001: Core architecture boundaries

Status: Accepted

## Context

ARX is expected to grow from a dual-pane local commander into a resource commander covering local filesystems, SSH/SFTP, optimized rsync transfers, archives, synchronization, saved hosts, asynchronous jobs, and terminal sessions.

The expensive failure mode is allowing UI or one concrete transport to become the application's domain model. That would make every later backend or operation a cross-cutting rewrite.

## Decision

ARX uses explicit layers with inward dependencies:

```text
TUI / presentation
        |
        v
Application actions and queries
        |
        v
Domain contracts
  |        |        |
  v        v        v
VFS     Transfers   Jobs
  \        |        /
   \       v       /
    Infrastructure adapters
    local / ssh / sftp / rsync / archives / process
```

Rules:

1. TUI components do not call `std::fs`, SSH libraries, `ssh`, `rsync`, archive tools, or subprocesses directly.
2. Domain types do not depend on Ratatui, Crossterm, Tokio process types, russh, or command-line syntax.
3. Infrastructure adapters translate external behavior into domain capabilities, events, results, and errors.
4. Long-running or cancellable work executes as jobs.
5. A transfer request describes semantics; it does not prescribe the implementation mechanism.
6. Backend-specific facts are queried through capabilities rather than `if local` / `if sftp` / `if archive` branches in presentation code.
7. External tools are first-class adapters, not shell snippets distributed through the codebase.
8. Configuration and persisted identifiers must not contain runtime handles or secrets.

## Consequences

- Adding a new backend should primarily add an adapter and registration code.
- Adding a new transfer executor should not require changes to the TUI.
- UI code can enable or disable actions from capabilities without knowing backend technology.
- Infrastructure can use Tokio internally while pure domain logic stays synchronous.
- Some up-front interfaces are required before feature work, but this avoids later structural rewrites.
