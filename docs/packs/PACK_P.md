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

P2a added focused TestBackend characterization and extracted these leaf render helpers into `src/tui/overlays.rs`:

- `render_session_callout`
- `render_help`
- `help_full_lines`
- `render_viewer`
- `render_bookmarks`

Exact PR head `54c1ea80a0b2c921f0399991e2e881aa600e7cd1` passed CI #647 / run `32594193558`; squash merge `2f427bd1f3d138efd5f580ac950abe9d3e921845`.

## Completed: P2b utility overlays + sizing corrections

Tracked by #189/#191/#193 and merged through PRs #190/#192/#194.

P2b extracted Infrastructure Center, Smart Tree, Command Center, and Context Menu into `src/tui/overlays.rs`. The extraction exposed a pre-existing line-count-vs-percentage sizing defect, corrected separately without hiding semantic changes inside the refactor.

Key merges:

- P2b: `c41e626fa9a2b031cd379b5885927f1b4920a360`
- utility sizing: `26d309dac02e784460542f349dc2e1f4563538cc`
- History/Tab sizing: `a9049a2c76153e00b0fa0f15c223f56657c9bad7`

## Completed: P2c history, tabs, and inline input renderers

Tracked by #195 and merged through PR #196.

P2c extracted Directory History, Tab Switcher, Rename input, and File Search rendering into `src/tui/overlays.rs` while preserving parent predicates/order and corrected line-based popup sizing.

Exact PR head `6a986f00e2e869ce18ada8f3abe9b219284fc148` passed CI #658 / run `32607071395`; squash merge `cf584b0a774f82f9fca73a27058bdacfc1cd38ae`.

## P2 boundary decision

P2 is complete at `cf584b0a774f82f9fca73a27058bdacfc1cd38ae`.

The remaining inline surfaces are intentionally not forced into another frame-only slice: Hotlist performs I/O; Which-Key intersects `KeyRouter`; command-bar rendering creates mouse hitboxes; Workspace Sync / remote delete is feature-controller-specific; Hosts/SSH/Jobs and transfer/storage/filesystem surfaces are feature-controller territory.

## Completed: P3a SSH Host Manager controller

Tracked by #197 and merged through PR #198.

P3a created `src/tui/ssh_hosts.rs` and moved the cohesive SSH Host Manager rendering, form handling, managed-host persistence helpers, bounded connection test, Ed25519 key generation, config editor launcher, and dedicated keyboard branch behind `ssh_hosts::render` / `ssh_hosts::handle_key`.

The fail-closed key-generation confirmation gate remains before list-mode mutation keys. Exact PR head `e98cf076f658fda5086aa0ffa67ddcbeefb4cd2d` passed CI #661; squash merge `f9ec98f64c4bf02de93a4645e8afddd1713196ac`.

## Completed: P3 simple feature-controller macro-batch

Tracked by #199 and merged through PR #200.

One macro-transaction extracted four simple feature boundaries with shared runtime authority preserved:

- `src/tui/bookmarks.rs`
- `src/tui/hosts.rs`
- `src/tui/jobs.rs`
- `src/tui/user_menu.rs`

Independent review caught and corrected two implementation regressions before merge: a temporary Jobs routing shadow that made the extracted controller unreachable, and an accidental internal host-id addition to the generic Hosts presentation. The final Hosts regression fixture uses distinct host id and hostname values.

The same review exposed a pre-existing product bug: the old Jobs `Delete` route was unreachable behind the overlay `continue`. That semantic defect was recorded explicitly as #201 and resolved by moving reachable Delete handling into the Jobs controller while retaining `cancel_job_product_route` and the existing `SyncUiRuntime`/TransferQueueRuntime/JobManager authority.

Final PR head `f73d19fff5d90c3458afb3347074f086049b66e2` passed CI #666 / run `32634023679`: quality, Rust 1.88 MSRV, Apache mod_dav W1–W18 physical acceptance, and MinIO transfer retry physical acceptance.

Squash merge on `main`: `e78f0f347764f7ec389908ced44e55d15d4e6359`.

## Active macro-transaction: P3 orchestration controllers

Tracked by canonical #202. Duplicate tracker creations #203/#204 were closed immediately without code changes.

This transaction extracts the remaining large P3 feature-orchestration seams while preserving the P3/P5 boundary. It is one branch/PR with three independently reviewable commits:

1. Quick Actions orchestration into `src/tui/quick_actions.rs`;
2. Remote Edit lifecycle/initiation/deferred editor into `src/tui/remote_edit.rs`;
3. Workspace Sync action orchestration and feature rendering into `src/tui/workspace.rs`.

P3 owns feature initiation, lifecycle helpers, deferred feature driving, and feature-local rendering. P5 remains authoritative for async response application: `handle_effect_response`, `EffectEvent` application, `workspace_scan_rx`, `sync_launch_rx`, `verification_rx`, and `job_rx` stay parent-owned in this transaction.

Quick Actions must not absorb generic command input, mkdir, or shell execution; only an already-frozen `QuickActionPrompt` may be completed through a narrow helper. Remote Edit must preserve the one shared Job lifecycle and exact phases. Workspace must preserve immutable-plan, confirmation, supersession, queue-boundary, and verification presentation truth.

No second EffectDispatcher, JobManager, TransferQueueRuntime, WorkspaceSyncController, scheduler, provider registry, or response loop may be introduced.

## Acceptance

Every extraction must pass locally and on exact PR head:

```bash
cargo fmt --check
cargo check --locked --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-features
cargo +1.88 check --locked --all-features
git diff --check
```

CI must retain quality, Rust 1.88 MSRV, Apache mod_dav W1–W18 physical acceptance, and MinIO transfer-queue retry physical acceptance. Release validation is required only when a slice changes release/packaging-relevant behavior.

## Scope guards

PACK P must not introduce:

- new product actions/features;
- new shortcuts;
- provider/VFS behavior changes;
- duplicate JobManager / scheduler / Effect ownership;
- Lua/WASM/native plugin runtime;
- PACK Q ProviderRegistry convergence changes;
- PACK R registration abstractions.

If an extraction requires a semantic change to compile, stop and record/review that semantic change separately instead of hiding it inside the refactor.
