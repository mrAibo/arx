# ADR 0002: VFS locations, providers, and capabilities

Status: Accepted

## Context

The initial prototype represented a location as an enum with `Local`, `Sftp`, and `Archive` variants. That is convenient initially but makes every new provider a domain-wide enum change and encourages technology checks in callers.

ARX must support providers that do not share identical path semantics. Native local paths may be non-UTF-8. Remote Unix paths may be arbitrary bytes. Archives are mounted views over another resource rather than ordinary native filesystems. Future providers may use keys rather than POSIX paths.

## Decision

The stable domain concept is a location in a registered VFS namespace:

```text
VfsLocation
  namespace: NamespaceId
  path: VfsPath
```

`NamespaceId` is an opaque stable identifier. The TUI does not infer technology from it.

A runtime namespace registry resolves `NamespaceId` to a provider instance and descriptor. Examples:

```text
local              -> LocalFs
host:prod-db-01    -> SftpFs for saved host
archive:<runtime>  -> ArchiveFs mounted over a container location
```

Persisted locations use a serializable `LocationSpec`; runtime locations use resolved namespace handles. Runtime handles are never persisted.

### Path representation

`VfsPath` must preserve provider-native identity. It must not require lossy UTF-8 conversion merely for display. Initial implementations may support native OS paths and byte-oriented portable paths, with formatting kept separate from identity.

### Capabilities

Providers expose capabilities for a location or namespace. Presentation code asks capabilities such as:

- list
- read
- write
- mkdir
- rename
- remove
- metadata
- permissions
- symlink operations
- free-space query
- server-side copy/move
- watch/refresh support

Capabilities describe support and semantics; they are not inferred from provider names.

Archive providers may be read-only, writable through transactions, or support only selected mutations. They are not forced to pretend to have ordinary filesystem semantics.

### Provider registry

Providers are heterogeneous and resolved dynamically. Provider traits therefore need dyn-compatible async boundaries. Until Rust supports the desired dynamic async trait shape directly, implementations should use an explicit boxed-future boundary or a narrowly scoped helper crate rather than leaking concrete provider enums throughout the domain.

## Consequences

- New providers do not require UI rewrites.
- Archive nesting can be represented as mounted namespaces.
- Saved remote hosts and VFS locations remain related but distinct concepts.
- Display names and URIs are presentation/serialization concerns, not path identity.
- Provider capability changes can be represented without changing the panel model.
