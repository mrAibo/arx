# PACK O — Typed Local Quick Actions

This file is the live implementation record for PACK O and PR #179. It is updated as each step is completed so GitHub remains the authoritative handoff source.

## Baseline

- Product baseline before PACK O: PACK N merge `eec15d91d264a40760b6772135de516d40f1b95c`
- Architecture handoff merge on `main`: `dd858bd124610d0f527965e357892a148c60d5e9`
- PACK O branch: `feat/pack-o-typed-quick-actions`
- Handoff merge brought into the branch at `6b6e8f91bb8c90bead805009af78469e03527610`
- Tracking: #9, #178, PR #179
- Runner policy: read PR #179 immediately before execution and pin that exact head SHA in the deterministic script; do not store a self-referential “current HEAD” in this file.

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
- Touch cancellation is honored only before the open/create mutation boundary; once open/create succeeds, ARX finishes and reports the real terminal outcome instead of claiming a clean cancellation after a possible mutation
- tar.gz uses typed argv with `--` before user filenames
- tar subprocess cancellation uses `kill_on_drop(true)` plus cancellation-aware `tokio::select!`
- archive finalization is staged and noclobber
- long work runs off the TUI thread through the Effect lane
- Quick Action results remain accepted even after pane navigation
- Touch/Compress terminal results trigger a refresh for any pane still at the frozen origin, including failure paths that may have crossed a physical mutation boundary before a late error
- all filename/path/tool-error presentation escapes control characters while preserving printable Unicode
- Quit requests cooperative cancellation and waits for the Quick Action terminal result before allowing exit
- Rust MSRV stays 1.88

## Progress

- [x] O0 — merge canonical Roadmap/Handoff from current `main` into the PACK O branch
- [x] O1 — add typed `QuickActionService` request/outcome/failure models
- [x] O2 — implement Rust SHA-256 worker and tests
- [x] O3 — implement safe Touch worker and tests
- [x] O4 — implement typed tar.gz worker with staged noclobber finalization and tests
- [x] O5 — add `EffectLane::QuickAction`, typed Effect/Event variants and frozen prompt type
- [ ] O6+O7 — atomic compile-complete integration closure: ProcessService + Action/Catalog + Availability + AppState + TUI dispatch/prompt/result/refresh/safe Quit + service cancellation corrections
- [ ] O8 — reconcile Cargo.lock and run full local `cargo fmt`, `cargo check --locked --all-features`, Clippy/tests and Rust 1.88 gate
- [ ] O9 — update README/ROADMAP/ARCHITECTURE and this file to shipped truth
- [ ] O10 — exact-head CI + Release validation, review final diff, Ready, merge, close #9/#178

### Why O6 and O7 are now atomic

The original plan tried to make O6 a green non-TUI checkpoint and reserve `src/tui.rs` for O7. That boundary is structurally invalid on this branch: O5 already introduced both `Effect::QuickAction` and `EffectEvent::QuickActionFinished`. `ProcessService` must consume the Effect and the exhaustive TUI `EffectEvent` match must consume the Event before the crate can compile. Therefore there is no meaningful compile-complete state between those two consumers.

PACK O does **not** turn this into a TUI refactor. The only change is the transaction boundary: core wiring and the minimal existing-TUI wiring land in one integration commit. PACK P remains the separate behavior-preserving TUI decomposition pack after PACK O merges.

## Current known CI state

The earlier Draft runs CI #620 / Release #91 were intentionally non-authoritative because integration was incomplete. CI #620 stopped first on rustfmt in `src/services/quick_actions.rs`; that run must not be used as final PACK O evidence.

Only exact-head green runs after O6+O7–O9 are completed count as acceptance evidence.

## O6 execution log

### Attempt 1 — hard stop, no code commit

The first deterministic O6 runner started from exact head `de2982999a09d943584d7d746d785ab979f978b2` and correctly stopped before formatting/check/test/commit because one `src/app/mod.rs` anchor matched zero times. Hermes restored the worktree to the exact clean starting head; no partial code changes were committed or pushed.

Two defects were identified in the runner itself:

