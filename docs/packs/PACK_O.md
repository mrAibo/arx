# PACK O — Typed Local Quick Actions

This file is the live implementation record for PACK O and PR #179. It is updated as each step is completed so GitHub remains the authoritative handoff source.

## Baseline

- Product baseline before PACK O: PACK N merge `eec15d91d264a40760b6772135de516d40f1b95c`
- Architecture handoff merge on `main`: `dd858bd124610d0f527965e357892a148c60d5e9`
- PACK O branch: `feat/pack-o-typed-quick-actions`
- Handoff merge brought into the branch at `6b6e8f91bb8c90bead805009af78469e03527610`
- Tracking: #9, #178, PR #179

## Scope

Deliver three built-in, typed, local-only Quick Actions through the existing Action / Availability / Effect architecture:

1. SHA-256
2. Touch file
3. Compress selected entries to tar.gz

No TUI architecture rewrite, plugin runtime, remote/cloud Quick Actions, arbitrary archive formats, or new global shortcut is part of PACK O.

## Safety contract

- local-only availability; remote/cloud providers fail closed
- filenames are never interpolated into `sh -c`
- SHA-256 is computed in Rust
- Touch uses `O_NOFOLLOW` and an opened regular-file descriptor
- tar.gz uses typed argv with `--` before user filenames
- archive finalization is staged and noclobber
- long work runs off the TUI thread through the Effect lane
- Quick Action mutation/results remain accepted even after pane navigation
- Quit requests cooperative cancellation and waits for the Quick Action result before exiting
- Rust MSRV stays 1.88

## Progress

- [x] O0 — merge canonical Roadmap/Handoff from current `main` into the PACK O branch
- [x] O1 — add typed `QuickActionService` request/outcome/failure models
- [x] O2 — implement Rust SHA-256 worker and tests
- [x] O3 — implement safe Touch worker and tests
- [x] O4 — implement typed tar.gz worker with staged noclobber finalization and tests
- [x] O5 — add `EffectLane::QuickAction`, typed Effect/Event variants and frozen prompt type
- [ ] O6 — wire `ProcessService`, Action/ActionId/Catalog, Availability and AppState lifecycle
- [ ] O7 — wire TUI dispatch, prompt submission, result presentation, refresh and safe Quit cancellation
- [ ] O8 — reconcile Cargo.lock and run local `cargo fmt`, `cargo check --locked --all-features`, Clippy/tests
- [ ] O9 — update README/ROADMAP/ARCHITECTURE and this file to shipped truth
- [ ] O10 — exact-head CI + Release validation, review final diff, Ready, merge, close #9/#178

## Current known CI state

The earlier Draft runs CI #620 / Release #91 were intentionally non-authoritative because integration was incomplete. CI #620 stopped first on rustfmt in `src/services/quick_actions.rs`; that run must not be used as final PACK O evidence.

Only exact-head green runs after O6–O9 are completed count as acceptance evidence.

## O6 execution log

### Attempt 1 — hard stop, no code commit

The first deterministic O6 runner started from exact head `de2982999a09d943584d7d746d785ab979f978b2` and correctly stopped before formatting/check/test/commit because one `src/app/mod.rs` anchor matched zero times. Hermes restored the worktree to the exact clean starting head; no partial code changes were committed or pushed.

Two defects were identified in the runner itself:

1. the `accepts_effect` comment anchor used `conflict/resolution information`, while the repository text is `conflict or recovery instructions`;
2. the replacement guard for `Action::Quit` omitted the required `||` between the RemoteEdit and QuickAction pending-effect conditions.

Both defects are in the runner specification, not in repository code. O6 remains unchecked until the corrected deterministic runner completes all gates and pushes one clean integration commit.

### Attempt 2 — hard stop, no code commit

The second deterministic runner started from exact head `642301c60632030dfd48ad7058a0dcd2d2899331`. The corrected code-only `accepts_effect` anchor matched and all deterministic replacements applied, but the generated `Action::Quit` guard still lacked the `||` token at execution time. `cargo fmt` therefore failed at Rust parsing before `cargo check`, tests, commit, or push. Hermes restored the worktree clean to the exact starting head.

No repository code defect was discovered in Attempt 2. The failure remained in the runner text itself. To remove this class of failure, Attempt 3 must not inject the entire multi-line Quit arm. It must replace only the existing one-line guard condition with a single explicit Rust expression containing `||`, and assert that exact expression exists before formatting.

## Next step

Re-run O6 from the new documentation-only exact head using a narrowly-scoped guard replacement plus a pre-format assertion for the exact `RemoteEdit || QuickAction` expression. `src/tui.rs` remains out of scope until O7.
