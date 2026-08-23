# PACK P — Behavior-preserving TUI Decomposition

Canonical tracker: #183. Parent architecture roadmap: #180.

## Baseline

- PACK P start `main`: `a512578681f5c391c1aeb1f48881b4460dc40f62`
- PACK O is complete; #9/#178 closed and PR #179 merged.
- Rust MSRV: 1.88.
- Product behavior, provider semantics, keybindings, Action/Effect/Job contracts, and runtime authority are frozen unless a separately reviewed issue explicitly changes them.

## Goal

Decompose `src/tui.rs` incrementally so rendering, feature orchestration, input routing, and async response handling have explicit testable module boundaries. This pack is a refactor, not a feature pack.

## Sequence

- [x] P0 — characterization baseline for the seams that move first
- [ ] P1 — pure presentation-model extraction into `src/tui/` submodules
- [ ] P2 — frame/render-only extraction
- [ ] P3 — feature-controller extraction in independently green slices
- [ ] P4 — keyboard/mouse routing extraction
- [ ] P5 — Effect/Job response handling extraction
- [ ] P6 — thin runtime/event-loop composition root
- [ ] P7 — docs/final exact-head acceptance and PACK P closure

## Completed: P0 + P1a

The first code transaction was deliberately narrow and is complete.

PR #186 accepted exact head `43f70e8eb535c0eab7a658b7368a3970a131540a` with CI #644 / run `32589046943` fully green: quality, Rust 1.88 MSRV, Apache mod_dav W1–W18 physical acceptance, and MinIO safe-read retry physical acceptance.

Squash merge on `main`: `32f649816adb3e04b0d9f415ac352601a9b699a4`.

P0 added semantic characterization for the workspace ribbon while existing session-callout, footer/command-bar, Quick Action / Remote Edit safety, and S3 identity/selection regressions remained green.

P1a created `src/tui/presentation.rs` and moved only the workspace ribbon and session-callout presentation model. Frame rendering, routing, Action dispatch, Effect/Job handling, provider execution, and feature-controller ownership remained in `src/tui.rs`.

P1 remains open because additional presentation-only seams may still move as later rendering slices expose them.

## Completed: P2a leaf overlays

Tracked by #187 and merged through PR #188.

P2a added focused TestBackend characterization and extracted only these leaf render helpers into `src/tui/overlays.rs`:

- `render_session_callout`
- `render_help`
- `help_full_lines`
- `render_viewer`
- `render_bookmarks`

Exact PR head `54c1ea80a0b2c921f0399991e2e881aa600e7cd1` passed CI #647 / run `32594193558`: quality, Rust 1.88 MSRV, Apache mod_dav W1–W18 physical acceptance, and MinIO safe-read retry physical acceptance.

Squash merge on `main`: `2f427bd1f3d138efd5f580ac950abe9d3e921845`.

The parent `render()` composition root, pane rendering/state, command-bar hitboxes, Which-Key, sync-preview rendering, Hosts/SSH orchestration, event routing, Action dispatch, Effect/Job handling, and provider behavior remained in `src/tui.rs`.

## Completed: P2b utility overlays

Tracked by #189 and merged through PR #190.

P2b added focused TestBackend characterization and extracted these four state-driven utility overlays into the existing `src/tui/overlays.rs` module:

- Infrastructure Center
- Smart Tree
- Command Center
- Context Menu

Exact PR head `8ce28a3959d8b2f1e896ee5b9a8fac1fd8789ef7` passed CI #650 / run `32597743305`: quality, Rust 1.88 MSRV, Apache mod_dav W1–W18 physical acceptance, and MinIO safe-read retry physical acceptance.

Squash merge on `main`: `c41e626fa9a2b031cd379b5885927f1b4920a360`.

The extraction exposed a pre-existing line-count-vs-percentage popup sizing defect. That semantic fix was deliberately split from P2b:

- #191 / PR #192 corrected the four extracted utility overlays and made `centered_rect_lines` a generic exact/clamped line-height helper while preserving the delete-confirmation minimum at its call site. Exact head `88b53b23ac30b2d21ffb4c69a0ef072922fb3535`; CI #653 green; merge `26d309dac02e784460542f349dc2e1f4563538cc`.
- #193 / PR #194 applied the same already-reviewed line-height semantics to the still-inline Directory History and Tab Switcher call sites only. Exact head `d47f67d7c2805623e9454efb1fb35eed81077dca`; CI #655 green; merge `a9049a2c76153e00b0fa0f15c223f56657c9bad7`.

Those fixes changed popup geometry only; Action/Effect/Job/provider/VFS/keybinding ownership remained unchanged.

## Active transaction: P2c history, tabs, and inline input renderers

Tracked by #195.

Extract only these currently-inline pure render surfaces from `render()` into the existing `src/tui/overlays.rs` module:

- Directory History
- Tab Switcher
- Rename input bar
- File Search bar

The parent `render()` must retain the same show/hide predicates and relative overlay ordering. Directory History and Tab Switcher must keep the line-based sizing already corrected in #193/#194.

Add focused TestBackend characterization on a normal terminal (target `120x24`) after callable seams exist, covering representative history numbering/path rendering, left/right tab rendering and cursor marker, rename pattern text, and file-search query/match count.

Explicitly keep out of P2c: Hotlist (render still performs `AppState::load_hotlist()` I/O), command bar / `CommandHitbox`, Which-Key / `KeyRouter` / Action Catalog logic, Hosts/SSH/Jobs, transfer/storage/filesystem surfaces, sync-preview rendering, pane rendering, event routing, Action dispatch, Effect/Job handling, provider/VFS behavior, and keybindings.

Use focused semantic/render characterization rather than whole-screen golden snapshots.

## Acceptance

Every extraction must pass locally and on exact PR head:

```bash
cargo fmt --check
cargo check --locked --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo test --locked --all-features
cargo +1.88 check --locked --all-features
git diff --check
```

CI must retain quality, Rust 1.88 MSRV, Apache mod_dav W1–W18 physical acceptance, and MinIO transfer-queue retry physical acceptance. Substantial extraction PRs also require Release validation before merge.

## Scope guards

PACK P must not introduce:

- new product actions/features;
- new shortcuts;
- provider/VFS behavior changes;
- duplicate JobManager / scheduler / Effect ownership;
- Lua/WASM/native plugin runtime;
- PACK Q ProviderRegistry convergence changes;
- PACK R registration abstractions.

If an extraction requires a semantic change to compile, stop and split/review that semantic change separately instead of hiding it inside the refactor.
