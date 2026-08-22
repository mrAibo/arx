# ARX Roadmap

## CURRENT — Product Truth

ARX **v0.20.0** is the current release line. It promotes the already-merged storage,
transfer-control, and Linux packaging work built on the v0.19.0 Transfer Queue
baseline. PACK L is release-readiness/publication only; it does not introduce new
runtime behavior.

Current product truth:

- **Platform:** Linux only; published target is Linux x86_64. Native Windows support is
  not planned.
- **MSRV:** Rust 1.88.
- **SFTP:** Remote Edit, SSH Host Manager, transactional copy, and existing physical
  safety acceptance retained.
- **S3:** AWS S3 + MinIO PHYSICAL PASS / SUPPORTED MVP; Moto EMULATED PASS;
  Cloudflare R2 / Wasabi remain UNVERIFIED best-effort targets.
- **WebDAV:** Apache mod_dav PHYSICAL PASS, W1–W18; Basic auth only; Nextcloud /
  ownCloud remain UNVERIFIED.
- **Transfer Queue:** bounded FIFO runtime, truthful progress/rate/ETA where known,
  safe retry, cooperative pause/resume/cancel, and owned shutdown.
- **Transfer Center v2:** keyboard-first Active / History / All views with selected-job
  detail and controls routed to the existing TransferQueueRuntime.
- **Storage Inspector (`Alt+U`):** Linux-local, read-only `du++`-style scan with
  logical/allocated byte truth, hard-link handling, drill-down/top files, partial/error
  evidence, and JobManager cancellation.
- **Filesystems (`Alt+D`):** Linux-local, read-only `df++`-style mount/capacity/inode
  view with explicit unavailable/autofs truth and manual refresh.
- **Distribution:** GitHub Release is the single publication path; Linux x86_64 ships
  tar.gz, `.deb`, `.rpm`, and one `SHA256SUMS`, all produced from one validated ELF.

## RELEASED — v0.17.0 (2026-08-16)

Release-readiness established the Linux x86_64 publication baseline:

- Rust 1.88 MSRV gate.
- checksums, quality gates, and packaged-binary smoke.
- release notes and installation truth.

## RELEASED — v0.17.1

Historical SFTP Remote Edit reliability/hardening release over v0.17.0. PACK C merged
via PR #155 and delivered transport-only invalidation, bounded pooled-session health
checks, deterministic fault/race coverage, safer writable-parent policy, and
JobManager-integrated Remote Edit lifecycle.

Physical acceptance included E1–E12, pool-health, fault/race, TUI lifecycle, and zero
leaked `.arx-part-*` partial-upload artifacts on accepted paths.

## RELEASED — v0.18.0

The WebDAV release over the v0.17.1 reliability baseline. PACK E merged via PR #157.

Delivered:

- PROPFIND / GET / PUT / DELETE / MKCOL / COPY / MOVE provider semantics.
- HTTP Basic auth with OS-keyring / environment secret resolution.
- bounded F3 preview and one-file Local ↔ WebDAV F5 copy.
- authoritative raw-href identity and strict DAV namespace/propstat parsing.
- atomic remote no-clobber via `If-None-Match: *`.
- local staged download with real noclobber finalization.
- no blind retry of ambiguous mutations.
- Apache mod_dav W1–W18 PHYSICAL PASS.

Not claimed: Digest/Bearer auth, WebDAV F4 remote edit, recursive WebDAV
transfer/delete, cross-target WebDAV move, or Nextcloud/ownCloud physical
certification.

## RELEASED — v0.19.0 (2026-08-21)

v0.19.0 is the Transfer Queue release, tag target
`092c3013e29ba70e083634ff50df36a794056f1d`.

Delivered:

- one persistent bounded FIFO `TransferQueueRuntime`.
- default concurrency N=2, configurable in the range 1..=8.
- background Copy/Move orchestration for already-supported TransferPlanner paths.
- one JobManager lifecycle truth for queue/run/pause/cancel/retry/completion.
- truthful status-bar percentage, byte rate, and ETA only where known.
- cooperative same-task Pause/Resume with `PausePending`, same JobId, same attempt.
- bounded automatic retry, maximum 3 total attempts, only for `SafeToRetry`.
- `AmbiguousMutation` / `RecoveryRequired` outcomes never blindly replayed.
- workers/retry timers owned and joined at shutdown.

