# ARX Roadmap

## CURRENT — Product Truth

ARX **v0.18.0 is the released baseline**. PACK G Transfer Queue has since merged into
`main` via **PR #159** (merge SHA
`b5890aec0b424fa5fe9f1b67e84f42a7b62d73de`) and issue **#15 is CLOSED**.
The next public release is **v0.19.0**, which promotes the merged Transfer Queue as a
released product capability without adding new provider combinations or platform work.

Current backend truth:

- **SFTP:** released baseline with Remote Edit, SSH Host Manager, transactional copy,
  and physical safety acceptance retained.
- **S3:** AWS S3 + MinIO PHYSICAL PASS / SUPPORTED MVP; Moto EMULATED PASS;
  Cloudflare R2 / Wasabi remain UNVERIFIED best-effort targets.
- **WebDAV:** Apache mod_dav PHYSICAL PASS, W1–W18; Basic auth only; Nextcloud /
  ownCloud remain UNVERIFIED.
- **Transfer Queue:** merged and physically accepted, including real MinIO safe-read
  retry coverage.
- **MSRV:** Rust 1.88.
- **Platform:** Linux only. Native Windows support is not planned; issue #17 is closed
  as `not planned`.

## RELEASED — v0.17.0 (2026-08-16)

The v0.17.0 Release Readiness gates completed and the release was published:

- Cargo version / tag / release line resolved.
- MSRV verified at Rust 1.88.
- Release workflow hardened with checksums, quality gates, and packaged-binary smoke.
- Supported artifact matrix frozen to Linux x86_64 binary + source.
- Release notes and installation truth published.

## RELEASED — v0.17.1

v0.17.1 is the historical reliability/hardening release over v0.17.0. It did **not**
ship WebDAV as a release feature.

PACK C landed in `main` via **PR #155** (merge SHA `6e4b5fbb`) and delivered:

