# ARX Roadmap

GitHub state is authoritative over this document. Re-fetch current `main`, issues, PRs, tags, workflow state, and releases before acting on any recorded SHA or backlog item.

## CURRENT — v0.25.0 release candidate / v0.24.0 published

**Current public release until the new tag is published:** `v0.24.0`  
**Immutable v0.24.0 tag target:** `6d413fac5d5b493859bfadfbedbeb436b1140e0b`  
**v0.25.0 release issue:** #281  
**v0.25.0 feature baseline before release preparation:** `main` → `d603281aca350749e3e195752c17295ab1638bc6`  
**Platform:** Linux only; published target Linux x86_64  
**MSRV:** Rust 1.88

The v0.25.0 release candidate is a **release/public-truth slice only**. It publishes three WebDAV capabilities that were already independently frozen, implemented, physically accepted, merged, and post-merge accepted after v0.24.0:

1. WebDAV → WebDAV recursive/cross-target Copy — #275 / PR #276.
2. Multi-root recursive WebDAV Delete — #277 / PR #278.
3. Verified WebDAV → WebDAV Move — #279 / PR #280.

The release does not introduce a new provider/runtime/retry/scheduler/secret authority, dependency change, MSRV change, or unrelated feature.

Publication truth remains separate from source-tree version metadata. Until an immutable `v0.25.0` tag is created on an accepted release commit and the tag-triggered Release workflow publishes successfully, `v0.24.0` remains the current public release.

## CURRENT PRODUCT TRUTH

- **Local / SFTP:** browsing, transactional copy, bounded preview, SFTP conflict-safe text Remote Edit, OpenSSH-backed host/session behavior.
- **Remote Workspace:** v0.24.0 provides Compare → Preview → Execute → Verify for Local→Local, Local→SFTP, SFTP→Local, and same-host/cross-host SFTP→SFTP sync with bounded remote → ARX → remote streaming and real two-endpoint OpenSSH acceptance.
- **Transfer Queue:** one persistent bounded FIFO runtime, configurable concurrency `1..=8`, truthful progress/rate/ETA where known, cooperative Pause/Resume/Cancel, and bounded safe retry.
- **Transfer Center v2:** Active / History / All views and controls routed to the existing `TransferQueueRuntime`.
- **Storage Inspector (`Alt+U`):** Local read-only logical/allocated usage plus exact S3 object inspection and bounded paginated bucket/prefix LiveScan analytics.
- **Filesystems (`Alt+D`):** read-only Linux capacity/inode view. S3 never receives fabricated filesystem capacity, inode, free-space, or `df` semantics.
- **Effective keymap:** one conflict-safe effective runtime map with user overrides and `arx --print-keymap` discovery.
- **Mouse / split panes / terminal:** visible-row-correct mouse behavior, vertical+horizontal split panes, typed tmux/GNU Screen lifecycle.
- **Typed local Quick Actions:** SHA-256, Touch, Compress-to-tar.gz plus mkdir/chmod/symlink surface.
- **S3:** AWS S3 + MinIO supported paths; Moto emulated; exact object and bounded bucket/prefix inspector shipped in v0.23.0; Cloudflare R2 / Wasabi remain unverified best-effort targets.
- **WebDAV interoperability:** Apache mod_dav, Nextcloud 34.0.2-apache, and ownCloud 11.0.0 physically accepted through Basic auth.
- **WebDAV Local↔remote recursive operations:** recursive WebDAV→Local download, Local→WebDAV upload, safe bounded recursive delete foundation, and one-job multi-root Local↔WebDAV F5 Copy.
- **Accepted post-v0.24 WebDAV transaction surface:** one-root recursive same/cross-target WebDAV→WebDAV Copy; multi-root recursive WebDAV Delete; one-root verified same/cross-target WebDAV→WebDAV Move through copy → verify → frozen-source delete.
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

## ACCEPTED — WEBDAV TRANSACTION SURFACE FOR v0.25.0

### Recursive WebDAV → WebDAV Copy — #275 / PR #276

**Accepted feature head:** `a4c6c16f68ba61d78774d34d3eb434e9fcc7c381`  
**Squash merge:** `8e58381a88c85584c47407083e9c13599e1ac01c`

Accepted truth:

- exactly one exact current-listed WebDAV collection root;
- one new WebDAV destination root;
- same-target different-root and cross-target support through the same bounded WebDAV → ARX → WebDAV streaming model;
- complete bounded source manifest before mutation and full revalidation immediately before mutation;
- provider-native raw href identity remains authoritative;
- destination root is noclobber/attempt-owned;
- ambiguous destination mutation is not blindly replayed;
- independent exact destination verification is required before success;
- failed cleanup or unresolved mutation certainty is `RecoveryRequired`;
- no claim of server-side recursive COPY.