Physical acceptance covered Local queue/runtime behavior, real MinIO safe-read retry,
WebDAV ambiguous mutation evidence, and SFTP commit/rename ambiguity policy.

## RELEASED — v0.20.0

v0.20.0 consolidates four already-merged product packs into one release.

### PACK I — Storage Inspector / Filesystems

- **I1** merged via PR #162: Linux Local Storage Inspector core using `dua-core` for
  parallel traversal; logical vs allocated bytes, symlink no-follow, hard-link
  de-duplication, top-N, depth/same-filesystem policy, truthful errors/cancellation.
- **I2** merged via PR #164: one `StorageScan` JobManager lifecycle plus read-only
  drill-down TUI; no second job runtime and no destructive cleanup actions.
- **I3** merged via PR #166: Linux `/proc/self/mountinfo` + `statvfs` filesystem view;
  block/inode modes, deterministic sort/filter, explicit unavailable/autofs states,
  manual refresh only.

### PACK J — Transfer Center UX v2

Merged via PR #168 at `f95b601745a093a29cff1f05b463616ecca37f6e`.

- Ctrl+Y overlay owns input while open.
- Active / History / All filters with deterministic cursor clamping.
- compact list plus selected-job lifecycle/progress/scheduler/attempt detail.
- Pause/Resume/Cancel call the existing TransferQueueRuntime; UI does not fabricate
  JobManager transitions.
- terminal history presentation is bounded without changing JobManager retention.

### PACK K — Native Linux packages

Merged via PR #171 at `b4e14ee25a4b7be88f5c0330eaf14509c55023e7`.

- tar.gz, Debian `.deb`, and RPM `.rpm` from the **same validated release ELF**.
- no rebuild between package formats or between validate and publish.
- exact package payload manifests; unexpected files/symlinks fail closed.
- RPM build-id payload injection disabled so the package stays inside the declared
  executable/docs contract.
- Debian runtime dependencies derived from the exact ELF with `dpkg-shlibdeps`.
- `cargo-about` generated `THIRD_PARTY_LICENSES.html` included in every package.
- one `SHA256SUMS` covers all three published package artifacts.

## RELEASE PROCESS POLICY

A release candidate must keep runtime code frozen and change only release truth unless
a newly discovered blocker requires a separately reviewed fix. Before tagging:

- Cargo version and root Cargo.lock version match the intended tag.
- README, ROADMAP, and `docs/releases/vX.Y.Z.md` describe the same product truth.
- exact-head quality / Rust 1.88 MSRV / physical provider gates are green.
- Release validation builds once and validates third-party notices, tar/deb/rpm exact
  payloads, metadata, ELF identity, checksums, and artifact upload.
- tag targets the accepted merge SHA; publication reuses validated artifacts without a
  rebuild.

## PLATFORM POLICY

ARX remains intentionally **Linux-only**. Future platform effort, if useful, should
stay within Linux (for example additional architectures) rather than creating a
separate Windows product surface.

## FUTURE

- WebDAV post-MVP — Digest/Bearer auth, F4 remote edit, recursive transfers,
  cross-target move, and Nextcloud/ownCloud physical certification.
- Cross-backend Move.
- SFTP → SFTP workspace sync.
- Recursive remote delete.
- Binary remote editing.
- Plugin system (Lua/WASM).
- Additional Linux architectures and, if justified, signed package-repository
  distribution.

### S3 Inspector and Analytics (POST-MVP)

- Read-only Object Inspector and Bucket Inspector for S3.
- Usage analytics with explicit evidence source (LiveScan / StorageLens / Inventory /
  OtherProvider / Unavailable) and as-of freshness.
- S3 remains an object-storage usage model, not POSIX `df`; never fabricate filesystem
  capacity semantics for providers that cannot prove them.
