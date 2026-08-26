# ARX Roadmap

GitHub state is authoritative over this document. Re-fetch current `main`, issues, PRs, tags, workflow state, and releases before acting on any recorded SHA or backlog item.

## CURRENT — v0.24.0 published / WebDAV backlog next

**Current public release:** `v0.24.0`
**Release tag target:** `6d413fac5d5b493859bfadfbedbeb436b1140e0b`
**Published:** 2026-08-26
**Platform:** Linux only; published target Linux x86_64
**MSRV:** Rust 1.88
**Previous immutable release:** `v0.23.0` → `f66a25f3f2b4fb66832ecc50d85f9f105ebba086`
**Release commit / publication baseline:** `6d413fac5d5b493859bfadfbedbeb436b1140e0b`; later docs-only commits may advance `main` without moving the release tag.

v0.24.0 ships **SFTP → SFTP Workspace Sync** on top of the v0.23.0 read-only S3 Object & Bucket Inspector and the retained WebDAV recursive-operation surface.

Release acceptance evidence:

- release PR: #273; exact candidate head `fd798b3117bec1f782f53cbedd48bdab72eaab08`
- exact-head CI: run `33010488213` — success
- exact-head SFTP physical: run `33010488203` — success
- exact-head Nextcloud/ownCloud interoperability: run `33010488206` — success
- exact-head Release validator: run `33010488294` — success
- release squash merge / release commit: `6d413fac5d5b493859bfadfbedbeb436b1140e0b`
- post-merge CI: run `33011943055` — success
- post-merge SFTP physical: run `33011943080` — success
- release workflow on immutable `v0.24.0`: run `33012256020` — validate success, publish success
- tag ↔ Cargo version, release notes, hero asset, Format, Clippy, tests, Rust 1.88 MSRV, one release build, third-party licenses, TAR/DEB/RPM payloads, packaged-binary smoke, checksums, and artifact publication all passed
- published Release is latest, `draft=false`, `prerelease=false`
- independent artifact verification: all three package hashes match `SHA256SUMS`; published GitHub asset digests match; tarball binary reports `arx 0.24.0`; `--help` exits successfully
- v0.23.0 and v0.22.0 tag targets remained unchanged throughout the release

Published artifacts:

- `arx-v0.24.0-x86_64-unknown-linux-gnu.tar.gz`
- `arx_0.24.0_amd64.deb`
- `arx-0.24.0-1.x86_64.rpm`
- `SHA256SUMS`

Published SHA-256 values:

- tar.gz: `d8463a3d78e1d19e61f7cb3383b17d5f4cf0fb4f046e989d1f1129aa18237f42`
- DEB: `697c7f6e5c69d23ee97ae4664fb60af26f140ef5005686f10db97895a692fda4`
- RPM: `9a5f6a99ed5dc4ed7db77def9f1ab7efe1feb419c1b76a674b586dcc7ece1bc2`

## CURRENT PRODUCT TRUTH

- **Local / SFTP:** browsing, transactional copy, bounded preview, SFTP conflict-safe text Remote Edit, OpenSSH-backed host/session behavior.
- **Remote Workspace:** published v0.24.0 provides Compare → Preview → Execute → Verify for Local→Local, Local→SFTP, SFTP→Local, and same-host/cross-host SFTP→SFTP sync with bounded remote → ARX → remote streaming and real two-endpoint OpenSSH acceptance.
- **Transfer Queue:** one persistent bounded FIFO runtime, configurable concurrency `1..=8`, truthful progress/rate/ETA where known, cooperative Pause/Resume/Cancel, and bounded safe retry.
- **Transfer Center v2:** Active / History / All views and controls routed to the existing `TransferQueueRuntime`.
- **Storage Inspector (`Alt+U`):** Local read-only logical/allocated usage plus exact S3 object inspection and bounded paginated bucket/prefix LiveScan analytics.
- **Filesystems (`Alt+D`):** read-only Linux capacity/inode view. S3 never receives fabricated filesystem capacity, inode, free-space, or `df` semantics.
- **Effective keymap:** one conflict-safe effective runtime map with user overrides and `arx --print-keymap` discovery.
- **Mouse / split panes / terminal:** visible-row-correct mouse behavior, vertical+horizontal split panes, typed tmux/GNU Screen lifecycle.
- **Typed local Quick Actions:** SHA-256, Touch, Compress-to-tar.gz plus mkdir/chmod/symlink surface.
- **S3:** AWS S3 + MinIO supported paths; Moto emulated; exact object and bounded bucket/prefix inspector shipped in v0.23.0; Cloudflare R2 / Wasabi remain unverified best-effort targets.
- **WebDAV:** Apache mod_dav, Nextcloud 34.0.2-apache, and ownCloud 11.0.0 physically accepted through Basic auth.
- **WebDAV recursive operations:** recursive WebDAV→Local download, Local→WebDAV upload, safe bounded recursive delete for one collection, and one-job multi-root Local↔WebDAV F5 Copy.
- **Distribution:** GitHub Releases is the binary/package publication path; Linux x86_64 ships tar.gz, DEB, RPM, and one `SHA256SUMS`, all produced from one validated ELF.
- **Extension surface:** `arx.menu` remains the supported lightweight extension mechanism; there is no embedded Lua/WASM/native plugin runtime.

