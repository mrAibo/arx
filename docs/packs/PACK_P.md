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
- [x] P2 — frame/render-only extraction
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

P1 remains open because additional presentation-only seams may still move as later controller slices expose them.

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

## Completed: P2c history, tabs, and inline input renderers

Tracked by #195 and merged through PR #196.

P2c extracted Directory History, Tab Switcher, Rename input, and File Search rendering into `src/tui/overlays.rs` while preserving parent predicates/order and the corrected line-based popup sizing.

Four focused TestBackend characterization tests run on `120x24`. Exact PR head `6a986f00e2e869ce18ada8f3abe9b219284fc148` passed CI #658 / run `32607071395`; squash merge `cf584b0a774f82f9fca73a27058bdacfc1cd38ae`.

## P2 boundary decision

P2 is complete at `cf584b0a774f82f9fca73a27058bdacfc1cd38ae`.

The remaining inline surfaces are intentionally not forced into another frame-only slice: Hotlist performs I/O; Which-Key intersects `KeyRouter`; command-bar rendering creates mouse hitboxes; Workspace Sync / remote delete is feature-controller-specific; Hosts/SSH/Jobs and transfer/storage/filesystem surfaces are feature-controller territory.

## Completed: P3a SSH Host Manager controller

Tracked by #197 and merged through PR #198.

P3a created `src/tui/ssh_hosts.rs` and moved the cohesive SSH Host Manager rendering, form handling, managed-host persistence helpers, bounded connection test, Ed25519 key generation, config editor launcher, and dedicated keyboard branch behind `ssh_hosts::render` / `ssh_hosts::handle_key`.

The fail-closed key-generation confirmation gate remains before list-mode mutation keys. Parent event/render composition roots remain authoritative. Exact PR head `e98cf076f658fda5086aa0ffa67ddcbeefb4cd2d` passed CI #661: quality, Rust 1.88 MSRV, Apache mod_dav W1–W18 physical acceptance, and MinIO retry physical acceptance.

Squash merge on `main`: `f9ec98f64c4bf02de93a4645e8afddd1713196ac`.

## Active macro-transaction: P3 simple feature controllers

Tracked by #199.

To reduce mechanical ping-pong while preserving reviewability, this transaction uses one branch/PR with multiple independently green commits:

1. Bookmarks + generic Hosts controllers;
2. Jobs controller;
3. User Menu controller.

The intended feature APIs are narrow `render(...)` and `handle_key(...) -> bool` seams. Existing `PaneLoader`, `TransferQueueRuntime`, and `EffectDispatcher` instances are passed through; no duplicate runtime authority is introduced. Parent `event_loop` and parent `render()` remain composition roots and retain relative feature order.

Mechanical import/rustfmt/clippy/test-fixture corrections may be repaired locally without stopping. The transaction must stop on a semantic contradiction, unexpected production dependency, changed branch head, or required scope expansion.

Explicitly out of this macro-transaction: SSH Host Manager semantics, Hotlist I/O, Which-Key/KeyRouter, command-bar hitboxes, Workspace Sync/remote delete, Remote Edit, Quick Actions, Transfer Center implementation, Storage Inspector, Filesystems, provider/VFS behavior, keybindings, and all new registry/plugin/scheduler abstractions.

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

CI must retain quality, Rust 1.88 MSRV, Apache mod_dav W1–W18 physical acceptance, and MinIO transfer-queue retry physical acceptance. Release validation is required only when a slice changes release/packaging-relevant behavior; behavior-preserving internal decomposition slices do not require a release workflow run.

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
