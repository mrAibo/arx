# ARX Development Handoff

This document is the canonical continuation point for active ARX development. It is intentionally compact: a new development session should be able to recover current product truth, frozen architecture rules, release state, and the next work sequence without reconstructing chat history.

> **Authority rule:** live GitHub state wins over this file. Re-fetch current `main`, open issues/PRs, workflow state, tags, and releases before acting on any SHA or publication statement recorded here. Published tags are immutable evidence.

## 1. Current release line and baseline

Repository: `mrAibo/arx`

- Release version in this source tree: **v0.23.0**
- Previous immutable public release: **v0.22.0**
- v0.22.0 tag target: `8737bbd2afaf0d6e7146a5d8c59ee1a0606325bf`
- Accepted pre-release main baseline: `5078379d32dff352cce8478930fcbd023cb8951e`
- Rust MSRV: **1.88**
- Product platform: **Linux only**
- Published target: **Linux x86_64**
- Release assets: tar.gz, `.deb`, `.rpm`, and `SHA256SUMS`
- Release publication uses one validated ELF and reuses the validated artifact bundle; do not rebuild between validation and publication.

v0.23.0 packages the already accepted read-only S3 Object & Bucket Inspector on top of the v0.22.0 product baseline. The live GitHub tag, Release workflow, and Release object are authoritative for whether publication has completed; do not infer publication merely from the version in this source tree.

## 2. Current phase

The immediate phase is **v0.23.0 release execution**, not another architecture pack or feature expansion.

Release sequence:

1. keep the release candidate limited to version/lockfile/release-note/public-truth changes
2. keep the accepted S3 Inspector runtime frozen unless a genuine correctness/safety blocker is discovered
3. require exact-head standard CI and Release PR validation
4. merge only the reviewed exact PR head
5. require post-merge CI before tagging
6. create immutable `v0.23.0` on the accepted main commit
7. let the existing tag-triggered Release workflow validate and publish the one-build artifact bundle
8. independently verify tag target, Release state, assets, checksums, binary version, and v0.22.0 tag immutability
9. only after release acceptance, freeze the next major feature: **SFTP → SFTP workspace synchronization**

Do not begin SFTP→SFTP implementation by silently extending the release slice. Freeze a dedicated issue/contract first.

## 3. v0.23.0 product truth

v0.23.0 retains the v0.22.0 product surface and adds the accepted S3 Object & Bucket Inspector.

Shipped product surface:

- Local / SFTP browsing, transactional copy, bounded preview, and SFTP conflict-safe text Remote Edit
- Remote Workspace Compare → Preview → Execute → Verify for Local→Local, Local→SFTP, and SFTP→Local
- persistent bounded Transfer Queue with concurrency `1..=8`, truthful progress/rate/ETA where known, Pause/Resume/Cancel, bounded safe retry, and Transfer Center views
- Local Storage Inspector and Linux Filesystems views
- conflict-safe effective keymap + `arx --print-keymap`
- typed local Quick Actions (SHA-256, Touch, Compress to tar.gz)
- hardened tmux / GNU Screen lifecycle and split-pane/mouse behavior
- AWS S3 + MinIO supported paths with Moto emulated evidence; R2/Wasabi best-effort/unverified
- WebDAV Basic-auth provider path with Apache mod_dav, Nextcloud 34.0.2, and ownCloud 11.0.0 physical acceptance
- recursive WebDAV → Local download
- recursive Local → WebDAV upload
- safe bounded recursive WebDAV delete for one exact collection
- multi-root Local↔WebDAV F5 Copy as one queued job
- Linux x86_64 tar.gz / DEB / RPM distribution from one validated ELF

### S3 Object & Bucket Inspector included in v0.23.0

Issue #264 / PR #265 added:

- exact provider-native object identity
- `HeadObject` facts only: size, last modified, ETag, content type, storage class, metadata, endpoint identity, version ID when returned
- paginated `ListObjectsV2` bucket/prefix LiveScan
- observed object count and logical bytes
- bounded largest-object ranking
- bounded immediate-prefix ranking
- bounded prefix and storage-class cardinality
- age and storage-class distributions
- progress, cancellation, and truthful partial-state handling
- no S3 cleanup/mutation UI in this slice
- no fabricated POSIX capacity/free-space/`df` semantics
- no invented cost/billing data
- no second provider registry, S3 client cache, scheduler, or JobManager

