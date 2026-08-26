# ARX Development Handoff

This document is the canonical continuation point for active ARX development. It is intentionally compact: a new development session should be able to recover current product truth, frozen architecture rules, release state, and the next work sequence without reconstructing chat history.

> **Authority rule:** live GitHub state wins over this file. Re-fetch current `main`, open issues/PRs, workflow state, and releases before acting on any SHA or backlog statement recorded here. Published tags are immutable evidence; current `main` may be ahead of the latest release.

## 1. Current release and main baseline

Repository: `mrAibo/arx`

- Current public release: **v0.22.0**
- v0.22.0 tag target: `8737bbd2afaf0d6e7146a5d8c59ee1a0606325bf`
- Published: 2026-08-26
- Accepted post-release `main` after S3 Inspector merge: `d95f6183c93932fba7f5ac10f421ce6abbe1f044`
- Rust MSRV: **1.88**
- Product platform: **Linux only**
- Published target: **Linux x86_64**
- Release assets: tar.gz, `.deb`, `.rpm`, and `SHA256SUMS`
- Release publication uses one validated ELF and reuses the validated artifact bundle; do not rebuild between validation and publication.

v0.22.0 remains the immutable published baseline. `main` is currently ahead of it because issue #264 / PR #265 added the read-only S3 Object & Bucket Inspector after the release.

## 2. Current phase

The immediate phase is **release preparation / public-truth synchronization**, not another architecture pack.

Current direction:

1. keep README, ROADMAP, this handoff, and live GitHub state synchronized
2. keep the merged S3 Inspector runtime frozen unless a genuine correctness/safety blocker is discovered
3. prefer a minimal **v0.23.0** release before starting another major feature
4. after that release, the preferred next major feature is **SFTP → SFTP workspace synchronization**
5. preserve exact-head CI and affected physical provider acceptance

Do not begin SFTP→SFTP implementation by silently extending the S3 or release slice. Freeze a dedicated issue/contract first.

## 3. Current product truth

### Published in v0.22.0

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

### Completed on current main after v0.22.0

Issue #264 / PR #265 added **S3 Object & Bucket Inspector**:

- exact provider-native object identity
- `HeadObject` facts only: size, last modified, ETag, content type, storage class, metadata, endpoint identity, version ID when returned
- paginated `ListObjectsV2` bucket/prefix LiveScan
- observed object count and logical bytes
- bounded largest-object ranking
- bounded immediate-prefix ranking
- bounded storage-class cardinality
- age and storage-class distributions
- progress, cancellation, and truthful partial-state handling
- no S3 cleanup/mutation UI in this slice
- no fabricated POSIX capacity/free-space/`df` semantics
- no invented cost/billing data
- no second provider registry, S3 client cache, scheduler, or JobManager

Accepted implementation head: `d0ceb64781777bd04fe51aeaff9b8d3dfa3c3343`  
Squash merge on main: `d95f6183c93932fba7f5ac10f421ce6abbe1f044`

Final exact-head CI and post-merge push CI were green across Quality, Rust 1.88 MSRV, MinIO/S3 physical acceptance, Apache WebDAV physical regression, and real PTY multiplexer acceptance. The MinIO lane explicitly executed `tests/s3_inspector_minio.rs` and proved the real object + prefix inspector path.

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

### A. First — v0.23.0 release

Recommended because the S3 Inspector is a complete user-visible feature already accepted on `main`.

Keep the release candidate minimal:

- bump Cargo version and root Cargo.lock version to `0.23.0`
- add `docs/releases/v0.23.0.md`
- synchronize README/ROADMAP release truth
- keep runtime code frozen unless a separately reviewed release blocker is found
- run exact-head standard gates and affected physical lanes
- run the existing one-build/no-rebuild release validation
- merge with pinned expected head
- tag the accepted main commit as immutable `v0.23.0`
- allow the existing tag-triggered workflow to publish tar.gz / DEB / RPM / `SHA256SUMS`
- independently verify tag target, release state, assets, checksums, packaged binary version, and v0.22.0 tag immutability

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
- `Cargo.lock` remains authoritative

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
8. create a new immutable tag on the accepted main commit
9. allow the existing tag-triggered Release workflow to publish validated artifacts
10. independently verify tag target, release state, assets, checksums, packaged binary version, and prior-tag immutability

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

> Continue development of `github.com/mrAibo/arx`. Read `docs/DEVELOPMENT_HANDOFF.md`, `ROADMAP.md`, and `ARCHITECTURE.md`; treat live GitHub state as authoritative. Current public release is v0.22.0, while current main contains the accepted S3 Object & Bucket Inspector from #264/#265. Preserve the frozen architecture authorities. Prefer a minimal v0.23.0 release before beginning the next major feature, then freeze SFTP→SFTP workspace sync as a dedicated slice. Use Hermes only for deterministic Linux-local execution.
