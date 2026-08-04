# ADR 0005: Remote hosts, groups, OpenSSH integration, and persistence

Status: Accepted

## Context

ARX needs a durable inventory of remote machines, flexible grouping by project/role/environment, saved locations, and connection preferences. OpenSSH already owns complex connection configuration and authentication semantics.

Duplicating `ssh_config` would create conflicting sources of truth and security risk.

## Decision

OpenSSH remains authoritative for transport configuration whenever practical. ARX stores metadata that OpenSSH does not own.

### Host identity

A saved host has a stable ARX `HostId` and references an OpenSSH alias. Display name is not identity and may change.

ARX metadata may contain:

- display name
- OpenSSH alias
- favorite flag
- group memberships
- free-form tags
- default remote directory
- transfer preference
- notes

Passwords, private keys, and raw secrets are not stored in ordinary ARX configuration.

### Groups

Grouping is many-to-many. A host may belong simultaneously to `Database`, `Project A`, and `Production`.

Group hierarchy is presentation metadata, not ownership. Deleting or moving a group does not delete hosts.

Cycles in group parent relationships are invalid and must be rejected during validation.

### OpenSSH resolution

Where effective SSH configuration is needed, ARX prefers `ssh -G <alias>` for OpenSSH-specific resolution rather than implementing the full `ssh_config` language. Native SSH/SFTP adapters may consume resolved values through a dedicated connection profile abstraction.

### Saved locations

Saved locations reference stable host IDs / namespace specs and paths. They are separate from hosts so multiple useful locations can exist per machine.

### Configuration persistence

Configuration uses a versioned schema and atomic replace-on-success writes:

```text
config.toml       user preferences and schema version
hosts.toml        ARX host/group metadata
state.toml        non-critical UI/session state, if enabled
```

Exact file splitting may evolve without changing the requirement that each persisted document carries or inherits a schema version and supports explicit migration.

Runtime connection handles, process IDs, namespace handles, and cancellation tokens are never serialized.

## Consequences

- Existing SSH aliases, ProxyJump, IdentityFile, agent, and known_hosts behavior remain useful.
- Host organization can evolve independently from network configuration.
- Configuration migrations are intentional rather than ad-hoc parsing fallbacks.
- Later shared/team inventories can be introduced without changing the connection model.
