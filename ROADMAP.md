# ARX Roadmap

GitHub state is authoritative over this document. Re-fetch current `main`, issues, PRs, tags, workflow state, and releases before acting on any recorded SHA or backlog item.

## CURRENT — v0.23.0 published / v0.24.0 release preparation

**Current public release:** `v0.23.0`  
**Release tag target:** `f66a25f3f2b4fb66832ecc50d85f9f105ebba086`  
**Published:** 2026-08-26  
**Platform:** Linux only; published target Linux x86_64  
**MSRV:** Rust 1.88  
**Previous immutable release:** `v0.22.0` → `8737bbd2afaf0d6e7146a5d8c59ee1a0606325bf`
**Current main:** `cd5f1b147ac4ca78d7fdab134d548b97a7c20a00` — public-truth baseline after PR #272
**Active release candidate:** `v0.24.0` via #271 / `release/v0.24.0-prep`; source version metadata does not imply publication

v0.23.0 ships the read-only **S3 Object & Bucket Inspector** on top of the v0.22.0 WebDAV recursive-operation baseline.

Release acceptance evidence:

- release PR: #267, squash-merged as `f66a25f3f2b4fb66832ecc50d85f9f105ebba086`
- exact-head CI on `e0264de19c58c8e22be573ddad6af2734cec9a6c`: run `32990258665` — success
- exact-head Nextcloud/ownCloud interoperability: run `32990258700` — success
- post-merge CI on `f66a25f3...`: run `32991066627` — success
- release workflow on `v0.23.0`: run `32994286262` — validate success, publish success
- tag ↔ Cargo version, release notes, hero asset, Format, Clippy, tests, Rust 1.88 MSRV, one release build, third-party licenses, TAR/DEB/RPM payloads, packaged-binary smoke, checksums, and artifact publication all passed
- published Release is latest, `draft=false`, `prerelease=false`
- independent artifact verification: all three package hashes match `SHA256SUMS`; tarball binary reports `arx 0.23.0`; `--help` exits successfully
- v0.22.0 tag remained unchanged throughout the release

Published artifacts:

- `arx-v0.23.0-x86_64-unknown-linux-gnu.tar.gz`
- `arx_0.23.0_amd64.deb`
- `arx-0.23.0-1.x86_64.rpm`
- `SHA256SUMS`

Published SHA-256 values:

- tar.gz: `67946d5cbf19130f8c9783cd9a18377ec5586bb6df84d8f9e9dc0840162f69cc`
- DEB: `b00af1a1da4a65548fc1fe43e8ca7144686433eddf7a1e39733d181b2dc6c8fc`
- RPM: `278d4d3733aed26b801e971852315859f933cf9092c6481dc8313bd5d4f782e5`

## CURRENT PRODUCT TRUTH

- **Local / SFTP:** browsing, transactional copy, bounded preview, SFTP conflict-safe text Remote Edit, OpenSSH-backed host/session behavior.
- **Remote Workspace:** published v0.23.0 provides Compare → Preview → Execute → Verify for Local→Local, Local→SFTP, and SFTP→Local; the v0.24.0 release candidate adds the already accepted SFTP→SFTP sync with same-host/cross-host bounded streaming and real two-endpoint OpenSSH acceptance.
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

Released and retained in v0.23.0:

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
- [ ] WebDAV→WebDAV recursive/cross-target copy or move with truthful target/recovery semantics
- [ ] optional metadata/property mutation only for a demonstrated admin use case
- [ ] Digest/Bearer auth only if interoperability evidence requires it

These are enhancements, not regressions or current release blockers.

## ACCEPTED / UNRELEASED — SFTP → SFTP WORKSPACE SYNC

Issue #269 was completed by PR #270 and feature-merged as `fe413aecfdc3bf5685849e73b396800f7f3ab7e0`. Public-truth baseline `cd5f1b147ac4ca78d7fdab134d548b97a7c20a00` retains the feature. It is **not part of published v0.23.0** and is the sole major feature targeted by the v0.24.0 release candidate.

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
- exact-head CI / WebDAV interop / SFTP physical all success; post-merge CI run `33002995697` and post-merge SFTP physical run `33002995516` success

The focused release package is tracked in #271 and targets **v0.24.0** without another major feature.

## RECOMMENDED FEATURE SEQUENCE

1. **Next:** publish focused v0.24.0 with the already accepted SFTP→SFTP Workspace Sync.
2. **After v0.24.0:** WebDAV → WebDAV recursive copy, only after exact source/target and recovery semantics are frozen.
3. **Later:** general safe cross-provider Move modeled as copy → verify → delete-source.
4. Other candidates: binary remote editing, additional Linux architectures, signed repositories, provider-specific read-only analytics where evidence is truthful.

Native Windows support remains out of scope. Windows SSH clients may interoperate with ARX running on Linux; that does not change the Linux-only product policy.

## RELEASE HISTORY

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