1. the `accepts_effect` comment anchor used `conflict/resolution information`, while the repository text is `conflict or recovery instructions`;
2. the replacement guard for `Action::Quit` omitted the required `||` between the RemoteEdit and QuickAction pending-effect conditions.

Both defects were in the runner specification, not repository code.

### Attempt 2 — hard stop, no code commit

The second deterministic runner started from exact head `642301c60632030dfd48ad7058a0dcd2d2899331`. The corrected code-only `accepts_effect` anchor matched and all deterministic replacements applied, but the generated `Action::Quit` guard still lacked the `||` token at execution time. `cargo fmt` therefore failed at Rust parsing before `cargo check`, tests, commit, or push. Hermes restored the worktree clean to the exact starting head.

No repository code defect was discovered in Attempt 2. The failure remained in the runner text itself.

### Attempt 3 — hard stop, no code commit

The third deterministic runner started from exact head `bcbe5447676c9715631f377a2344f6c2c8323400`. All anchors matched, the generated `RemoteEdit || QuickAction` Quit guard was proven before formatting, and `cargo fmt` plus `diff --check` succeeded. The run then stopped on the file-scope contract because the local Hermes worktree did not have `src/app/quick_action_prompt.rs` materialized even though `src/app/mod.rs` references that module.

GitHub repository truth was checked independently after the stop: `src/app/quick_action_prompt.rs` is already a tracked file in PR #179 with blob `c900d19a484f95ac7e98bb94da590e22ffd98eb9`. Attempt 3 therefore exposed a local worktree materialization mismatch, not a missing repository artifact. No O6 code was committed or pushed.

### Attempt 4 — hard stop, no code commit; architecture boundary corrected

Attempt 4 started from exact head `57a203c66fdbce574355639914b29b7b9f6ac6be`. The materialization preflight proved `src/app/quick_action_prompt.rs` is tracked at the expected blob and restored it without changing repository state. All O6 anchors then matched, the generated Quit guard was valid, `cargo fmt`, `git diff --check`, the code-scope guard and the narrow Cargo.lock reconciliation all passed.

`cargo check --all-features` then correctly failed because the exhaustive `apply_effect_event` match in `src/tui.rs` did not yet cover `EffectEvent::QuickActionFinished`. This is not a new implementation bug: it proves the former O6/O7 split itself was unsatisfiable. O6 added the producer-side `ProcessService` consumer for `Effect::QuickAction`, while the Event consumer was explicitly deferred to O7 and `src/tui.rs` was forbidden. Hermes restored the tree clean and made no commit/push.

### Atomic integration import scan — hard stop, no code commit

The first atomic O6+O7 runner was pre-scanned from exact head `1ea627354089e51e2b62c762cd8a96285c2f6baf` before mutation. Every source anchor in sections 1–6 and TUI sections 8–14 matched, including the commander-mapping protection. Only the two additive `src/tui.rs` import-block anchors were stale because the import lists had been refactored earlier.

GitHub independently confirmed the exact current import blocks. The correction is purely additive and behavior-preserving: add `QuickActionPrompt` to the existing `arx::app` import list and add `QuickActionFailureKind`, `QuickActionKind`, `QuickActionOutcome`, and `QuickActionRequest` to the existing `arx::services` import list. No implementation, architecture, scope, or lifecycle rule changes. Hermes stopped and restored the exact clean starting tree; no commit/push occurred.

### Atomic integration full-test gate — hard stop, no code commit

After the two import anchors were corrected, the atomic O6+O7 patch reached the full locked all-feature test suite. Formatting, unlocked and locked all-feature `cargo check`, Cargo.lock reconciliation, focused PACK O tests, the physical system-tar test and 921 tests all passed. Exactly one pre-existing test failed: `app::tests::quit_waits_for_remote_edit_outcome`.

The failure is caused directly by the intentional PACK O AppState lifecycle change. The old RemoteEdit-only Quit guard emitted `Remote edit in progress — wait for a safe outcome`, while PACK O now uses one guard for both `EffectLane::RemoteEdit` and `EffectLane::QuickAction` with the message `Operation in progress — wait for a safe cancellation outcome`. The existing test still asserted that the message contains `Remote edit`, so its expectation became stale when the message contract was generalized. GitHub independently confirmed that exact assertion in `src/app/mod.rs`.

