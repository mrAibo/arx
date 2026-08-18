# ARX Roadmap

## CURRENT — Product Truth

ARX **v0.17.0 released 2026-08-16** (SHA `32a7e78`). Documentation and product positioning reflect the actual runtime. Remote Workspace is the hero workflow. SFTP F3/F4 and the SSH Host Manager (F12) are implemented. The S3 backend is physically accepted against real AWS S3 (`715844024414`) and MinIO. Keybindings match runtime truth.

## RELEASED — v0.17.0 (2026-08-16)

The Release Readiness gates below were completed and **v0.17.0 was published**:

- Cargo.toml version / tag / release line resolved (v0.17.0)
- MSRV verified at Rust 1.88 against CI toolchain
- Release workflow hardened: checksums, quality gates, smoke
- Supported artifact matrix frozen (Linux x86_64 binary + source)
- Release notes and install/doc truth published
- Tagged and published

## THEN — Next Public Release

Release from current main after Product Truth and Release Readiness gates pass.

## PACK C — reliability hardening (MERGED)

PACK C post-v0.17.0 hardening landed in `main` via **PR #155** (merged 2026-08-18, merge SHA `6e4b5fbb`). Issues **#47–#53 are CLOSED**.

Delivered:

- **Reliability:** transport-only connection invalidation (#47), bounded pooled-session health probe (#48)
- **Cleanup:** dead RemoteEditState variants (#49), double-lock removed (#52)
- **Safety testing:** deterministic fault-injection suite (#50), writable non-sticky parent policy (#53)
- **Architecture:** Remote Edit lifecycle integrated with Job Manager (#51), typed phases/outcomes, lane-isolated failure handling

Physical SFTP Remote Edit acceptance (real OpenSSH fixture, `ARX_SFTP_SMOKE_HOST`):

- **E1–E12** safety matrix — PASS
- **fault/race** — PASS
- **pool-health** (real sshd, reuse / stale-reconnect / no-replay) — PASS
- **TUI lifecycle** (shared JobManager + job_events) — PASS
- artifact audit: **0** partial-upload (`.arx-part-*`) fragments leaked

## THEN — Next Public Release = v0.17.1

Patch release from current `main` (post-PACK-C reliability hardening). Release Readiness gates apply; MSRV stays 1.88. No new major features, no WebDAV/Windows/plugin/S3-scope/router changes.

## FUTURE

- S3 object-storage backend — **RELEASED as SUPPORTED MVP.** Real AWS S3 (account `715844024414`) and MinIO both passed physical acceptance (20/20 tests, immutable SHA `b5f0ee6`). Moto emulated PASS. Cloudflare R2 / Wasabi remain UNVERIFIED (best-effort only, not claimed supported).
- Cross-backend Move
- SFTP → SFTP workspace sync
- Recursive remote delete
- Binary remote editing
- Plugin system (Lua/WASM)
- Broader platform support (aarch64, macOS, Windows)

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