Accepted implementation head: `d0ceb64781777bd04fe51aeaff9b8d3dfa3c3343`  
Squash merge on main: `d95f6183c93932fba7f5ac10f421ce6abbe1f044`  
Public-truth sync merge: `5078379d32dff352cce8478930fcbd023cb8951e`

Final exact-head feature CI and post-merge push CI were green across Quality, Rust 1.88 MSRV, MinIO/S3 physical acceptance, Apache WebDAV physical regression, and real PTY multiplexer acceptance. The MinIO lane explicitly executed `tests/s3_inspector_minio.rs` and proved the real object + prefix inspector path.

SFTP→SFTP workspace synchronization remains intentionally unsupported in v0.23.0.

## 4. Architecture status

The O → P → Q → R architecture sequence is complete.

| Pack | Scope | Status |
|---|---|---|
| O | typed local Quick Actions | COMPLETE |
| P | TUI decomposition | COMPLETE |
| Q | VFS convergence / resolver authority | COMPLETE |
| R | internal feature/command registration | COMPLETE |

External plugins remain **no GO**. `arx.menu` is the supported lightweight extension path.

### Frozen VFS authority model

```text
Location          = typed identity / address / navigation
ProviderRegistry  = explicit provider execution authority
CapabilitySet     = exact-location / concrete-instance capability truth
VfsProvider       = backend provider interface
```

Two deliberate resolver seams remain distinct:

- `ProviderRegistry::provider_for_location` — string-path backend operations / legacy listing path authority.
- `ProviderRegistry::provider_for_page_location` — typed page/native-identity operations for exact targets.

Do not merge these casually, add a third resolver authority, or reconstruct provider-native identity from display strings.

### Runtime authority rules

Do not introduce a second:

- ProviderRegistry
- TransferQueueRuntime
- JobManager
- EffectDispatcher
- scheduler
- retry authority
- secret store

Transfer execution remains owned by TransferPlanner / Transfer Queue / executor seams. Mutations remain routed through the existing typed mutation/provider authorities.

## 5. WebDAV truth

Physically accepted targets:

- Apache mod_dav — W1–W18 plus recursive download/upload/delete and multi-root F5 coverage
- Nextcloud 34.0.2-apache — I1–I12 plus recursive download/upload/delete and multi-root F5 coverage
- ownCloud 11.0.0 — I1–I12 plus recursive download/upload/delete and multi-root F5 coverage

Basic auth through keyring/environment-backed secrets is the shipped path.

Still intentionally unsupported / enhancement-only under #13:

- multi-root recursive WebDAV delete
- WebDAV→WebDAV recursive/cross-target copy or move
- metadata/property mutation without a demonstrated admin use case
- Digest/Bearer auth without interoperability evidence requiring it

Do not treat these as v0.23.0 release blockers unless live GitHub state explicitly changes that decision.

## 6. Recommended next sequence

### A. Finish v0.23.0 release

The candidate should remain minimal:

- Cargo package and root Cargo.lock version `0.23.0`
- `docs/releases/v0.23.0.md`
- README / ROADMAP / canonical handoff release truth synchronized
- no runtime, dependency, workflow, or package-logic changes unless a separately reviewed blocker appears
- exact-head standard CI and Release PR validation green
- independent exact-head diff review
- pinned merge using the reviewed head SHA
- post-merge push CI green
- immutable `v0.23.0` tag on accepted main
- existing tag-triggered workflow publishes tar.gz / DEB / RPM / `SHA256SUMS`
- independent post-publish verification of tag target, Release state, assets, checksums, binary `arx 0.23.0`, and v0.22.0 immutability

### B. After v0.23.0 — SFTP → SFTP workspace sync

Freeze a dedicated issue before implementation. Extend the existing Compare → Preview → Execute → Verify model rather than creating another synchronization engine.

Required design concerns:

