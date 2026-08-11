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

- S3/MinIO and WebDAV production backends
- Cross-backend Move
- SFTP → SFTP workspace sync
- Recursive remote delete
- Binary remote editing
- Plugin system (Lua/WASM)
- Broader platform support (aarch64, macOS, Windows)
