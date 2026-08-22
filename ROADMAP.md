# ARX Roadmap

## CURRENT — Product Truth

ARX **v0.20.0** is the current release line. It promotes the already-merged storage,
transfer-control, and Linux packaging work built on the v0.19.0 Transfer Queue
baseline. PACK L was release-readiness/publication only; it did not introduce new
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
- **Typed local Quick Actions:** SHA-256, Touch file, and Compress to tar.gz are built-in
  Action/Catalog entries discovered through Command Center. They are local-only, use a
  dedicated correlated Effect lane, and never interpolate filenames into `sh -c`.
- **SSH/X11 environment:** ARX inherits the process/session environment and never
  synthesizes `DISPLAY`; X11 forwarding must be established by the SSH client/session.
- **User extension surface:** `arx.menu` is the supported lightweight mechanism; the
  unwired embedded Lua prototype/runtime has been removed and WASM is not product scope.
- **Distribution:** GitHub Release is the single publication path; Linux x86_64 ships
  tar.gz, `.deb`, `.rpm`, and one `SHA256SUMS`, all produced from one validated ELF.

Backlog truth after PACK O:

- **tmux/screen:** tmux discovery + attach are shipped; screen discovery and attach
  lifecycle hardening remain in #7.
- **Mouse:** right-click and drag-selection are shipped; pane-wheel scrolling,
  Shift+Click range selection, and provider-aware context availability remain in #10.
- **Split panes:** the vertical split/focus model is shipped; horizontal mode, resize,
  and explicit close semantics remain in #16.
- **WebDAV:** the supported Apache mod_dav MVP is shipped; #13 tracks only post-MVP
  auth/interoperability/recursive-operation work.

## ACTIVE DEVELOPMENT SEQUENCE — PACK P → Q → R

The canonical continuation document is [`docs/DEVELOPMENT_HANDOFF.md`](docs/DEVELOPMENT_HANDOFF.md).
The architecture sequence is tracked by umbrella issue **#180**.

The approved order after PACK O is strict:

1. **PACK P — TUI decomposition.** Behavior-preserving decomposition of the >10k-line
   `src/tui.rs` composition bottleneck: characterization tests, rendering extraction,
   feature controllers, keyboard/mouse routing, Effect/Job response handling, then a
   thin runtime/event loop. No product features and no public plugin API.
2. **PACK Q — VFS convergence.** Finish the phased ProviderRegistry migration by
   removing duplicate execution authority. Preserve `Location` as typed identity/address
   where useful; make `ProviderRegistry` the execution authority and `CapabilitySet`
   the capability truth.
3. **PACK R — internal feature/command registration.** Introduce only the smallest
   internal registration layer proven necessary after P/Q. Migrate Quick Actions,
   Storage Inspector, and SSH Host Manager as proof consumers. Do not freeze a broad
   public `FeatureModule` plugin trait prematurely.
4. **External plugin decision gate.** No Lua/WASM/`.so` runtime is scheduled. Evaluate
   external plugins only after PACK R and only if real demand exists. Manifest
   permissions without real OS/runtime enforcement are not a security boundary.

GitHub state and exact SHAs are authoritative over stale chat summaries or cached
reviews. A new development session should read the handoff document, this roadmap,
issue #180, and the current PACK P planning state before changing code.

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

## PACK M — BACKLOG RECONCILIATION

PACK M was tracked by #174 and changed repository truth only; it added no runtime
behavior.

Reconciled against the v0.20.0 tree:

- #8 Cloud Storage backends — **closed completed**; S3 and WebDAV supported MVPs ship.
- #41 Embedded Terminal through Action/Command Center — **closed completed**; PR #43
  delivered the requested shared Action Catalog / Command Center path.
- #14 Lua plugin issue — **closed duplicate** of canonical plugin decision #11.
- #5, #7, #9, #10, #11, #13, and #16 were rewritten so they described only work that
  remained rather than features already present in v0.20.0.

The reconciliation also found a README overclaim: at that point compress/touch/SHA-256
were not present as typed built-in Quick Actions in the Action Catalog. PACK M therefore
kept documentation partial until the implementation could ship; PACK O now closes that
gap.

