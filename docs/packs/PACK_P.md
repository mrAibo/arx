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

- [ ] P0 — characterization baseline for the seams that move first
- [ ] P1 — pure presentation-model extraction into `src/tui/` submodules
- [ ] P2 — frame/render-only extraction
- [ ] P3 — feature-controller extraction in independently green slices
- [ ] P4 — keyboard/mouse routing extraction
- [ ] P5 — Effect/Job response handling extraction
- [ ] P6 — thin runtime/event-loop composition root
- [ ] P7 — docs/final exact-head acceptance and PACK P closure

## First transaction: P0 + P1a

The first code PR is deliberately narrow.

### P0 characterization

Add semantic tests before moving the first presentation helpers. Pin representative behavior for:

- workspace ribbon phase/text in commander, differences, preview/running/verifying and terminal verification states;
- provider identity used by the ribbon, including provider-native S3/WebDAV display truth where applicable;
- session callout text and its embed/suppress rule;
- existing footer/command-bar tests remain unchanged and green;
- existing Quick Action / Remote Edit safe-quit tests remain unchanged and green;
- existing S3 fail-closed selection/identity regressions remain unchanged and green.

Avoid enormous full-screen golden snapshots in this first slice. Prefer direct semantic assertions against the existing output-model helpers so later moves can prove behavior identity without coupling tests to terminal padding.

### P1a extraction

Retain `src/tui.rs` as the module/composition root and create a child module under `src/tui/`. Move only presentation-model helpers with no backend/process/job mutation authority.

Initial candidates are the workspace ribbon and session-callout presentation helpers. Do not move the event loop, Action dispatch, Effect handling, Job handling, provider execution, key routing, or frame rendering in P1a.

The extraction is allowed to use `pub(super)` only where the parent `tui` module needs the helper. Do not create a public library API.

## Acceptance

Every extraction must pass locally and on exact PR head:

```bash
cargo fmt --check
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
