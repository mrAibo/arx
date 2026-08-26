# ARX Roadmap

GitHub state is authoritative over this document. Re-fetch current `main`, issues, PRs, and release state before acting on any SHA or backlog item recorded here.

## CURRENT — v0.22.0 release + post-release main truth

**Current public release:** `v0.22.0`  
**Release tag target:** `8737bbd2afaf0d6e7146a5d8c59ee1a0606325bf`  
**Published:** 2026-08-26  
**Current accepted main after #265:** `d95f6183c93932fba7f5ac10f421ce6abbe1f044`  
**Platform:** Linux only; published target Linux x86_64  
**MSRV:** Rust 1.88

v0.22.0 is the current stable 0.x release line. It keeps the one-build/no-rebuild release pipeline and expands the WebDAV recursive surface from one-way recursive download into recursive upload, safe recursive delete, and multi-root F5 Copy.

Current `main` is intentionally ahead of the published release: PR #265 merged the read-only S3 Object & Bucket Inspector from issue #264. This capability is accepted on `main` but is not yet present in the published v0.22.0 binaries.

Current product truth:

- **Local / SFTP:** browsing, transactional copy, bounded preview, SFTP conflict-safe text Remote Edit, OpenSSH-backed host/session behavior.
- **Remote Workspace:** compare → Preview → Execute → Verify for Local→Local, Local→SFTP, and SFTP→Local. SFTP→SFTP workspace sync remains unsupported.
- **Transfer Queue:** one persistent bounded FIFO runtime, configurable concurrency `1..=8`, truthful progress/rate/ETA where known, cooperative Pause/Resume/Cancel, and bounded safe retry.
- **Transfer Center v2:** Active / History / All views and controls routed to the existing `TransferQueueRuntime`.
- **Storage Inspector (`Alt+U`):** Local read-only logical/allocated usage, drill-down/top-files, hard-link handling, partial/error/cancel truth; current `main` additionally provides exact S3 object inspection plus bounded paginated bucket/prefix LiveScan analytics.
- **Filesystems (`Alt+D`):** read-only Linux capacity/inode view with explicit unavailable/autofs truth. S3 does not receive fabricated filesystem capacity or `df` semantics.
- **Effective keymap:** one conflict-safe effective runtime map with user overrides and `arx --print-keymap` discovery.
- **Mouse / split panes / terminal:** visible-row-correct mouse behavior, vertical+horizontal split panes, typed tmux/GNU Screen lifecycle.
- **Typed local Quick Actions:** SHA-256, Touch, Compress-to-tar.gz plus the existing mkdir/chmod/symlink surface.
- **S3:** AWS S3 + MinIO physically accepted supported MVPs; current `main` adds exact object and bounded bucket/prefix inspection through the existing provider/registry/job authorities; Moto emulated; Cloudflare R2 / Wasabi remain unverified best-effort targets.
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

## COMPLETED ON MAIN — S3 Object & Bucket Inspector

Issue [#264](https://github.com/mrAibo/arx/issues/264) was completed by PR [#265](https://github.com/mrAibo/arx/pull/265).

**Accepted implementation head:** `d0ceb64781777bd04fe51aeaff9b8d3dfa3c3343`  
**Squash merge on main:** `d95f6183c93932fba7f5ac10f421ce6abbe1f044`  
**State:** merged and post-merge CI accepted; not yet published in v0.22.0

The completed slice is deliberately read-only:

- exact provider-native object identity and `HeadObject` inspection
- size, last modified, ETag, content type, storage class, metadata, endpoint identity, and version ID only when actually returned
- paginated `ListObjectsV2` bucket/prefix LiveScan
- observed object count and logical bytes
- bounded largest-object ranking
- bounded immediate-prefix ranking with explicit unavailable truth when exact ranking cannot be retained
- age distribution and storage-class distribution
- bounded prefix cardinality and bounded storage-class cardinality
- progress, cancellation, and truthful partial-state handling
- reuse of the existing `ProviderRegistry`, per-target `S3Provider`/AWS client, `JobManager`, and UI architecture
- no S3 mutation/cleanup UI, no fake capacity/free-space/`df` data, no invented billing/cost data

Acceptance evidence for the final exact PR head included Format, Clippy, full tests, Rust 1.88 MSRV, real PTY multiplexer acceptance, Apache WebDAV regression acceptance, and the existing S3/MinIO physical lane. The MinIO lane explicitly executed `tests/s3_inspector_minio.rs` and proved the real object + prefix inspector path. The post-merge push CI on `d95f6183...` also completed successfully.

Cloudflare R2 / Wasabi remain best-effort/unverified; this slice does not claim physical certification for them.

## RELEASED WEBDAV WORK

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

The next major feature direction after the completed S3 Inspector should be **SFTP → SFTP workspace synchronization**.

Before implementation, freeze a dedicated issue around the existing Compare → Preview → Execute → Verify model. The slice must reuse the existing workspace-sync controller, provider authority, transfer queue, job lifecycle, retry policy, and verification model rather than introducing a second synchronization engine.

Required boundaries before implementation:

- preserve exact source and destination SFTP host/path identities
- support same-host and cross-host cases only where the execution model remains explicit and truthful
- reuse frozen Preview semantics before any destructive Mirror consequences
- keep copy/mutation ordering deterministic and recoverable
- verify the real destination after execution rather than treating transfer completion as synchronization proof
- keep cancellation and partial-completion truth explicit
- do not silently reinterpret SFTP→SFTP as server-side rename/move when a cross-host transfer is actually required
- do not combine this automatically with general cross-provider Move or WebDAV→WebDAV recursive copy

## RELEASE DECISION

**Recommendation: cut v0.23.0 before beginning the SFTP → SFTP implementation.**

Reasoning:

- #264/#265 is a complete, user-visible capability rather than an internal cleanup.
- Exact-head and post-merge CI are green, including the affected physical MinIO path.
- The completed S3 Inspector is cleanly separable from the next synchronization feature.
- Shipping it now keeps `main` and public binaries from drifting through another major feature block.
- A v0.23.0 release can remain a minimal release-truth slice: version/lockfile, release notes/public docs, release validation, tag, and publication; runtime behavior should remain frozen unless a genuine release blocker is discovered.

This is a release recommendation, not an already-created release. Until that release process is explicitly executed, v0.22.0 remains the current published version.

## RECOMMENDED FEATURE SEQUENCE

This is prioritization guidance, not a promise that each item must ship in the named version.

### Next — SFTP → SFTP workspace sync

Extend existing Compare → Preview → Execute → Verify only if both source and destination identities, mutation ordering, verification, and recovery remain truthful. Reuse the existing workspace-sync controller and transfer authority.

### After that — WebDAV → WebDAV recursive copy

Only after exact target/source identity and cross-target execution semantics are frozen. Do not pretend server-side MOVE spans unrelated targets.

### Later — general safe cross-provider Move

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
