# ARX Roadmap

## CURRENT — Product Truth

Documentation and product positioning reflect the actual runtime. Remote Workspace is the hero workflow. SFTP F3/F4 are implemented. Keybindings match runtime truth.

## NEXT — Release Readiness

- Resolve Cargo.toml version vs public tag/release line
- Verify MSRV against CI toolchain and dependency requirements
- Harden release workflow: checksums, quality gates, smoke
- Freeze supported artifact matrix
- Release notes covering user-visible capabilities
- Install/documentation truth for source and binary
- Release candidate dry run on real artifact
- Tag and publish

## THEN — Next Public Release

Release from current main after Product Truth and Release Readiness gates pass.

## POST-RELEASE DEBT

Issues #47–#53 track deferred engineering work:

- **Reliability:** transport-only connection invalidation (#47), connection health probe (#48)
- **Cleanup:** dead RemoteEditState variants (#49), double-lock (#52)
- **Safety testing:** fault-injection suite (#50), parent policy (#53)
- **Architecture:** Job Manager integration for Remote Edit (#51)

## FUTURE

- S3/MinIO backend — implementation in progress; not released as supported until physical acceptance gates pass.
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
