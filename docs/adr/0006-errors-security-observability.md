# ADR 0006: Errors, security boundaries, and observability

Status: Accepted

## Context

ARX must translate failures from local filesystems, SSH/SFTP, rsync, archive tools, subprocesses, and configuration into behavior that the TUI and Job Manager can handle consistently. Raw strings and raw external exit codes are not sufficient domain contracts.

Remote and external-tool support also creates risks around credential leakage, shell injection, host-key verification, and unsafe logging.

## Decision

### Error taxonomy

Infrastructure failures are mapped into a stable domain error classification before reaching application/UI layers. Initial categories include:

- not found
- already exists
- permission denied
- unsupported operation
- unavailable dependency/provider
- authentication failed
- host-key verification failed
- network failure
- timeout
- interrupted/cancelled
- no space
- conflict
- invalid input/configuration
- external tool failure
- integrity/verification failure
- internal invariant failure

Errors may retain a redacted source chain for diagnostics, but UI behavior depends on the stable category and structured context rather than parsing strings.

### Security boundaries

- No shell-string command construction for ordinary adapters; pass executable and arguments separately.
- External command logging is redacted before emission.
- Passwords, private keys, tokens, and sensitive environment values are never written to normal logs.
- SSH host-key verification is enabled by default and must not be silently bypassed.
- OpenSSH/agent/keychain facilities are preferred to inventing credential storage.
- Destructive operations require explicit policy and confirmation at the application layer.
- Temporary files for transactional writes use safe permissions and same-filesystem placement when atomic replacement is required.

### Observability

`tracing` is the internal observability API. Structured spans should include safe identifiers such as job ID, provider ID, host ID, operation kind, and executor ID, but never credentials.

User-visible job logs and diagnostic logs are separate concerns. Raw tool output may be attached to a job detail stream after redaction and size limits.

### Panic policy

Expected I/O/network/process failures return typed errors; they do not panic. Panics represent programmer/invariant failures. Terminal restoration must be guarded so a panic does not leave the terminal unusable.

## Consequences

- TUI can offer meaningful retry/conflict/authentication UX independent of backend.
- Logs remain useful without becoming a credential leak.
- Adapter-specific exit codes stay localized.
- Security-sensitive defaults are architectural requirements rather than conventions.
