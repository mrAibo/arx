# ARX Roadmap

GitHub state is authoritative over this document. Re-fetch current `main`, issues, PRs, and release state before acting on any SHA or backlog item recorded here.

## CURRENT — v0.22.0 product truth

**Current public release:** `v0.22.0`  
**Release tag target:** `8737bbd2afaf0d6e7146a5d8c59ee1a0606325bf`  
**Published:** 2026-08-26  
**Platform:** Linux only; published target Linux x86_64  
**MSRV:** Rust 1.88

v0.22.0 is the current stable 0.x release line. It keeps the one-build/no-rebuild release pipeline and expands the WebDAV recursive surface from one-way recursive download into recursive upload, safe recursive delete, and multi-root F5 Copy.

Current product truth:

- **Local / SFTP:** browsing, transactional copy, bounded preview, SFTP conflict-safe text Remote Edit, OpenSSH-backed host/session behavior.
- **Remote Workspace:** compare → Preview → Execute → Verify for Local→Local, Local→SFTP, and SFTP→Local. SFTP→SFTP workspace sync remains unsupported.
- **Transfer Queue:** one persistent bounded FIFO runtime, configurable concurrency `1..=8`, truthful progress/rate/ETA where known, cooperative Pause/Resume/Cancel, and bounded safe retry.
- **Transfer Center v2:** Active / History / All views and controls routed to the existing `TransferQueueRuntime`.
- **Local Storage Inspector (`Alt+U`):** read-only logical/allocated usage, drill-down/top-files, hard-link handling, partial/error/cancel truth.
- **Filesystems (`Alt+D`):** read-only Linux capacity/inode view with explicit unavailable/autofs truth.
- **Effective keymap:** one conflict-safe effective runtime map with user overrides and `arx --print-keymap` discovery.
- **Mouse / split panes / terminal:** visible-row-correct mouse behavior, vertical+horizontal split panes, typed tmux/GNU Screen lifecycle.
- **Typed local Quick Actions:** SHA-256, Touch, Compress-to-tar.gz plus the existing mkdir/chmod/symlink surface.
- **S3:** AWS S3 + MinIO physically accepted supported MVPs; Moto emulated; Cloudflare R2 / Wasabi remain unverified best-effort targets.
- **WebDAV providers:** Apache mod_dav, Nextcloud 34.0.2-apache, and ownCloud 11.0.0 physically accepted through the Basic-auth path.
- **WebDAV recursive download:** one exact selected collection → one new Local tree with exact href identity, bounded Depth:1 traversal, manifest-before-mutation, noclobber staging, and truthful cleanup/recovery.
- **WebDAV recursive upload:** one Local directory → one new remote tree with complete Local pre-scan, shared 50,000/128 bounds, no-follow reads, root ownership, noclobber write semantics, and recovery truth.
- **WebDAV recursive delete:** one exact selected collection with complete+revalidated manifest, deepest-first/root-last execution, fresh empty proof before each collection DELETE, and no blind retry of ambiguity.
- **WebDAV multi-root F5:** multiple selected current sibling roots in Local↔WebDAV as one queued sequential job, with root-level Items progress and truthful partial completion.
- **Distribution:** GitHub Releases is the sole binary/package publication path. Linux x86_64 ships tar.gz, `.deb`, `.rpm`, and one `SHA256SUMS`, all produced from one validated ELF. The source tree intentionally does not track a current `bin/arx` copy.
- **Extension surface:** `arx.menu` remains the supported lightweight admin extension mechanism; there is no embedded Lua/WASM/native plugin runtime.

## COMPLETED FOUNDATION

The architecture sequence **O → P → Q → R is complete**.

```text
Location          = typed identity / address / navigation
ProviderRegistry  = execution authority
CapabilitySet     = exact-location / concrete-instance capability truth
VfsProvider       = backend provider interface
```

Do not add a second `ProviderRegistry`, `TransferQueueRuntime`, `JobManager`, `EffectDispatcher`, scheduler, retry authority, or secret store.

Provider-native identity remains authoritative. Presentation/display names never reconstruct existing remote addresses.

### External plugins

There is **no GO** for an external Lua/WASM/`.so` plugin runtime. Re-evaluate only if real user/ecosystem demand appears and a truthful enforcement/security model can be defined. `arx.menu` remains the supported lightweight extension path.

## RELEASED WEBdav WORK

The #13 post-MVP umbrella now includes these shipped capabilities:

- core WebDAV provider semantics
- Apache mod_dav W1–W18 physical acceptance
- Nextcloud 34.0.2 and ownCloud 11.0.0 I1–I12 physical certification
- WebDAV F5 source/selection truth hardening
- RFC-compatible MOVE Depth behavior
- exact recursive WebDAV collection download to Local (#248 / PR #250)
- recursive Local → WebDAV upload (#253 / PR #254)
- safe bounded recursive WebDAV delete (#255 / PR #256)
- one-job multi-root Local↔WebDAV F5 Copy (#257 / PR #258)

Remaining #13 items are enhancements, not release blockers:

- [ ] multi-root recursive WebDAV delete
- [ ] WebDAV → WebDAV recursive/cross-target copy or move with truthful target/recovery semantics
- [ ] optional metadata/property mutation only for a demonstrated admin use case
- [ ] Digest/Bearer auth only if interoperability evidence requires it

## SELECTED NEXT PRODUCT DIRECTION

The next major usefulness direction should move beyond continued WebDAV breadth and add **read-only S3 storage intelligence**.

Before implementation, create/freeze a dedicated issue for an **S3 Object Inspector + Bucket Inspector** slice with these boundaries:

- read-only only; no cleanup/mutation surface
- reuse the existing S3 provider/registry/job authorities
- object details: key, size, last modified, ETag, content type, storage class, metadata, version information where the provider can prove it
- bucket/prefix view: object count, logical bytes, largest objects/prefixes, age distribution, storage-class distribution where derivable
- pagination/streaming and cancellation for large buckets
- explicit evidence/freshness source for aggregate analytics (`LiveScan`, `StorageLens`, `Inventory`, `OtherProvider`, `Unavailable`)
- AWS S3 + MinIO physical acceptance; R2/Wasabi remain best-effort until separately evidenced
- **never fabricate POSIX `df`/filesystem-capacity semantics for S3**

Do not combine the S3 Inspector automatically with WebDAV→WebDAV transfer, SFTP→SFTP workspace sync, or general cross-provider Move.

## RECOMMENDED FEATURE SEQUENCE

This is prioritization guidance, not a promise that each item must ship in the named version.

### Next — S3 Object/Bucket Inspector

Read-only storage intelligence using truthful provider evidence, bounded/paginated scans, cancellation, and no POSIX-capacity fiction.

### After that — SFTP → SFTP workspace sync

Extend existing Compare → Preview → Execute → Verify only if both source and destination identities, mutation ordering, verification, and recovery remain truthful. Reuse the existing workspace-sync controller and transfer authority.

### Later — WebDAV → WebDAV recursive copy

Only after exact target/source identity and cross-target execution semantics are frozen. Do not pretend server-side MOVE spans unrelated targets.

### Later still — general safe cross-provider Move

Model as copy → verify → delete source with explicit ambiguity/recovery boundaries. Never treat cross-provider Move as an optimistic rename.

## OTHER PRODUCT BACKLOG

Items not currently represented by active GitHub issues should be promoted to dedicated issues before implementation:

- binary remote editing
- additional Linux architectures, especially ARM64 if user demand justifies it
- signed package-repository distribution if operationally worthwhile
- provider-specific read-only analytics where evidence is truthful

Native Windows support remains out of scope. Windows SSH clients may interoperate with an ARX process running on Linux; that does not change the Linux-only product policy.

## RELEASED — v0.22.0 (2026-08-26)

Release target: `8737bbd2afaf0d6e7146a5d8c59ee1a0606325bf`.

Highlights:

- recursive Local → WebDAV upload
- safe bounded recursive WebDAV delete with destructive ambiguity/LOCK/cancellation physical proof
- multi-root Local↔WebDAV F5 Copy as one queued job
- recursive WebDAV download retained from v0.21.0
- Apache mod_dav, Nextcloud 34.0.2, and ownCloud 11.0.0 physical acceptance
- existing MinIO retry and real tmux/GNU Screen PTY lanes remained green

Published artifacts:

- `arx-v0.22.0-x86_64-unknown-linux-gnu.tar.gz`
- `arx_0.22.0_amd64.deb`
- `arx-0.22.0-1.x86_64.rpm`
- `SHA256SUMS`

The official Release run validated tag/version truth, release notes/hero asset, format, Clippy, full tests, Rust 1.88, one release build, third-party licenses, TAR/DEB/RPM exact payloads and metadata, packaged-binary identity, checksums, and publication from the validated artifact bundle.

Independent post-publish verification rechecked all three package hashes against `SHA256SUMS`; the tarball binary reports `arx 0.22.0` and `--help` exits successfully.

## RELEASE HISTORY

### v0.21.0

- exact recursive WebDAV collection download to a new Local tree
- Nextcloud 34.0.2 and ownCloud 11.0.0 certification
- WebDAV target/source truth and MOVE interoperability fixes
- split-pane, effective-keymap, tmux/screen, mouse, pause-acceptance, and local Quick Action improvements

### v0.20.0

- Local Storage Inspector / Filesystems
- Transfer Center v2
- native Linux tar.gz / DEB / RPM publication contract

### v0.19.0

- persistent bounded FIFO Transfer Queue
- configurable concurrency, progress/rate/ETA, Pause/Resume/Cancel
- bounded safe retry and ambiguity/recovery classification

### v0.18.0

- WebDAV MVP with PROPFIND / GET / PUT / DELETE / MKCOL / COPY / MOVE
- Basic auth through keyring/environment secret resolution
- raw-href authority and Apache W1–W18 physical acceptance

### v0.17.x

Historical Linux publication/SFTP Remote Edit baseline. Any old tracked `bin/arx` copy from that era is obsolete and is not a current distribution channel.

## RELEASE PROCESS POLICY

A release candidate should keep runtime code frozen and change only release truth unless a newly discovered correctness/safety blocker requires a separately reviewed fix.

Before tagging:

- Cargo version and root Cargo.lock version match the intended tag.
- public docs and `docs/releases/vX.Y.Z.md` describe the same product truth.
- exact-head quality / Rust 1.88 MSRV / affected physical provider gates are green.
- Release validation builds once and validates notices, tar/deb/rpm exact payloads, package metadata, ELF identity, checksums, and artifact upload.
- the tag targets the accepted merge SHA.
- publication reuses validated artifacts without rebuilding them.

## DEVELOPMENT POLICY

- Prefer user-visible vertical slices over new architecture packs.
- Implement one selected feature as a coherent macro-batch instead of a chain of micro-tasks.
- Collect related findings and fix them coherently on the same implementation branch.
- Keep product/runtime/documentation/physical acceptance changes for one capability in the same slice when logically required.
- Use exact-head CI and physical evidence as acceptance gates.
- Do not weaken production semantics to satisfy timing-sensitive tests.
- Move toward a usable release once the frozen slice is complete; do not extend scope merely because adjacent ideas are available.
- Use Hermes only for deterministic Linux-local execution that connected tooling cannot perform; architecture, review, PR/merge, and release authority remain outside Hermes.