## FROZEN ARCHITECTURE

The architecture sequence O → P → Q → R is complete.

```text
Location          = typed identity / address / navigation
ProviderRegistry  = execution authority
CapabilitySet     = exact-location / concrete-instance capability truth
VfsProvider       = backend provider interface
```

Provider-native identity remains authoritative. Display text never reconstructs existing remote addresses.

Do not introduce a second:

- `ProviderRegistry`
- `TransferQueueRuntime`
- `JobManager`
- `EffectDispatcher`
- scheduler
- retry authority
- secret store

External plugins remain **no GO**. Re-evaluate Lua/WASM/native plugins only if real user/ecosystem demand appears and a truthful enforcement/security model exists. `arx.menu` remains the supported lightweight admin extension path.

## RELEASED — S3 Object & Bucket Inspector

Issue #264 was completed by PR #265.

**Accepted feature head:** `d0ceb64781777bd04fe51aeaff9b8d3dfa3c3343`  
**Feature squash merge:** `d95f6183c93932fba7f5ac10f421ce6abbe1f044`  
**Public-truth sync before release:** `5078379d32dff352cce8478930fcbd023cb8951e`

The shipped inspector is deliberately read-only:

- exact provider-native S3 object identity and `HeadObject` facts
- size, last modified, ETag, content type, storage class, metadata, endpoint identity, and version ID only when returned by the backend
- paginated `ListObjectsV2` bucket/prefix LiveScan
- observed object count and logical bytes
- bounded largest-object ranking
- bounded immediate-prefix ranking
- bounded prefix and storage-class cardinality
- age and storage-class distributions
- progress, cancellation, and truthful partial-state handling
- reuse of the existing `ProviderRegistry`, per-target `S3Provider`/AWS client, `JobManager`, and UI architecture
- no S3 cleanup/lifecycle-management mutation UI
- no fake capacity/free-space/`df` semantics
- no invented billing/cost data

Real MinIO acceptance explicitly exercises `tests/s3_inspector_minio.rs`. AWS S3 remains the supported product path. Cloudflare R2 / Wasabi remain best-effort/unverified.

## WEBDAV POST-MVP — ISSUE #13

Released and retained in v0.24.0:

- core WebDAV provider semantics
- Apache mod_dav W1–W18 physical acceptance
- Nextcloud 34.0.2 and ownCloud 11.0.0 I1–I12 interoperability
- RFC-compatible MOVE behavior
- exact recursive WebDAV collection download → Local (#248 / PR #250)
- recursive Local → WebDAV upload (#253 / PR #254)
- safe bounded recursive WebDAV delete (#255 / PR #256)
- one-job multi-root Local↔WebDAV F5 Copy (#257 / PR #258)

Remaining enhancements under #13:

- [ ] multi-root recursive WebDAV delete
- [ ] WebDAV→WebDAV recursive/cross-target copy with exact source/target identity and truthful staging/recovery semantics
- [ ] WebDAV→WebDAV move only after copy → verify → delete-source semantics are separately proven
- [ ] optional metadata/property mutation only for a demonstrated admin use case
- [ ] Digest/Bearer auth only if interoperability evidence requires it

These are enhancements, not regressions or current release blockers.

## RELEASED — SFTP → SFTP WORKSPACE SYNC

Issue #269 was completed by PR #270 and feature-merged as `fe413aecfdc3bf5685849e73b396800f7f3ab7e0`. The focused release package in #271 published it as **v0.24.0** on release commit/tag target `6d413fac5d5b493859bfadfbedbeb436b1140e0b`.

Accepted truth:

- exact source and destination SFTP host/path identities
- explicit same-host and cross-host execution truth
- bounded remote → ARX → remote file streaming through the existing SFTP transfer authority
- no server-side copy/rename fiction and no SFTP→SFTP Move expansion
- existing workspace-sync controller, `ProviderRegistry`, Job lifecycle, retry/recovery model, journal, frozen Preview and post-execution verification retained
- SFTP mkdir/delete routed through existing provider mutation seams
- stale source/destination fail closed before mutation
- cancellation and partial/recovery truth retained
- permanent two-endpoint OpenSSH physical lane with strict host-key checking
- exact feature head `9656102bc679b71ca49513b37735bc79a3874a91`; squash merge `fe413aecfdc3bf5685849e73b396800f7f3ab7e0`
- exact-head CI / WebDAV interop / SFTP physical all success; feature post-merge CI run `33002995697` and SFTP physical run `33002995516` success
- v0.24.0 release candidate `fd798b3117bec1f782f53cbedd48bdab72eaab08`; release commit/tag target `6d413fac5d5b493859bfadfbedbeb436b1140e0b`
- v0.24.0 release workflow `33012256020` — validate success, publish success

The focused release package was published as **v0.24.0**; #271 records the release acceptance and closure evidence.

## RECOMMENDED FEATURE SEQUENCE

1. **Next:** WebDAV → WebDAV recursive/cross-target copy under #13, only after exact source/target and recovery semantics are frozen.
2. **Later:** general safe cross-provider Move modeled as copy → verify → delete-source.
3. Other candidates: binary remote editing, additional Linux architectures, signed repositories, provider-specific read-only analytics where evidence is truthful.

Native Windows support remains out of scope. Windows SSH clients may interoperate with ARX running on Linux; that does not change the Linux-only product policy.

## RELEASE HISTORY

### v0.24.0 — 2026-08-26

- SFTP → SFTP Workspace Sync for same-host and cross-host roots
- bounded remote → ARX → remote streaming with frozen Preview and verification
- real two-endpoint OpenSSH physical acceptance
- v0.23.0 S3 Inspector and v0.22.0 WebDAV recursive surface retained

### v0.23.0 — 2026-08-26

- read-only S3 Object & Bucket Inspector
- exact `HeadObject` facts
- bounded paginated `ListObjectsV2` LiveScan
- cancellation/partial truth and bounded aggregation memory
- AWS S3 + real MinIO acceptance

### v0.22.0 — 2026-08-26

- recursive Local → WebDAV upload
- safe bounded recursive WebDAV delete
- multi-root Local↔WebDAV F5 Copy
- Apache / Nextcloud / ownCloud acceptance retained

### v0.21.0

- exact recursive WebDAV collection download
- Nextcloud / ownCloud certification
- WebDAV target/source truth and MOVE interoperability fixes

### v0.20.0

- Local Storage Inspector / Filesystems
- Transfer Center v2
- tar.gz / DEB / RPM release contract

### v0.19.0

- persistent bounded FIFO Transfer Queue
- concurrency, progress/rate/ETA, Pause/Resume/Cancel
- bounded safe retry and ambiguity/recovery classification

### v0.18.0

- WebDAV MVP with PROPFIND / GET / PUT / DELETE / MKCOL / COPY / MOVE
- Basic auth through keyring/environment secret resolution
- raw-href authority and Apache physical acceptance

## RELEASE PROCESS POLICY

For future releases:

1. freeze scope and reconcile fresh `main`
2. prepare one release-candidate branch
3. change release truth only unless a genuine blocker needs separate review
4. require exact-head standard and affected physical gates
5. validate one release ELF and exact package payloads
6. merge with pinned expected head
7. require post-merge CI success
8. create a new immutable tag on the accepted release commit
9. publish from the validated artifact bundle without rebuilding
10. independently verify tag target, Release state, assets, checksums, packaged binary version, and prior-tag immutability

Never repurpose a published version identity.

## DEVELOPMENT POLICY

- Prefer user-visible vertical slices over architecture churn.
- Implement one selected feature as a coherent macro-batch rather than many micro-tasks.
- Keep product/runtime/documentation/physical acceptance changes for one capability together when logically required.
- Use exact-head CI and physical evidence as acceptance gates.
- Fail closed when provider identity, capability, mutation safety, or recovery truth is ambiguous.
- Never fabricate progress, rate, ETA, capacity, cost, or provider semantics.
- Do not weaken production behavior merely to satisfy tests.
- `Cargo.lock` remains authoritative; release-only version bumps must not alter unrelated dependency resolution or registry checksums.
