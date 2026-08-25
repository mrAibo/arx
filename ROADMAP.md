# ARX Roadmap

GitHub state is authoritative over this document. Re-fetch current `main`, issues, PRs, and release state before acting on any SHA or backlog item recorded here.

## CURRENT — v0.21.0 product truth

**Current public release:** `v0.21.0`  
**Release tag target:** `3427cd085740d2c3f8a4bffbbf55b34ba3d9bb85`  
**Published:** 2026-08-25  
**Platform:** Linux only; published target Linux x86_64  
**MSRV:** Rust 1.88

v0.21.0 is the current stable 0.x release line. It keeps the established one-build/no-rebuild release pipeline and adds the first recursive WebDAV capability on top of the v0.20.0 storage, transfer-control, and native-packaging baseline.

Current product truth:

- **Local / SFTP:** browsing, transactional copy, bounded preview, SFTP conflict-safe text Remote Edit, OpenSSH-backed host/session behavior.
- **Remote Workspace:** compare → Preview → Execute → Verify for Local→Local, Local→SFTP, and SFTP→Local. SFTP→SFTP workspace sync remains unsupported.
- **Transfer Queue:** one persistent bounded FIFO runtime, configurable concurrency `1..=8`, truthful progress/rate/ETA where known, cooperative Pause/Resume/Cancel, and bounded safe retry.
- **Transfer Center v2:** Active / History / All views and controls routed to the existing TransferQueueRuntime.
- **Storage Inspector (`Alt+U`):** Linux-local, read-only `du++`-style logical/allocated usage, drill-down/top-files, hard-link handling, partial/error/cancel truth.
- **Filesystems (`Alt+D`):** Linux-local, read-only `df++`-style capacity/inode view with explicit unavailable/autofs truth.
- **Effective keymap:** one conflict-safe effective runtime map with user overrides and `arx --print-keymap` discovery.
- **Mouse:** visible-row-correct clicks, active-pane wheel, Shift+Click ranges, drag-selection safety, provider-aware typed context menu.
- **Split panes:** vertical + horizontal orientation, explicit close, keyboard ratio resize `20..=80` step 5, section-aware same-location mouse behavior. No independent split Location and no nested pane tree.
- **tmux / GNU Screen:** typed discovery and terminal mode release/reacquire around interactive attach.
- **Typed local Quick Actions:** SHA-256, Touch, Compress-to-tar.gz, with the pre-existing mkdir/chmod/symlink surface retained. No shell-string interpolation for the new actions.
- **S3:** AWS S3 + MinIO physically accepted supported MVPs; Moto emulated; Cloudflare R2 / Wasabi remain unverified best-effort targets.
- **WebDAV:** Apache mod_dav, Nextcloud 34.0.2-apache, and ownCloud 11.0.0 physically accepted through the supported Basic-auth path.
- **WebDAV recursive download:** one exact selected WebDAV collection → one new Local tree, using provider-native href identities, bounded authenticated `PROPFIND Depth: 1`, manifest-before-Local-mutation, noclobber staging, cleanup/recovery truth, and checked cumulative byte progress.
- **Distribution:** GitHub Release is the publication path; Linux x86_64 ships tar.gz, `.deb`, `.rpm`, and one `SHA256SUMS`, all produced from one validated ELF.
- **Extension surface:** `arx.menu` is the supported lightweight admin extension mechanism. There is no embedded Lua/WASM/native plugin runtime.

## STABILIZATION — v0.21.x

The immediate phase after v0.21.0 is stabilization, not another architecture sequence.

Priorities:

1. Keep README / ROADMAP / development handoff / current status synchronized with released product truth.
2. Treat real user bug reports and regressions as higher priority than speculative feature work.
3. Keep runtime architecture frozen unless a demonstrated correctness/safety blocker requires a reviewed change.
4. Preserve exact-head CI and physical provider acceptance when affected code changes.
5. Clean obsolete GitHub PR/branch/status clutter after verifying that no unique unfinished work remains.

The first stabilization pass is complete: the public truth documents were refreshed for v0.21.0 and obsolete historical draft PRs were closed without deleting their history.

### Development cadence after stabilization

ARX should now progress in **large, coherent user-visible vertical slices**, not long chains of micro-tasks or stacked micro-PRs.

- Freeze the complete behavioral/safety contract before implementation.
- Implement the selected feature as one coherent branch / macro-batch.
- Include its unit/contract tests, physical acceptance changes, and necessary documentation in the same slice.
- Collect related review findings and correct them coherently on the same branch instead of bouncing between tiny implementation prompts.
- Accept only one exact final head SHA after independent full-diff review and required CI/physical evidence.
- Prefer completing one useful capability and moving toward the next release over speculative architecture work.

