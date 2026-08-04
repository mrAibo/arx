# ADR 0003: Jobs, events, progress, and cancellation

Status: Accepted

## Context

ARX will run heterogeneous long-lived work: local copies, rsync transfers, SFTP streams, archive rebuilds, searches, synchronization previews, remote commands, capability probes, and later terminal-related operations.

The initial `Job` structure encoded source and destination directly, which does not fit all job kinds and would couple scheduling to transfer semantics.

## Decision

The Job Manager owns lifecycle, scheduling, cancellation, concurrency, and history. Operation-specific parameters live in immutable job specifications outside the generic lifecycle record.

Core concepts:

```text
JobId          opaque newtype
JobSpec        immutable operation description
JobRecord      lifecycle + timestamps + progress + outcome
JobCommand     cancel / retry / pause where supported
JobEvent       structured state/progress/output/warning/result events
```

Minimum lifecycle:

```text
Queued -> Starting -> Running -> Finished
                         |          
                         +-> Failed
                         +-> Cancelling -> Cancelled
```

`Paused` is not a universal state. It is advertised only when an executor can actually pause safely.

### Progress

Progress is generic, not byte-only. It can be:

- indeterminate
- bytes completed / optional total
- items completed / optional total
- phase + nested progress

Rate and ETA are derived when meaningful rather than required for every job.

### Cancellation

Cancellation is cooperative first and executor-specific. The Job Manager sends a cancellation request; the executor translates it appropriately:

- native async I/O: stop at safe boundaries and clean partial destination when policy requires
- rsync/process: signal escalation with a grace period
- SFTP: abort stream/channel and reconcile partial destination policy
- archive transaction: abort before commit or roll back temporary output

Cancellation must never silently delete a valid source.

### Events

Events are structured domain messages. Raw stdout/stderr may be retained in job detail logs, but the TUI does not parse tool output itself.

### Persistence

Running executor handles are runtime-only. Persisted job history stores safe summaries and results, not process IDs, channel handles, credentials, or cancellation tokens.

## Consequences

- New job kinds do not require scheduler redesign.
- Transfer, search, sync, and remote-command jobs share one lifecycle system.
- The UI can render one job manager for all operations.
- Executor implementations remain responsible for translating cancellation and progress into the common model.
