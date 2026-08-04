# AGENTS.md

## Project principles

ARX is a systems tool. Prefer simple, explicit designs and existing operating-system capabilities over custom frameworks.

Before changing code:

1. Read `README.md`, `ARCHITECTURE.md`, and `ROADMAP.md`.
2. Read the affected module and its callers.
3. Check whether the change belongs in TUI, app/service, VFS, transfer, jobs, remote, archive, process, or config.
4. Prefer the smallest change that preserves architectural boundaries.

## Rules

- Do not let TUI components invoke `std::fs`, `ssh`, `rsync`, or archive programs directly.
- Long-running operations must be represented as jobs.
- Do not add a dependency when the Rust standard library or an existing dependency cleanly solves the task.
- Prefer mature system utilities for established protocols and formats.
- Never silently overwrite or enable destructive synchronization.
- Never disable SSH host-key verification by default.
- Never log credentials, private keys, passwords, or sensitive environment values.
- Keep external process construction centralized in adapters.
- Keep VFS operations separate from transfer planning.
- Host grouping is many-to-many; do not model a host as belonging to only one group.
- OpenSSH configuration is authoritative for connection details whenever practical.

## Rust style

- `cargo fmt` is authoritative formatting.
- `cargo clippy --all-targets --all-features -- -D warnings` must pass.
- Prefer concrete types and straightforward ownership over premature generic abstractions.
- Use `thiserror` for domain/library errors.
- Avoid `unwrap()`/`expect()` in runtime paths unless an invariant is local and obvious.
- Async work belongs at I/O/process boundaries; do not make pure logic async without need.
- Cancellation must be explicit for long-running work.

## Tests

Behavior changes require tests where practical. Prioritize tests for:

- transfer planning
- overwrite/conflict safety
- path/location parsing
- cancellation
- interrupted transfers
- host grouping
- capability fallback
- destructive sync preview
- error mapping

Integration tests for remote functionality should use disposable SSH/SFTP test environments rather than production hosts.
