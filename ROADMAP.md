# ARX Roadmap

## CURRENT — Product Truth

ARX **v0.17.1 is the released reliability baseline**. PACK C hardening is shipped, and PACK E WebDAV has since merged into `main` via **PR #157** (merge SHA `c0cd992661b013c73f1872ce2b541cc12ea0f9d7`). The next public release is **v0.18.0**, which promotes the merged WebDAV Basic-auth MVP as a supported capability.

WebDAV release truth:

- **Apache mod_dav:** PHYSICAL PASS, W1–W18.
- **Auth:** Basic only for the MVP; Digest/Bearer are DEFERRED.
- **Nextcloud / ownCloud:** UNVERIFIED; no physical-certification claim.
- **F3:** bounded preview supported.
- **F5:** one-file Local ↔ WebDAV transfer supported.
- **F7 / F8:** MKCOL and non-recursive delete supported.
- **F4 remote edit:** unsupported.
- **Cross-target WebDAV move:** unsupported.
- Authoritative raw href identity, no-clobber transfer semantics, and no blind retry of ambiguous mutations are part of the accepted WebDAV safety contract.

The S3 backend remains physically accepted against real AWS S3 and MinIO. SFTP Remote Edit and the SSH Host Manager remain part of the released baseline. MSRV stays **Rust 1.88**.

## RELEASED — v0.17.0 (2026-08-16)

The v0.17.0 Release Readiness gates completed and the release was published:

- Cargo version / tag / release line resolved.
- MSRV verified at Rust 1.88.
- Release workflow hardened with checksums, quality gates, and packaged-binary smoke.
- Supported artifact matrix frozen to Linux x86_64 binary + source.
- Release notes and installation truth published.

## RELEASED — v0.17.1

v0.17.1 is the historical reliability/hardening release over v0.17.0. It did **not** ship WebDAV as a release feature.

PACK C landed in `main` via **PR #155** (merge SHA `6e4b5fbb`) and delivered:

- **Reliability:** transport-only connection invalidation (#47), bounded pooled-session health probe (#48).
- **Cleanup:** dead RemoteEditState variants (#49), double-lock removed (#52).
- **Safety testing:** deterministic fault-injection suite (#50), writable non-sticky parent policy (#53).
- **Architecture:** Remote Edit lifecycle integrated with Job Manager (#51), typed phases/outcomes, lane-isolated failure handling.

Physical SFTP Remote Edit acceptance included:

- **E1–E12** safety matrix — PASS.
- **fault/race** — PASS.
- **pool-health** (real sshd, reuse / stale-reconnect / no-replay) — PASS.
- **TUI lifecycle** (shared JobManager + job_events) — PASS.
- artifact audit: **0** partial-upload (`.arx-part-*`) fragments leaked.

## PACK E — WebDAV MVP (MERGED)

PACK E merged via **PR #157** at merge SHA `c0cd992661b013c73f1872ce2b541cc12ea0f9d7`.

Delivered MVP surface:

- PROPFIND / GET / PUT / DELETE / MKCOL / COPY / MOVE.
- HTTP Basic auth with OS-keyring / environment secret resolution; no plaintext password in config.
- bounded F3 preview.
- one-file Local ↔ WebDAV F5 path.
- authoritative raw-href identity and strict DAV namespace / propstat parsing.
- atomic remote no-clobber via `If-None-Match: *`.
- local staged download with real noclobber finalization.
- automatic retry disabled for ambiguous mutations.
- dedicated `webdav-physical` CI gate.
- Apache mod_dav physical acceptance **W1–W18 PASS**.

Not claimed by PACK E:

- Digest/Bearer authentication.
- WebDAV F4 remote edit.
- recursive WebDAV transfer/delete.
- cross-target WebDAV move.
- Nextcloud / ownCloud physical certification.

## THEN — Next Public Release = v0.18.0

v0.18.0 is the WebDAV release pack over the v0.17.1 reliability baseline. It is a **minor release** because WebDAV is a new supported backend capability. Release Readiness requires the standard quality/MSRV/build/package gates plus exact-SHA `webdav-physical` W1–W18 acceptance. No new feature development belongs in this release pack.

## FUTURE

- S3 object-storage backend — **RELEASED as SUPPORTED MVP.** Real AWS S3 (account `715844024414`) and MinIO passed physical acceptance (20/20 tests, immutable SHA `b5f0ee6`). Moto emulated PASS. Cloudflare R2 / Wasabi remain UNVERIFIED (best-effort only, not claimed supported).
- WebDAV post-MVP — Digest/Bearer auth, F4 remote edit, recursive transfers, cross-target move, and Nextcloud/ownCloud physical certification remain future work.
- Transfer Queue / richer transfer orchestration.
- Cross-backend Move.
- SFTP → SFTP workspace sync.
- Recursive remote delete.
- Binary remote editing.
- Plugin system (Lua/WASM).
- Broader platform support (aarch64, macOS, Windows).

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

Execution rule: potentially expensive recursive scans are background Jobs. No blocking full-tree scan from pane render.

Provider rule:

- **Local:** native filesystem information.
- **SFTP / remote:** expose filesystem stats only when the transport/provider can prove them; otherwise `Unsupported`.
- **S3:** object-storage usage model, not POSIX `df`. Use object bytes/count and explicit evidence source.

#### S3 Inspector and Analytics (POST-MVP)

- Read-only Object Inspector (Ctrl+I) and Bucket Inspector for S3.
- Usage Analytics with explicit evidence source (LiveScan / StorageLens / Inventory / OtherProvider / Unavailable) and as-of freshness.
- Behavioral inspiration only: AWS S3 Console, duf, dust, dua, gdu. No source copied; no dependencies added.
