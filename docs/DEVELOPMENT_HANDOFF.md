# ARX Development Handoff

This is the canonical continuation point for active ARX development. Live GitHub state is authoritative over this file: re-fetch current `main`, open issues/PRs, workflow state, tags, and releases before acting on recorded SHAs.

## 1. Current release baseline

Repository: `mrAibo/arx`

- Current public release: **v0.24.0**
- v0.24.0 tag target / release commit: `6d413fac5d5b493859bfadfbedbeb436b1140e0b`
- Published: 2026-08-26
- Previous immutable release: **v0.23.0** → `f66a25f3f2b4fb66832ecc50d85f9f105ebba086`
- Canonical docs may advance `main` beyond the release commit without moving the v0.24.0 tag
- Rust MSRV: **1.88**
- Product platform: **Linux only**
- Published target: **Linux x86_64**
- Release assets: tar.gz, `.deb`, `.rpm`, `SHA256SUMS`

v0.24.0 ships the accepted SFTP → SFTP Workspace Sync and retains the read-only S3 Object & Bucket Inspector from v0.23.0. Publication is complete and independently verified.

Release evidence:

- release PR #273 exact candidate: `fd798b3117bec1f782f53cbedd48bdab72eaab08`
- exact-head CI: run `33010488213` — success
- exact-head SFTP physical: run `33010488203` — success
- exact-head Nextcloud/ownCloud interoperability: run `33010488206` — success
- exact-head Release validator: run `33010488294` — success
- PR #273 squash merge / release commit: `6d413fac5d5b493859bfadfbedbeb436b1140e0b`
- post-merge CI: run `33011943055` — success
- post-merge SFTP physical: run `33011943080` — success
- final Release run: `33012256020` — validate success, publish success
- published Release: latest, non-draft, non-prerelease
- independent package hashes matched `SHA256SUMS` and published GitHub asset digests
- tarball binary reports `arx 0.24.0`; `--help` exits successfully
- v0.23.0 and v0.22.0 tag targets remained unchanged

Published package SHA-256 values:

- tar.gz: `d8463a3d78e1d19e61f7cb3383b17d5f4cf0fb4f046e989d1f1129aa18237f42`
- DEB: `697c7f6e5c69d23ee97ae4664fb60af26f140ef5005686f10db97895a692fda4`
- RPM: `9a5f6a99ed5dc4ed7db77def9f1ab7efe1feb419c1b76a674b586dcc7ece1bc2`

## 2. Current phase

The focused **v0.24.0** release package is complete: SFTP → SFTP Workspace Sync is published, the immutable tag points to the accepted release commit, and package/checksum/binary verification is complete. #271 is the release closure record.

Current sequence:

1. keep v0.24.0 and all older release tags immutable
2. keep the WebDAV #13 backlog explicit; released WebDAV functionality remains supported
3. next candidate: WebDAV → WebDAV recursive/cross-target copy only after exact source/target identity, copy semantics, confirmation and recovery truth are frozen
4. after that, consider general cross-provider Move only as copy → verify → delete-source
5. continue exact-head, physical-provider and one-build release discipline for every future slice

## 3. Shipped product truth

### Local / SFTP

- Local and SFTP browsing
- transactional copy and bounded preview
- conflict-safe SFTP text Remote Edit
- OpenSSH-backed host/session behavior

### Remote Workspace

Published v0.24.0 supports Compare → Preview → Execute → Verify for:

- Local → Local
- Local → SFTP
- SFTP → Local
- SFTP → SFTP, for both same-host different-root and cross-host roots, using bounded remote → ARX → remote streaming and the existing verification/recovery authorities.

### Transfer runtime

- one persistent bounded FIFO `TransferQueueRuntime`
- configurable concurrency `1..=8`
- truthful progress/rate/ETA where known
- Pause/Resume/Cancel
- bounded safe retry
- Transfer Center Active / History / All views

### Storage / filesystem intelligence

- Local Storage Inspector with logical/allocated usage, drill-down/top files, hard-link handling, cancellation, partial/error truth
- S3 exact object inspection plus bounded paginated bucket/prefix LiveScan
- Linux Filesystems capacity/inode view
- no fabricated POSIX capacity/free-space/inode/`df` semantics for S3
- no invented S3 billing/cost data

### S3

