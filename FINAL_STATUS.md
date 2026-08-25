# ARX Current Status

This file is the concise current-status snapshot for the project. GitHub state is authoritative over any SHA or issue count recorded here.

## Release

**Current public release:** `v0.21.0`  
**Release tag target:** `3427cd085740d2c3f8a4bffbbf55b34ba3d9bb85`  
**Published:** 2026-08-25  
**Platform:** Linux only  
**Published architecture:** Linux x86_64  
**Rust MSRV:** 1.88

v0.21.0 is published and accepted for public use as the current stable 0.x release.

Published release assets:

- `arx-v0.21.0-x86_64-unknown-linux-gnu.tar.gz`
- `arx_0.21.0_amd64.deb`
- `arx-0.21.0-1.x86_64.rpm`
- `SHA256SUMS`

Release validation confirmed the accepted tag/commit, one release ELF, native package payloads and metadata, packaged binary identity (`arx 0.21.0`), third-party license output, and checksums before publication.

## Current product status

### Core

- Dual-pane Linux TUI with tabs, history, mouse, configurable effective keymap, and split panes.
- Split panes support vertical/horizontal orientation, explicit close, and keyboard ratio resize.
- Command Center and internal action registration are the canonical discovery path.
- `arx --print-keymap` exposes the resolved effective bindings.
- `arx.menu` is the supported lightweight admin extension surface; there is no embedded Lua/WASM/native plugin runtime.

### Local / SFTP

- Local and SFTP browsing.
- Transactional Local↔SFTP file copy with rollback/recovery semantics.
- SFTP bounded text preview.
- Conflict-safe SFTP text Remote Edit.
- OpenSSH-backed configuration/session behavior.
- Remote Workspace compare → Preview → Execute → Verify for Local→Local, Local→SFTP, and SFTP→Local.
- SFTP→SFTP workspace synchronization remains intentionally unsupported.

### Transfer runtime

- One persistent `TransferQueueRuntime`.
- Configurable concurrency `1..=8`.
- Truthful progress/rate/ETA where evidence exists.
- Cooperative Pause/Resume/Cancel.
- Bounded safe retry (maximum three total attempts).
- Ambiguous remote mutations and recovery-required outcomes are never blindly replayed.
- Transfer Center Active / History / All views use the same runtime authority.

### Storage visibility

- Local Storage Inspector (`Alt+U`) — read-only logical/allocated usage, drill-down, top files, partial/error/cancel truth.
- Filesystems (`Alt+D`) — read-only Linux mount/capacity/inode view.
- No cleanup/mount/quota/resize behavior is implied by these views.

### S3

- AWS S3: physically accepted supported MVP.
- MinIO: physically accepted supported MVP.
- Moto: emulated acceptance.
- Cloudflare R2 / Wasabi: unverified best-effort only.

### WebDAV

Physically accepted targets:

- Apache mod_dav — W1–W18.
- Nextcloud 34.0.2-apache — I1–I12.
- ownCloud 11.0.0 — I1–I12.

Shipped behavior includes one-file Local↔WebDAV copy, MKCOL/delete/COPY/MOVE provider semantics, exact provider-native href authority, noclobber safety, bounded preview, and Basic auth backed by keyring/environment secrets.

v0.21.0 additionally ships **one exact selected WebDAV collection → one new Local tree** recursive download with bounded `PROPFIND Depth: 1`, manifest-before-Local-mutation, cycle/duplicate/path safety, staged file downloads, attempt-root cleanup, and checked cumulative byte progress.

Not shipped yet:

- Local directory → WebDAV recursive upload
- recursive WebDAV delete
- WebDAV→WebDAV recursive/cross-target operations
- multiple recursive roots
- metadata/property mutation
- Digest/Bearer authentication without concrete interoperability need

These remain tracked under issue #13.

## Architecture status

The O → P → Q → R architecture sequence is complete.

Frozen authority model:

```text
Location          = typed identity / address / navigation
ProviderRegistry  = execution authority
CapabilitySet     = exact-location / concrete-instance capability truth
```

Do not introduce a second ProviderRegistry, TransferQueueRuntime, JobManager, EffectDispatcher, scheduler, retry authority, or secret store.

Exact provider-native identity is authoritative; display names are presentation only.

## Current backlog

At the v0.21.0 baseline, the only active product issue is **#13 — WebDAV post-MVP**.

Recommended next major feature candidate:

1. Local directory → WebDAV recursive upload after a fresh mutation/recovery contract review.

Later candidates:

2. recursive WebDAV delete
3. multiple-root recursive planning
4. WebDAV↔WebDAV / cross-provider transfer evolution
5. SFTP→SFTP workspace sync / safe cross-backend Move
6. S3 Object/Bucket Inspector and evidence-based analytics
7. additional Linux architectures / signed repositories if justified

External plugins remain a decision gate, not scheduled product work.

## Stabilization phase

The immediate post-v0.21.0 priority is stabilization:

- keep public documentation synchronized with released truth
- prefer real bug reports/regressions over speculative features
- keep runtime architecture frozen unless a demonstrated blocker requires change
- preserve exact-head CI and affected physical acceptance
- clean obsolete PR/branch/status clutter only after verifying it is superseded

## Acceptance gates

Standard code acceptance:

```bash
cargo fmt --check
cargo check --locked --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-features
cargo +1.88 check --locked --all-features
git diff --check
```

Provider or transfer semantic changes also require their affected physical lanes.

## Canonical references

- `README.md` — public product/install truth
- `ROADMAP.md` — current roadmap and prioritization
- `ARCHITECTURE.md` — architecture contracts
- `docs/DEVELOPMENT_HANDOFF.md` — continuation rules
- `docs/releases/v0.21.0.md` — release notes
- GitHub Releases / Issues / PRs — live authoritative state