### Multi-root recursive WebDAV Delete — #277 / PR #278

**Accepted feature head:** `8bedf8d14397099d361f26fdb55b79b68252d809`  
**Squash merge:** `58c657e7d380d0b62a4fa494c325fcc62db6295b`

Accepted truth:

- one or more exact current-selected WebDAV collection roots;
- selection freezes exact provider-native identities; display names are not addresses;
- aggregate planning and whole-batch revalidation complete before the first DELETE;
- aggregate cap is 50,000 planned items and is enforced while manifests are accumulated;
- roots execute deterministically and sequentially;
- descendants remain child-first/root-last with fresh exact collection-empty proof;
- global item progress, cancellation, definitive partial failure, and recovery state remain truthful;
- ambiguous DELETE immediately becomes `RecoveryRequired`, with no replay or later-root continuation.

### Verified WebDAV → WebDAV Move — #279 / PR #280

**Accepted feature head:** `735135beb473b54925d1912942e74d7299593e1b`  
**Squash merge / v0.25 feature baseline:** `d603281aca350749e3e195752c17295ab1638bc6`

Transaction truth:

`freeze source → bounded copy → independently verify destination → revalidate source + destination → delete exact already-frozen source manifest`

- exactly one exact current-listed WebDAV collection root;
- same-target and cross-target use the same ARX-mediated transaction;
- copy success alone is never Move success;
- copied-source and destructive-delete snapshots must describe the same exact source identity set before destination mutation;
- source drift before first source DELETE leaves source untouched;
- destination drift/uncertainty prevents source deletion and becomes `RecoveryRequired`;
- source deletion consumes the already frozen manifest through the existing `MutationService` authority and cannot absorb newly appeared children;
- once the first source DELETE succeeds, the verified destination is committed and is never rolled back;
- definitive later source deletion/cancellation is truthful partial state and `NeverRetry`;
- ambiguous source DELETE is issued once and becomes `RecoveryRequired` with no replay;
- no server-side recursive MOVE transaction authority.

The Apache acceptance matrix also established URI canonical-match truth: valid percent-escape hex digits are case-insensitive (`%C3%A9` equals `%c3%a9`) while ARX never percent-decodes path bytes, collapses separators, or drops query identity. Raw server href remains authoritative after matching.

Exact #279 acceptance included CI `33054126864`, WebDAV Remote Copy Physical `33054126792`, SFTP Workspace Sync Physical `33054126837`, and Nextcloud/ownCloud WebDAV Interoperability `33054126797`; post-merge exact-main CI `33054582669`, WebDAV Remote Copy Physical `33054582657`, and SFTP Workspace Sync Physical `33054582642` also succeeded.

## WEBDAV POST-MVP — ISSUE #13

Released and retained before v0.25.0:

- core WebDAV provider semantics;
- Apache mod_dav W1–W18 physical acceptance;
- Nextcloud 34.0.2 and ownCloud 11.0.0 I1–I12 interoperability;
- RFC-compatible one-resource MOVE behavior;
- exact recursive WebDAV collection download → Local (#248 / PR #250);
- recursive Local → WebDAV upload (#253 / PR #254);
- safe bounded recursive WebDAV delete foundation (#255 / PR #256);
- one-job multi-root Local↔WebDAV F5 Copy (#257 / PR #258).

Completed on current main and selected for v0.25.0 publication:

- [x] WebDAV→WebDAV recursive/cross-target Copy — #275 / PR #276;
- [x] multi-root recursive WebDAV Delete — #277 / PR #278;
- [x] verified WebDAV→WebDAV Move — #279 / PR #280.

Remaining #13 items are deliberately evidence-driven, not an automatic feature queue:

- [ ] optional metadata/property mutation — only for a demonstrated admin use case;
- [ ] Digest/Bearer auth — only if concrete interoperability evidence shows Basic/app-password paths are insufficient.

No next WebDAV feature slice is selected merely because the previous one completed.

## RELEASED — SFTP → SFTP WORKSPACE SYNC

Issue #269 was completed by PR #270 and feature-merged as `fe413aecfdc3bf5685849e73b396800f7f3ab7e0`. The focused release package published it as **v0.24.0** on immutable tag target `6d413fac5d5b493859bfadfbedbeb436b1140e0b`.

Accepted truth:

- exact source and destination SFTP host/path identities;
- same-host and cross-host execution truth;
- bounded remote → ARX → remote streaming through the existing SFTP transfer authority;
- no server-side copy/rename fiction and no SFTP→SFTP Move expansion;
- existing workspace-sync controller, `ProviderRegistry`, Job lifecycle, retry/recovery model, journal, frozen Preview and post-execution verification retained;
- permanent two-endpoint OpenSSH physical lane with strict host-key checking.

## RELEASED — S3 OBJECT & BUCKET INSPECTOR

Issue #264 was completed by PR #265 and published in v0.23.0.

The inspector remains deliberately read-only:

- exact provider-native S3 object identity and `HeadObject` facts;
- paginated bounded `ListObjectsV2` bucket/prefix LiveScan;
- observed object count and logical bytes;
- bounded rankings/distributions;
- progress, cancellation, and truthful partial-state handling;
- reuse of existing `ProviderRegistry`, per-target `S3Provider`/AWS client, `JobManager`, and UI architecture;
- no S3 cleanup/lifecycle-management mutation UI;
- no fake capacity/free-space/`df` semantics;
- no invented billing/cost data.

Real MinIO acceptance exercises `tests/s3_inspector_minio.rs`. AWS S3 remains the supported product path. Cloudflare R2 / Wasabi remain best-effort/unverified.

## NEXT DECISION AFTER v0.25.0

Do not infer a next feature automatically. After v0.25.0 publication, choose the next user-visible slice from fresh evidence rather than extending provider surfaces speculatively.

Evidence-driven candidates include:

1. a concrete admin-driven WebDAV metadata/property use case, if one appears;
2. Digest/Bearer auth only if a real supported deployment proves it necessary;
3. binary remote editing only with a coherent conflict/safety contract;
4. additional Linux architectures or signed distribution only with clear deployment demand;
5. provider-specific read-only analytics where backend facts are truthful.

Native Windows support remains out of scope. Windows SSH clients may interoperate with ARX running on Linux; that does not change the Linux-only product policy.

## RELEASE HISTORY

### v0.25.0 — release candidate

- recursive same-target/cross-target WebDAV → WebDAV Copy;
- multi-root recursive WebDAV Delete;
- verified one-root same-target/cross-target WebDAV → WebDAV Move;
- percent-escape hex-case canonical matching correction without percent-decoding;
- no dependency/MSRV/configuration migration change.

Publication is not claimed until the immutable tag and GitHub Release are verified.

### v0.24.0 — 2026-08-26

- SFTP → SFTP Workspace Sync for same-host and cross-host roots;
- bounded remote → ARX → remote streaming with frozen Preview and verification;
- real two-endpoint OpenSSH physical acceptance;
- v0.23.0 S3 Inspector and v0.22.0 WebDAV recursive surface retained.

### v0.23.0 — 2026-08-26

- read-only S3 Object & Bucket Inspector;
- exact `HeadObject` facts;
- bounded paginated `ListObjectsV2` LiveScan;
- cancellation/partial truth and bounded aggregation memory;
- AWS S3 + real MinIO acceptance.

### v0.22.0 — 2026-08-26

- recursive Local → WebDAV upload;
- safe bounded recursive WebDAV delete;
- multi-root Local↔WebDAV F5 Copy;
- Apache / Nextcloud / ownCloud acceptance retained.

### v0.21.0

- exact recursive WebDAV collection download;
- Nextcloud / ownCloud certification;
- WebDAV target/source truth and MOVE interoperability fixes.

### v0.20.0

- Local Storage Inspector / Filesystems;
- Transfer Center v2;
- tar.gz / DEB / RPM release contract.

### v0.19.0

- persistent bounded FIFO Transfer Queue;
- concurrency, progress/rate/ETA, Pause/Resume/Cancel;
- bounded safe retry and ambiguity/recovery classification.

### v0.18.0

- WebDAV MVP with PROPFIND / GET / PUT / DELETE / MKCOL / COPY / MOVE;
- Basic auth through keyring/environment secret resolution;
- raw-href authority and Apache physical acceptance.

## RELEASE PROCESS POLICY

For future releases:

1. freeze scope and reconcile fresh `main`;
2. prepare one release-candidate branch;
3. change release truth only unless a genuine blocker needs separate review;
4. require exact-head standard and affected physical gates;
5. validate one release ELF and exact package payloads;
6. merge with pinned expected head;
7. require post-merge CI/affected physical success;
8. create a new immutable tag on the exact accepted release commit;
9. publish from the validated artifact bundle without rebuilding;
10. independently verify tag target, Release state, assets, checksums, packaged binary version, and prior-tag immutability.

Never repurpose or retarget a published version identity.

## DEVELOPMENT POLICY

- Prefer user-visible vertical slices over architecture churn.
- Implement one selected feature as a coherent macro-batch rather than many micro-tasks.
- Keep product/runtime/documentation/physical acceptance changes for one capability together when logically required.
- Use exact-head CI and physical evidence as acceptance gates.
- Fail closed when provider identity, capability, mutation safety, or recovery truth is ambiguous.
- Never fabricate progress, rate, ETA, capacity, cost, or provider semantics.
- Do not weaken production behavior merely to satisfy tests.
- `Cargo.lock` remains authoritative; release-only version bumps must not alter unrelated dependency resolution or registry checksums.