- exact source and destination SFTP host/path identity
- same-host vs cross-host execution truth
- deterministic transfer/mutation ordering
- Preview before destructive Mirror consequences
- destination verification after execution
- cooperative cancellation and truthful partial completion
- recovery/ambiguity boundaries
- reuse of existing workspace-sync controller, Transfer Queue, JobManager, provider registry, retry authority, and verification model
- never pretend a cross-host transfer is a server-side rename

### C. Later candidates

- WebDAV→WebDAV recursive copy under exact source/target semantics
- general safe cross-provider Move modeled as copy → verify → delete-source
- binary remote editing
- additional Linux architectures / signed repositories if justified

Create/freshen dedicated issues before implementing roadmap-only items.

## 7. External plugins — explicit decision gate

There is no GO for Lua, WASM, or native `.so` plugins.

Do not reintroduce a general plugin runtime unless real user/ecosystem demand exists and a reviewed enforcement model is available. A manifest permission is not a security boundary if arbitrary native code runs as the same OS user.

Core mutation/provider authority must remain inside the existing trusted runtime.

## 8. Engineering and acceptance invariants

Standard code gates:

```bash
cargo fmt --check
cargo check --locked --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-features
cargo +1.88 check --locked --all-features
git diff --check
```

Additional rules:

- exact SHA pinning for acceptance evidence
- exact-head CI is authoritative
- provider/transfer semantic changes require their affected physical lanes
- WebDAV semantic changes require Apache acceptance and, where portable behavior is affected, Nextcloud/ownCloud interoperability
- S3 semantic changes require MinIO physical evidence
- multiplexer terminal lifecycle changes require real PTY evidence
- fail closed when identity/capability/safety is ambiguous
- never fabricate progress, rate, ETA, capacity, cost, or provider semantics
- do not change runtime semantics merely to satisfy timing-sensitive tests
- destructive/remote mutation paths require truthful transaction, cancellation, retry, and recovery behavior
- `Cargo.lock` remains authoritative; release-only version bumps must not alter unrelated dependency checksums or resolution

## 9. Collaboration model

- **ChatGPT** owns architecture, connected-source recon, code/diff review, GitHub PR/merge/release operations, CI interpretation, and final acceptance decisions.
- **Hermes Agent** is a secondary Linux executor for deterministic shell-oriented work that connected tooling cannot perform.
- Prefer one coherent Hermes `/goal` or macro-task over repeated microtasks.
- Hermes must not independently redefine architecture/scope, open/merge PRs, close issues, move published tags, or publish releases unless authority is explicitly changed by the user.

## 10. Release policy

For feature releases:

1. freeze scope
2. reconcile fresh `main`
3. prepare one release-candidate branch
4. version/package/release-note truth only unless a genuine blocker requires a separately reviewed product fix
5. run exact-head standard + affected physical gates
6. validate release packages from one release ELF
7. merge with pinned expected head
8. require post-merge CI success
9. create a new immutable tag on the accepted main commit
10. allow the existing tag-triggered Release workflow to publish validated artifacts
11. independently verify tag target, release state, assets, checksums, packaged binary version, and prior-tag immutability

Never repurpose an already-published version identity.

## 11. New-session startup checklist

Before changing code:

1. Read this file, `ROADMAP.md`, and `ARCHITECTURE.md`.
2. Fetch current `main`; do not assume the SHA recorded here is still current.
3. Query open issues and PRs from GitHub.
4. Query the current release/tag state and treat published tags as immutable evidence.
5. Confirm whether the request is release work, bugfix, or a newly approved feature slice.
6. Check affected source and tests before freezing semantics.
7. Use connected GitHub tooling for review/PR/merge/CI whenever available.
8. Use Hermes only where Linux-local execution materially helps.

Minimal continuation prompt:

> Continue development of `github.com/mrAibo/arx`. Read `docs/DEVELOPMENT_HANDOFF.md`, `ROADMAP.md`, and `ARCHITECTURE.md`; treat live GitHub state as authoritative. The source tree release line is v0.23.0 and includes the accepted S3 Object & Bucket Inspector from #264/#265; verify live GitHub state to determine whether publication is complete. Preserve the frozen architecture authorities. Finish any remaining v0.23.0 release gates before beginning the next major feature, then freeze SFTP→SFTP workspace sync as a dedicated slice. Use Hermes only for deterministic Linux-local execution.