- AWS S3 supported product path
- MinIO physically accepted
- Moto emulated evidence
- Cloudflare R2 / Wasabi remain best-effort/unverified
- S3 Inspector is read-only and uses provider-returned facts only

### WebDAV

Physically accepted:

- Apache mod_dav
- Nextcloud 34.0.2-apache
- ownCloud 11.0.0

Shipped recursive surface:

- WebDAV → Local recursive download
- Local → WebDAV recursive upload
- safe bounded recursive delete for one selected collection
- one-job multi-root Local↔WebDAV F5 Copy

Remaining issue #13 enhancements:

- multi-root recursive WebDAV delete
- WebDAV→WebDAV recursive/cross-target copy with exact source/target identity and truthful staging/recovery semantics
- WebDAV→WebDAV move only after copy → verify → delete-source semantics are separately proven
- metadata/property mutation only for a demonstrated admin use case
- Digest/Bearer auth only if interoperability evidence requires it

### Other shipped surface

- conflict-safe effective keymap + `arx --print-keymap`
- vertical/horizontal split panes and visible-row-correct mouse behavior
- hardened tmux / GNU Screen lifecycle
- typed local Quick Actions: SHA-256, Touch, Compress-to-tar.gz plus mkdir/chmod/symlink surface
- Linux x86_64 tar.gz / DEB / RPM publication from one validated ELF
- `arx.menu` lightweight extension surface
- no embedded Lua/WASM/native plugin runtime

## 4. Frozen architecture

The O → P → Q → R architecture sequence is complete.

```text
Location          = typed identity / address / navigation
ProviderRegistry  = explicit provider execution authority
CapabilitySet     = exact-location / concrete-instance capability truth
VfsProvider       = backend provider interface
```

Provider-native identity remains authoritative. Display names never reconstruct existing remote resources.

Two resolver seams remain deliberately distinct:

- `ProviderRegistry::provider_for_location` — string-path / legacy backend authority
- `ProviderRegistry::provider_for_page_location` — typed page/native-identity authority

Do not add a second:

- `ProviderRegistry`
- `TransferQueueRuntime`
- `JobManager`
- `EffectDispatcher`
- scheduler
- retry authority
- secret store

Transfer execution remains owned by the existing planner / queue / executor seams. Mutations remain routed through typed provider/mutation authorities.

External plugins remain **no GO**. `arx.menu` is the supported lightweight extension path.

## 5. Released S3 Inspector evidence

Issue #264 / PR #265 added:

- exact provider-native object identity
- `HeadObject` facts only: size, last modified, ETag, content type, storage class, metadata, endpoint identity, version ID when returned
- paginated `ListObjectsV2` bucket/prefix LiveScan
- observed object count and logical bytes
- bounded largest-object and immediate-prefix ranking
- bounded prefix and storage-class cardinality
- age and storage-class distributions
- progress, cancellation, truthful partial-state handling
- no S3 mutation/cleanup path in this slice
- no second provider registry, S3 client cache, scheduler, or JobManager

Accepted feature head: `d0ceb64781777bd04fe51aeaff9b8d3dfa3c3343`  
Feature squash merge: `d95f6183c93932fba7f5ac10f421ce6abbe1f044`

Real MinIO acceptance explicitly exercised `tests/s3_inspector_minio.rs`.

## 6. Released feature — SFTP → SFTP workspace sync

Issue #269 / PR #270 completed the frozen contract without introducing another synchronization engine.

Accepted implementation truth:

- exact source and destination SFTP host/path identities remain authoritative
- same-host and cross-host are explicit and both stream bounded remote → ARX → remote
- no server-side copy/rename fiction and no general Move expansion
- existing workspace-sync controller, `ProviderRegistry`, Transfer Queue/Job lifecycle, retry/recovery authority, journal and verification model are reused
- SFTP directory creation and deletion use existing Registry/provider mutation seams
- frozen Preview, confirmation, stale validation, deterministic ordering, cancellation and post-execution verification are retained
- source/destination precommit failures are classified truthfully; ambiguous mutations are not blindly replayed
- permanent two-endpoint OpenSSH physical acceptance uses strict host-key checking

Evidence:

- exact feature head: `9656102bc679b71ca49513b37735bc79a3874a91`
- feature squash merge: `fe413aecfdc3bf5685849e73b396800f7f3ab7e0`
- accepted feature tree: `57463d53718fd1cdd2b838dbee5524d99a9de59c`
- feature post-merge CI: run `33002995697` — success
- feature post-merge SFTP physical: run `33002995516` — success
- public-truth baseline: `cd5f1b147ac4ca78d7fdab134d548b97a7c20a00`
- public-truth post-merge CI: run `33007674598` — success
- public-truth post-merge SFTP physical: run `33007674646` — success
- v0.24.0 release candidate: `fd798b3117bec1f782f53cbedd48bdab72eaab08`
- v0.24.0 release commit/tag target: `6d413fac5d5b493859bfadfbedbeb436b1140e0b`
- v0.24.0 post-merge CI: run `33011943055` — success
- v0.24.0 post-merge SFTP physical: run `33011943080` — success
- v0.24.0 Release run: `33012256020` — validate success, publish success
- issue #269: completed

Release status: published as focused **v0.24.0**; #271 records final release closure evidence.

## 7. Later candidates

Next after v0.24.0:

- WebDAV→WebDAV recursive copy under exact source/target semantics
- general safe cross-provider Move modeled as copy → verify → delete-source
- binary remote editing
- additional Linux architectures if demand justifies them
- signed repositories if operationally worthwhile

Native Windows support remains out of scope.

## 8. Engineering and acceptance invariants

Standard gates:

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
- provider/transfer semantic changes require affected physical lanes
- SFTP semantic changes require real SFTP evidence
- WebDAV semantic changes require Apache acceptance and, where portability is affected, Nextcloud/ownCloud interoperability
- S3 semantic changes require MinIO physical evidence
- multiplexer lifecycle changes require real PTY evidence
- fail closed on ambiguous identity/capability/safety
- never fabricate progress, rate, ETA, capacity, cost, or provider semantics
- do not change production semantics merely to satisfy timing-sensitive tests
- destructive/remote mutation paths require truthful transaction, cancellation, retry, and recovery behavior
- `Cargo.lock` remains authoritative

## 9. Collaboration model

- **ChatGPT** owns architecture, connected-source recon, diff review, GitHub PR/merge/release operations, CI interpretation, and acceptance decisions.
- **Hermes Agent** is a secondary Linux executor for deterministic shell-oriented work that connected tooling cannot perform.
- Prefer one coherent Hermes macro-task over repeated microtasks.
- Hermes must not independently redefine architecture/scope, merge PRs, close issues, move published tags, or publish releases unless authority is explicitly changed.

## 10. Release policy

For future releases:

1. freeze scope
2. reconcile fresh `main`
3. prepare one release-candidate branch
4. keep runtime frozen unless a genuine blocker needs separate review
5. require exact-head standard + affected physical gates
6. validate packages from one release ELF
7. merge with pinned expected head
8. require post-merge CI success
9. create a new immutable tag on the accepted release commit
10. publish the validated artifact bundle without rebuilding
11. independently verify tag target, Release state, assets, checksums, packaged binary version, and prior-tag immutability

Never repurpose an already-published version identity.

## 11. New-session startup checklist

Before changing code:

1. Read this file, `ROADMAP.md`, and `ARCHITECTURE.md`.
2. Fetch current `main`.
3. Query open issues and PRs.
4. Query latest Release and tag state.
5. Confirm the selected work slice and its frozen contract.
6. Inspect affected source/tests before proposing architecture changes.
7. Use connected GitHub tooling for review/PR/merge/CI whenever available.
8. Use Hermes only where Linux-local execution materially helps.

Minimal continuation prompt:

> Continue development of `github.com/mrAibo/arx`. Read `docs/DEVELOPMENT_HANDOFF.md`, `ROADMAP.md`, and `ARCHITECTURE.md`; treat live GitHub state as authoritative. Current public release is v0.24.0 at tag target `6d413fac5d5b493859bfadfbedbeb436b1140e0b`, shipping SFTP→SFTP Workspace Sync and retaining the v0.23.0 S3 Inspector plus released WebDAV recursive operations. Preserve frozen architecture authorities. The next recommended major slice is WebDAV→WebDAV recursive/cross-target copy under issue #13: freeze exact source/target identity, copy semantics, confirmation, cancellation, staged destination and recovery truth before implementation. Keep Move as a separate later copy → verify → delete-source decision. Use Hermes only for deterministic Linux-local execution.