- **Reliability:** transport-only connection invalidation (#47), bounded pooled-session
  health probe (#48).
- **Cleanup:** dead RemoteEditState variants (#49), double-lock removed (#52).
- **Safety testing:** deterministic fault-injection suite (#50), writable non-sticky
  parent policy (#53).
- **Architecture:** Remote Edit lifecycle integrated with Job Manager (#51), typed
  phases/outcomes, lane-isolated failure handling.

Physical SFTP Remote Edit acceptance included:

- **E1–E12** safety matrix — PASS.
- **fault/race** — PASS.
- **pool-health** (real sshd, reuse / stale-reconnect / no-replay) — PASS.
- **TUI lifecycle** (shared JobManager + job_events) — PASS.
- artifact audit: **0** partial-upload (`.arx-part-*`) fragments leaked.

## RELEASED — v0.18.0

v0.18.0 is the WebDAV release over the v0.17.1 reliability baseline.

PACK E merged via **PR #157** and delivered:

- PROPFIND / GET / PUT / DELETE / MKCOL / COPY / MOVE.
- HTTP Basic auth with OS-keyring / environment secret resolution; no plaintext
  password in config.
- bounded F3 preview.
- one-file Local ↔ WebDAV F5 path.
- authoritative raw-href identity and strict DAV namespace / propstat parsing.
- atomic remote no-clobber via `If-None-Match: *`.
- local staged download with real noclobber finalization.
- no blind retry of ambiguous mutations.
- dedicated `webdav-physical` CI gate.
- Apache mod_dav physical acceptance **W1–W18 PASS**.

Not claimed by the WebDAV MVP:

- Digest/Bearer authentication.
- WebDAV F4 remote edit.
- recursive WebDAV transfer/delete.
- cross-target WebDAV move.
- Nextcloud / ownCloud physical certification.

## PACK G — Transfer Queue (MERGED)

PACK G merged via **PR #159** at merge SHA
`b5890aec0b424fa5fe9f1b67e84f42a7b62d73de`; issue **#15 is CLOSED**.

Delivered product surface:

- one persistent bounded FIFO `TransferQueueRuntime`.
- default concurrency **N=2**, configurable through `[transfer] concurrency` in the
  range **1..=8**.
- background Copy/Move orchestration for already-supported TransferPlanner paths.
- one JobManager lifecycle truth for queue/run/pause/cancel/retry/completion.
- truthful status-bar percentage, byte rate, and ETA where known; unknown total stays
  unknown instead of becoming zero.
- **Ctrl+Y Transfer Center** and Jobs Pause/Resume/Cancel integration.
- cooperative same-task Pause/Resume with `PausePending`, same JobId and same attempt.
- bounded automatic retry, maximum **3 total attempts**, only for `SafeToRetry`.
- `AmbiguousMutation` / `RecoveryRequired` outcomes are never blindly replayed.
- worker and retry-timer ownership with joined shutdown.

Physical acceptance:

- **P1–P7, P11–P12:** real Local filesystem queue/runtime behavior PASS.
- **P8:** real MinIO `GetObject` body-fault path PASS; first read fails after staged
  cleanup, queue enters `RetryWaiting`, second GET succeeds byte-exact, GET count=2.
- **P9:** WebDAV ambiguous PUT provider evidence + queue one-shot policy PASS.
- **P10:** SFTP commit/rename ambiguity provider evidence + queue one-shot policy PASS.
- post-merge CI on `b5890aec...` passed `quality`, `msrv`, `webdav-physical`, and
  `transfer-queue-s3-retry-physical`.

## THEN — Next Public Release = v0.19.0

v0.19.0 is the **Transfer Queue release** over the v0.18.0 WebDAV baseline. It is a
minor release because it adds a substantial user-facing transfer-orchestration
capability.

Release Readiness requires:

- version / lockfile / README / roadmap / release-notes truth only;
- no production feature edits;
- exact-SHA `quality` success;
- exact-SHA Rust 1.88 `msrv` success;
- exact-SHA `webdav-physical` W1–W18 success;
- exact-SHA `transfer-queue-s3-retry-physical` success;
- release-workflow package / checksum / packaged-binary smoke success.

## PLATFORM POLICY

ARX remains intentionally **Linux-only**. The current published artifact is Linux
x86_64. Native Windows work is not planned, and issue #17 was closed accordingly.
Future platform effort, if any, should stay within Linux (for example additional Linux
architectures) rather than expanding to a separate Windows product surface.

## FUTURE

- WebDAV post-MVP — Digest/Bearer auth, F4 remote edit, recursive transfers,
  cross-target move, and Nextcloud/ownCloud physical certification.
- Cross-backend Move.
- SFTP → SFTP workspace sync.
- Recursive remote delete.
- Binary remote editing.
- Plugin system (Lua/WASM).
- Additional Linux packaging/architectures where useful.

### Storage Inspector / Disk Usage

#### A. Filesystem Overview ("df++")

Future fields:

- device / filesystem
- mount
- filesystem type
- total
- used
- available
- usage %
- inode total / used / free where supported
- read-only state
- provider / evidence

#### B. Usage Analyzer ("du++")

Future capabilities:

- recursive subtree size
- logical / apparent bytes
- allocated bytes where supported
- file count
- directory count
- top-N largest
- depth control
- same-filesystem policy
- symlink policy
- hard-link handling
- permission errors
- progress
- cancellation
- partial / inconclusive result

Execution rule: potentially expensive recursive scans are background Jobs. No blocking
full-tree scan from pane render.

Provider rule:

- **Local:** native filesystem information.
- **SFTP / remote:** expose filesystem stats only when the transport/provider can prove
  them; otherwise `Unsupported`.
- **S3:** object-storage usage model, not POSIX `df`. Use object bytes/count and explicit
  evidence source.

#### S3 Inspector and Analytics (POST-MVP)

- Read-only Object Inspector (Ctrl+I) and Bucket Inspector for S3.
- Usage Analytics with explicit evidence source (LiveScan / StorageLens / Inventory /
  OtherProvider / Unavailable) and as-of freshness.
- Behavioral inspiration only: AWS S3 Console, duf, dust, dua, gdu. No source copied;
  no dependencies added.