The selected next major slice is **#253 — Local directory → WebDAV recursive upload**.

Patch releases in the v0.21.x line should normally contain bug fixes, compatibility corrections, documentation truth, or release/distribution fixes — not large new feature surfaces. #253 is therefore the leading candidate for the next feature release rather than a v0.21.x patch.

## COMPLETED FOUNDATION

The architecture sequence **O → P → Q → R is complete**.

- **PACK O:** typed local Quick Actions — complete.
- **PACK P:** TUI decomposition — complete.
- **PACK Q:** VFS convergence — complete.
- **PACK R:** internal feature/command registration — complete.

Frozen architecture truths:

```text
Location          = typed identity / address / navigation
ProviderRegistry  = execution authority
CapabilitySet     = exact-location / concrete-instance capability truth
VfsProvider       = backend provider interface
```

Do not add a second ProviderRegistry, TransferQueueRuntime, JobManager, EffectDispatcher, scheduler, retry authority, or secret store.

Provider-native identity remains authoritative. Presentation/display names never reconstruct remote addresses.

### External plugins

There is **no GO** for an external Lua/WASM/`.so` plugin runtime. Re-evaluate only if real user/ecosystem demand appears and a truthful enforcement/security model can be defined. `arx.menu` remains the supported lightweight extension path.

## ACTIVE GITHUB BACKLOG

**#13 — WebDAV post-MVP** remains the product umbrella.  
**#253 — WebDAV recursive upload** is the selected active implementation slice under that umbrella.

Already shipped under #13:

- core WebDAV provider semantics
- Apache mod_dav W1–W18 physical acceptance
- Nextcloud 34.0.2 and ownCloud 11.0.0 I1–I12 physical certification
- WebDAV F5 source/selection truth hardening
- RFC-compatible MOVE Depth behavior
- exact recursive WebDAV collection download to Local

Selected now:

- [ ] **#253 Local directory → WebDAV recursive upload** — one selected Local directory to one new remote tree through the existing F5/planner/queue/executor authority, with full Local pre-scan, bounded tree limits, symlink/special-file rejection, destination-root noclobber, owned-root cleanup/recovery truth, whole-attempt byte progress, and Apache/Nextcloud/ownCloud physical acceptance.

Remaining #13 roadmap after #253:

- [ ] **Recursive WebDAV delete** — only with explicit bounded traversal/identity and recovery semantics.
- [ ] **WebDAV → WebDAV recursive/cross-target copy or move** — only with a truthful execution model; never pretend server-side MOVE spans unrelated targets.
- [ ] **Multiple-root recursive selection** — only after one stable planning/progress contract exists.
- [ ] **Metadata/property mutation** — optional, only if a real admin use case justifies it.
- [ ] **Digest/Bearer auth** — only if future interoperability evidence demonstrates that the shipped Basic-auth/app-password path is insufficient.

The remaining #13 items are enhancements, not regressions or release blockers.

## RECOMMENDED NEXT FEATURE SEQUENCE

This is prioritization guidance, not a promise that each item must ship in the named version.

### Candidate v0.22.0 — WebDAV write-side recursion

**Chosen implementation slice: #253.**

- One selected Local directory → one new WebDAV directory tree.
- Existing one-file upload remains supported.
- No merging into an existing remote root.
- No multiple roots, recursive delete, WebDAV↔WebDAV recursion, cross-backend Move, or speculative auth expansion in this slice.

Acceptance must include Apache mod_dav, Nextcloud, and ownCloud through the same real F5 → preparation → planner → Transfer Queue → executor product path, with explicit partial-remote-tree cleanup/recovery semantics and no blind retry of ambiguous mutations.

### Candidate v0.23.0 — Remote tree operations

Potential scope after upload is stable:

- recursive WebDAV delete
- multiple-root recursive planning if one coherent contract is proven

Do not combine these merely to fill a release; each mutation surface must have its own safety proof.

### Candidate v0.24.0 — Cross-provider transfer evolution

Potential work:

- WebDAV↔WebDAV transfer under explicit target truth
- SFTP→SFTP workspace sync
- safe cross-backend Move based on copy → verify → delete-source, never optimistic rename semantics across providers
- broader recursive remote operations only where transaction/recovery truth exists

### Candidate v0.25.0 — Storage intelligence

S3 post-MVP direction:

- read-only Object Inspector
- read-only Bucket Inspector
- usage analytics with explicit evidence source and freshness (`LiveScan`, `StorageLens`, `Inventory`, `OtherProvider`, `Unavailable`)

S3 must remain an object-storage model. Never fabricate POSIX `df`/filesystem-capacity semantics when the provider cannot prove them.

## OTHER PRODUCT BACKLOG

Items not currently represented by active GitHub issues should be promoted to dedicated issues before implementation:

- binary remote editing
- additional Linux architectures, especially ARM64 if user demand justifies it
- signed package-repository distribution if operationally worthwhile
- broader cross-backend Move
- SFTP→SFTP workspace sync
- provider-specific read-only inspection/analytics where evidence is truthful

Native Windows support remains out of scope. Windows SSH clients may interoperate with an ARX process running on Linux; that does not change the Linux-only product policy.

## RELEASED — v0.21.0 (2026-08-25)

Release target: `3427cd085740d2c3f8a4bffbbf55b34ba3d9bb85`.

Highlights:

- exact one-root WebDAV recursive download to a new Local tree
- Nextcloud 34.0.2 and ownCloud 11.0.0 physical WebDAV certification
- WebDAV F5 target/source truth and MOVE interoperability fixes
- horizontal/resizable split panes and section-aware mouse behavior
- conflict-safe configurable effective keymap + `arx --print-keymap`
- tmux/GNU Screen lifecycle hardening
- mouse follow-up and typed context menu
- deterministic transfer-pause acceptance
- typed local SHA-256 / Touch / Compress Quick Actions

Published artifacts:

- `arx-v0.21.0-x86_64-unknown-linux-gnu.tar.gz`
- `arx_0.21.0_amd64.deb`
- `arx-0.21.0-1.x86_64.rpm`
- `SHA256SUMS`

The tag-triggered Release workflow validated version/tag truth, format, Clippy, full tests, Rust 1.88, one release build, third-party licenses, TAR/DEB/RPM payload and metadata, packaged-binary identity, checksums, and publication from the validated artifact bundle.

## RELEASE HISTORY

### v0.20.0

Storage/operations release:

- Local Storage Inspector / Filesystems
- Transfer Center v2
- native Linux tar.gz / DEB / RPM publication contract
- typed local Quick Actions and lean extension/runtime cleanup landed in the post-v0.20 development range and are represented accurately in v0.21.0 notes

### v0.19.0

Transfer Queue release:

- one persistent bounded FIFO `TransferQueueRuntime`
- configurable concurrency
- truthful progress/rate/ETA
- cooperative Pause/Resume/Cancel
- bounded safe retry and recovery/ambiguity classification

### v0.18.0

WebDAV MVP:

- PROPFIND / GET / PUT / DELETE / MKCOL / COPY / MOVE
- Basic auth through keyring/environment secret resolution
- bounded F3 preview and one-file Local↔WebDAV F5 copy
- raw-href authority and noclobber safety
- Apache mod_dav W1–W18 physical acceptance

### v0.17.x

Linux publication and SFTP Remote Edit reliability baseline.

Historical implementation detail remains available through Git history, closed issues/PRs, and per-release notes; the roadmap intentionally emphasizes current truth over chronology.

## RELEASE PROCESS POLICY

A release candidate should keep runtime code frozen and change only release truth unless a newly discovered correctness/safety blocker requires a separately reviewed fix.

Before tagging:

- Cargo version and root Cargo.lock version match the intended tag.
- README, ROADMAP, handoff, and `docs/releases/vX.Y.Z.md` describe the same product truth.
- exact-head quality / Rust 1.88 MSRV / affected physical provider gates are green.
- Release validation builds once and validates notices, tar/deb/rpm exact payloads, package metadata, ELF identity, checksums, and artifact upload.
- the tag targets the accepted merge SHA.
- publication reuses validated artifacts without rebuilding them.

## DEVELOPMENT POLICY

- Prefer user-visible vertical slices over new architecture packs.
- Implement one selected feature as a coherent macro-batch instead of a chain of micro-tasks.
- Collect related findings and fix them coherently on the same implementation branch.
- Keep product/runtime/documentation/physical acceptance changes for one capability in the same slice when they are logically required.
- Use exact-head CI and physical evidence as acceptance gates.
- Do not weaken production semantics to satisfy timing-sensitive tests.
- Move toward a usable release once the frozen slice is complete; do not extend scope merely because adjacent ideas are available.
- Use Hermes only for deterministic Linux-local execution that connected tooling cannot perform; architecture, review, PR/merge, and release authority remain outside Hermes.