## PACK N — LEAN RUNTIME HARDENING

PACK N was tracked by #176 and merged through PR #177 at
`eec15d91d264a40760b6772135de516d40f1b95c`. It intentionally removed unsupported
runtime surface rather than adding a new subsystem.

Delivered:

- #5 X11 hardening: removed the startup heuristic that guessed
  `DISPLAY=localhost:0.0` whenever `SSH_CLIENT` existed. ARX now leaves DISPLAY and
  forwarding ownership entirely with the established session environment.
- #11 plugin decision, path A: removed the unwired `src/plugins` prototype, removed
  `pub mod plugins`, removed `mlua`, and pruned the orphaned Lua runtime subtree from
  `Cargo.lock` without dependency-version/checksum churn.
- `arx.menu` remains the supported lightweight admin-defined command extension path.
- no WASM runtime, general plugin API, terminal-brand detection, or new feature surface
  was introduced.

#5, #11, and #176 are complete under this lean-policy decision.

## PACK O — TYPED LOCAL QUICK ACTIONS

PACK O is tracked by #178 and PR #179 and completes the remaining scope of #9.

Delivered:

- **Compute SHA-256:** local focused/selected regular files are hashed in Rust with
  `sha2`; results preserve exact filename/digest association and hashing runs off the
  TUI thread.
- **Touch file:** local child-name prompt with traversal/absolute-name rejection,
  `O_NOFOLLOW`, exact opened regular-file verification, `futimens`, and a truthful
  pre-mutation cancellation boundary.
- **Compress to tar.gz:** focused/selected local entries, typed system-`tar` argv with
  `--` before filenames, `kill_on_drop(true)` cancellation, same-directory staging,
  and `persist_noclobber` finalization.
- all three actions live in the shared Action / ActionId / Action Catalog / Availability
  model and are discoverable through Command Center without new global shortcuts.
- remote/cloud providers fail closed; no local shell semantics are guessed for SFTP,
  S3, WebDAV, or Archive locations.
- `EffectLane::QuickAction` owns correlated async lifecycle, safe Quit cancellation,
  navigation-independent terminal result acceptance, and mutation-origin refresh.
- control characters in filenames/path/tool errors are escaped before presentation while
  printable Unicode remains intact.
- Cargo.lock gained only the already-resolved direct `sha2 0.10.9` root dependency;
  no package/version/checksum churn was introduced.

The implementation passed local fmt/check/clippy/full-test/Rust-1.88 gates and a
physical system-`tar` test before final exact-head CI/Release acceptance.

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
separate Windows product surface. Windows SSH clients may still be interoperability
subjects when they connect to an ARX process running on Linux; that does not change the
platform policy. ARX consumes the environment established by that session and does not
invent X11 forwarding state.

## FUTURE PRODUCT BACKLOG

Architecture packs P/Q/R take precedence after PACK O because they reduce the current
composition and dispatch bottlenecks. The product backlog remains active and should not
be lost during those refactors.

Near-term product follow-ups after the architecture sequence:

- #7 tmux/screen follow-up: screen discovery and real-terminal attach/detach lifecycle.
- #10 mouse follow-up: pane wheel, Shift+Click range selection, context availability.
- #16 split-pane follow-up: horizontal mode, resize, explicit close semantics.

Provider/transfer follow-ups:

- #13 WebDAV post-MVP — Digest/Bearer evaluation, physical Nextcloud/ownCloud
  certification, recursive transfers/delete under an explicit safe contract, and
  cross-target operations only with a truthful execution model.
- Cross-backend Move.
- SFTP → SFTP workspace sync.
- Recursive remote delete.
- Binary remote editing.
- Additional Linux architectures and, if justified, signed package-repository
  distribution.

### S3 Inspector and Analytics (POST-MVP)

- Read-only Object Inspector and Bucket Inspector for S3.
- Usage analytics with explicit evidence source (LiveScan / StorageLens / Inventory /
  OtherProvider / Unavailable) and as-of freshness.
- S3 remains an object-storage usage model, not POSIX `df`; never fabricate filesystem
  capacity semantics for providers that cannot prove them.
