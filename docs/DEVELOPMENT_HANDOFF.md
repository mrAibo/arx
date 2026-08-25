# ARX Development Handoff

This document is the canonical continuation point for active ARX development. It is intentionally compact: a new development session should be able to recover current product truth, frozen architecture rules, and the next work sequence without reconstructing chat history.

> **Authority rule:** live GitHub state wins over this file. Re-fetch current `main`, open issues/PRs, and release state before changing code. The v0.21.0 release tag target recorded below is immutable release evidence; current `main` may advance through later docs/bugfix work.

## 1. Current release baseline

Repository: `mrAibo/arx`

- Current public release: **v0.21.0**
- v0.21.0 tag target: `3427cd085740d2c3f8a4bffbbf55b34ba3d9bb85`
- Published: 2026-08-25
- Rust MSRV: **1.88**
- Product platform: **Linux only**
- Published target: **Linux x86_64**
- Release assets: tar.gz, `.deb`, `.rpm`, and `SHA256SUMS`
- Release publication uses one validated ELF and reuses the validated artifact bundle; do not rebuild between validation and publication.

v0.20.0 remains an immutable historical release baseline. Never move/reinterpret old tags to absorb later work.

## 2. Current phase — stabilization

The immediate phase after v0.21.0 is **stabilization**, not another architecture pack.

Priorities:

1. keep public docs and live GitHub status synchronized with shipped truth
2. prioritize real user regressions/bug reports over speculative features
3. keep runtime architecture frozen unless a demonstrated correctness/safety blocker requires change
4. preserve exact-head CI and affected physical provider acceptance
5. clean obsolete GitHub PR/branch/status clutter only after verifying it is superseded

The next major feature should be chosen only after stabilization is clean.

## 3. Current product truth

Completed post-v0.20.0 work now included in v0.21.0:

- configurable conflict-safe effective keymap + `arx --print-keymap`
- mouse follow-up and provider-aware typed context menu
- tmux / GNU Screen lifecycle hardening
- split panes: vertical + horizontal, explicit close, keyboard ratio resize, section-aware same-location mouse
- deterministic transfer-pause acceptance
- WebDAV F5 target/source truth hardening
- RFC-compatible WebDAV MOVE Depth behavior
- Nextcloud 34.0.2 and ownCloud 11.0.0 physical WebDAV certification
- exact one-root WebDAV recursive download to one new Local tree
- typed local SHA-256 / Touch / Compress Quick Actions

At this baseline, **#13 WebDAV post-MVP is the only active product issue**. Always re-query GitHub before relying on that count.

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

## 5. WebDAV truth after v0.21.0

Physically accepted targets:

- Apache mod_dav — W1–W18
- Nextcloud 34.0.2-apache — I1–I12
- ownCloud 11.0.0 — I1–I12

Basic auth through keyring/environment-backed secrets is the shipped path. Digest/Bearer are not scheduled without concrete interoperability evidence.

v0.21.0 ships recursive **WebDAV → Local** for one exact selected collection:

- exact provider-native `WebDavCollectionRef` / `WebDavObjectRef` href identities
- authenticated bounded `PROPFIND Depth: 1`
- complete manifest before Local mutation
- bounded descendants/depth and direct-child/root containment
- cycle/duplicate/presentation-name/path-component fail-closed behavior
- staged noclobber file downloads
- attempt-owned root cleanup on failure/cancel
- recovery-required outcome if cleanup itself fails
- cumulative checked byte progress

Not implied by that implementation:

- Local directory → WebDAV recursive upload
- recursive WebDAV delete
- WebDAV→WebDAV recursive/cross-target copy/move
- multiple recursive roots
- metadata/property mutation
- Digest/Bearer auth

Those remain under #13 and require fresh contracts before implementation.

## 6. Recommended next feature sequence

After stabilization, the preferred next design review is:

### A. Local directory → WebDAV recursive upload

Do not mirror the download implementation mechanically. Remote mutation requires a fresh contract covering at least:

- exact destination collection identity
- source pre-scan / manifest before remote mutation where feasible
- MKCOL semantics and empty directories
- staged/noclobber file upload reuse
- symlink policy
- partial remote-tree rollback/recovery
- cancellation boundaries
- retry ambiguity after remote mutations
- cleanup failures and recovery evidence
- progress semantics
- Apache / Nextcloud / ownCloud physical acceptance through the same real product path

### B. Later candidates

- recursive WebDAV delete
- multiple-root recursive planning
- WebDAV↔WebDAV transfer under truthful target semantics
- SFTP→SFTP workspace sync
- safe cross-backend Move (`copy → verify → delete-source`, never optimistic provider-crossing rename semantics)
- S3 Object/Bucket Inspector and evidence-based usage analytics
- additional Linux architectures / signed repositories if justified

Create/freshen dedicated issues before implementing backlog items that are currently roadmap-only.

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
- S3 transfer/retry changes require MinIO physical evidence
- multiplexer terminal lifecycle changes require real PTY evidence
- fail closed when identity/capability/safety is ambiguous
- never fabricate progress, rate, ETA, capacity, or provider semantics
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
2. Fetch current `main`; do not assume the v0.21.0 release target is still current main.
3. Query open issues and PRs from GitHub.
4. Treat current public release/tag state as immutable evidence.
5. Confirm whether the request is stabilization, bugfix, or a newly approved feature slice.
6. Check affected source and tests before freezing semantics.
7. Use connected GitHub tooling for review/PR/merge/CI whenever available.
8. Use Hermes only where Linux-local execution materially helps.

Minimal continuation prompt:

> Continue development of `github.com/mrAibo/arx`. Read `docs/DEVELOPMENT_HANDOFF.md`, `ROADMAP.md`, and `ARCHITECTURE.md`; treat live GitHub state as authoritative. Current public baseline is v0.21.0, with stabilization first and #13 WebDAV post-MVP as the active product umbrella unless GitHub now says otherwise. Preserve the frozen architecture authorities and use Hermes only for deterministic Linux-local execution.