This is not an implementation regression and no behavior should be reverted. The minimal completion is to update that one existing assertion in the already-authorized `src/app/mod.rs` file so it verifies the generalized contract (`safe cancellation outcome`, matching the new QuickAction companion test). Hermes correctly stopped rather than editing a test outside the supplied deterministic patch. No commit/push occurred.

## Pre-integration audit before the next code run

The next code run must close the whole existing type/lifecycle graph in one patch. The audit found these mandatory points:

1. `ProcessService::execute_with_registry_cancellable` handles `Effect::QuickAction` with the real cancellation flag.
2. ActionId/Action/ALL_ACTIONS/ACTION_CATALOG contain SHA-256, Touch and Compress; shared Availability is local-only and target-aware.
3. AppState exports/stores the frozen QuickAction prompt, accepts QuickAction mutation results after navigation, and blocks Quit while RemoteEdit or QuickAction is pending.
4. `dispatch_ui_action` rejects a second QuickAction, freezes the Local directory + current selected/focused names, and never introduces a new global shortcut.
5. Command-input Escape clears both mkdir and QuickAction pending prompts; generic `:` clears stale typed prompt ownership before becoming a shell command prompt.
6. Touch/Compress prompt submit dispatches a typed `Effect::QuickAction`; SHA dispatches directly.
7. `request_quit` cancels both RemoteEdit and QuickAction lanes and leaves AppState blocked until terminal response.
8. `apply_effect_event` handles every `QuickActionFinished` outcome with control-safe presentation; SHA opens viewer lines, mutation actions return concise status.
9. `handle_effect_response` refreshes any pane still at the frozen origin for **all** Touch/Compress terminal results, including failures that may have crossed a mutation boundary; SHA never refreshes.
10. `QuickActionService::touch_local` must not return `Cancelled` after `open(...create...)` may already have mutated the directory. Cancellation is checked before that boundary only.
11. `QuickActionService::compress_tar_gz_local` uses a `kill_on_drop(true)` tar command and `tokio::select!` against `CancellationFlag::cancelled()` so quit cannot orphan tar.
12. The existing commander hitbox helpers `action_id_to_action` / `action_to_id` stay unchanged: Quick Actions have no new keybinding and Command Center already executes typed `CommandTarget::Action` directly.
13. Cargo.lock may only add the existing direct `sha2` root dependency; no version/checksum/package churn.
14. Tests must cover local-only availability, Command Center discovery, mutation-result acceptance after navigation, Quit blocking, control-character escaping and the Touch pre-mutation cancellation boundary.

## Atomic patch scope (audited)

The O6+O7 implementation commit is allowed to modify only these existing tracked paths:

- `Cargo.lock`
- `docs/packs/PACK_O.md`
- `src/process/mod.rs`
- `src/services/quick_actions.rs`
- `src/app/actions.rs`
- `src/app/availability.rs`
- `src/app/mod.rs`
- `src/app/command_center.rs`
- `src/tui.rs`

`src/app/quick_action_prompt.rs` is pre-existing O5 content and must be materialized/verified if the local checkout omits it, but must remain byte-identical and unstaged in the O6+O7 commit.

The patch must not touch keymaps, commander hitbox helpers, provider/VFS contracts, jobs, transfer runtime, rendering architecture, module layout, or any PACK P refactor surface.

## Audit checkpoint

The complete graph audit is finished. The remaining known blocker is a single stale pre-existing test assertion in `src/app/mod.rs`; the implementation graph itself reached the full test suite with all other tests green. The next repository mutation should keep the atomic implementation unchanged and add only that one expectation update before rerunning the complete gates.

## Next step

Re-run the same atomic O6+O7 integration runner with the already-approved corrected TUI import anchors and one additional deterministic replacement in `src/app/mod.rs`: update `quit_waits_for_remote_edit_outcome` from asserting `contains("Remote edit")` to asserting `contains("safe cancellation outcome")`. Keep every other implementation line, scope guard, Cargo.lock rule and acceptance gate unchanged, and pin the exact PR #179 head immediately before execution.