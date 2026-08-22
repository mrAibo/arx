# ARX Development Handoff

This document is the canonical continuation point for active ARX development. It is
intentionally written so a new development session can recover the current state,
working rules, and next sequence from GitHub without reconstructing chat history.

> **Authority rule:** current GitHub state and exact SHAs win over any stale chat,
> cached review, or old local checkout. Re-check `main`, the active PR head, and CI
> before making changes.

## 1. Current baseline — 2026-08-22

Repository: `mrAibo/arx`

- Current published release: **v0.20.0**
- v0.20.0 tag target: `3db9cee78d056e4e568e9d9a7f08fb0f579ea707`
- Current `main`: `eec15d91d264a40760b6772135de516d40f1b95c`
- `main` includes PACK M repository-truth reconciliation and PACK N lean-runtime
  hardening.
- Rust MSRV: **1.88**
- Product platform: **Linux only**; published artifact target is Linux x86_64.

Important recent merges:

- PACK M / PR #175 → `d8abad76623b9fb22190de15249acbfb6fa12673`
- PACK N / PR #177 → `eec15d91d264a40760b6772135de516d40f1b95c`

PACK N deliberately removed unsupported runtime surface:

- ARX no longer guesses or synthesizes `DISPLAY`.
- the unwired `src/plugins` Lua prototype is gone.
- `pub mod plugins` is gone.
- `mlua` and its orphaned lockfile subtree are gone.
- `arx.menu` remains the supported lightweight admin-defined command extension path.
- there is currently **no public plugin API** and no Lua/WASM runtime in product scope.

Any review claiming that current `main` still contains `src/plugins` or exports
`pub mod plugins` is stale.

## 2. Active work — PACK O

Canonical tracking:

- Issue #9 — Quick Actions completion
- Umbrella implementation issue #178 — PACK O
- Draft PR #179 — `feat: add typed local Quick Actions`
- Branch: `feat/pack-o-typed-quick-actions`
- Snapshot PR head at handoff creation:
  `935c7b833bfd693d8bc9e7685c99de91e14af2be`

PACK O goal: finish three built-in, typed, local-only Quick Actions without adding a
plugin framework or rewriting the TUI architecture inside the feature PR.

### O1 — SHA-256

- typed `Action` / `ActionId` / Action Catalog entry
- local-only
- focused/selected regular files only
- Rust SHA-256 using pinned/already-locked `sha2 0.10.9`
- no shell interpolation
- hashing off the TUI thread
- preserve exact filename → digest association

### O2 — Touch file

- typed local action
- explicit child-name prompt
- reject traversal/absolute names
- no shell command construction
- operate on exact regular file with `O_NOFOLLOW`
- create missing regular file or update timestamps via the opened file
- do not follow symlinks

### O3 — Compress to tar.gz

- explicit tar.gz semantics
- local focused/selected entries
- system `tar` only through typed argv, never `sh -c`
- each filename is a separate argv element
- `--` protects dash-prefixed names
- stage in destination directory
- finalization must be noclobber; never overwrite an existing archive
- clear typed failure for missing/failing `tar`

### PACK O architecture already present on the Draft branch

The Draft branch already contains the lower-level architecture:

- `QuickActionService` with typed request/outcome/failure models
- Rust SHA-256 implementation
- safe Touch implementation
- typed tar.gz implementation
- dedicated `EffectLane::QuickAction`
- typed `Effect::QuickAction` / `EffectEvent::QuickActionFinished`
- typed frozen prompt state
- direct `sha2 = 0.10.9` dependency declaration

The remaining work is integration into existing composition points:

- ProcessService routing
- `ActionId` / `Action` / `ALL_ACTIONS` / `ACTION_CATALOG`
- shared availability
- AppState pending prompt and mutation-result acceptance
- TUI prompt/dispatch/result presentation
- safe cancellation/quit lifecycle
- root Cargo.lock reconciliation
- tests/docs

### Current CI note

CI run #620 / Release dry-run #91 on the snapshot Draft head are **not acceptance
evidence**. The branch was intentionally still incomplete. The quality job stopped on
`cargo fmt --check` in `src/services/quick_actions.rs`; the remaining integration had
not yet been wired. Do not treat those red runs as a regression of `main`.

After integration, only exact-head green runs on the final #179 head count as PACK O
acceptance evidence.

## 3. Approved architecture sequence after PACK O

Tracked by umbrella issue **#180**.

The approved order is strict:

```text
PACK O — typed Quick Actions
        ↓
PACK P — TUI decomposition
        ↓
PACK Q — VFS convergence
        ↓
PACK R — internal feature/command registration
        ↓
Decision gate: are external plugins actually justified?
```

Do not merge PACK P/Q/R concerns into #179.

## 4. PACK P — TUI decomposition

### Why

`src/tui.rs` is now a >10k-line composition bottleneck. Lower layers are already
substantially separated (`Action`/availability, Effects, Services, Jobs,
ProviderRegistry), but new features still funnel through one file containing event
loop, rendering, keyboard, mouse, action dispatch, overlays, and feature lifecycles.

### Scope

Behavior-preserving refactor only. No product feature and no public plugin API.

Preferred extraction order:

1. freeze/extend characterization and regression tests
2. extract pure rendering
3. extract feature controllers
   - workspace
   - transfer
   - remote edit
   - SSH hosts
   - storage/filesystems
   - Quick Actions
4. extract keyboard and mouse routing
5. extract Effect/Job response handling
6. leave a thin runtime/event loop as the composition root

The exact final folder tree is not a contract. Prefer incremental extractions with
small, reviewable behavior-preserving diffs over a big-bang rewrite.

### Success criteria

- no behavior regression
- same Action/Effect/Job safety boundaries
- TUI file(s) no longer serve as the universal feature integration funnel
- each extraction independently passes the standard acceptance gates

## 5. PACK Q — VFS convergence

`src/vfs/mod.rs` still explicitly describes a phased ProviderRegistry migration and
coexists with legacy `Location` dispatch paths.

Do **not** delete `Location` merely for abstraction purity. The target model is:

```text
Location          = typed identity / address
ProviderRegistry  = execution authority
CapabilitySet     = capability truth
```

PACK Q should inventory and remove duplicate execution authority where call sites still
perform provider behavior through direct `match Location` logic in parallel with the
registry.

Goals:

- one truthful provider execution path
- exact provider-native identities remain preserved
- capabilities remain fail-closed
- no fake POSIX semantics for S3/WebDAV
- cancellation/error taxonomy remains provider-aware
- no premature public provider-plugin ABI

## 6. PACK R — internal feature/command registration

Only after P and Q are stable, add the **smallest** internal registration layer needed
to stop built-in features from requiring bespoke composition wiring.

Possible internal concepts:

- `ActionRegistry`
- `CommandRegistry`
- registered feature controllers

Do not prematurely freeze a universal public `FeatureModule` trait. A large trait with
commands, availability, execution, rendering, jobs, and events would only replace one
God object with another abstraction bottleneck.

Proof consumers:

1. Quick Actions
2. Storage Inspector
3. SSH Host Manager

PACK R is successful when those built-ins can use the same registration mechanism and
new built-in functionality no longer needs edits across the old TUI funnel merely to
be discoverable and dispatched.

## 7. External plugins — explicitly deferred

There is **no GO** for an external plugin runtime at this stage.

Do not reintroduce Lua, WASM, or Rust `.so` plugins during O/P/Q/R.

After PACK R, external plugins may be evaluated only if there is real user/ecosystem
demand. If pursued, the preferred direction is read-only first and either a genuinely
sandboxed out-of-process protocol or capability-constrained WASI-style execution.

Important security rule:

> A manifest permission such as `network = false` or `filesystem_write = false` is
> documentation, not enforcement, when an arbitrary native plugin runs as the same OS
> user. JSON over stdin/stdout gives crash/ABI isolation, but not a security sandbox.

Therefore core-owned authority must remain intact for mutations. Provider plugins and
mutation-capable third-party plugins are late-stage concerns, not part of the current
roadmap.

## 8. Engineering and safety invariants

These rules apply across all packs unless a separately reviewed design explicitly
changes them:

- exact SHA pinning for acceptance evidence
- exact-head CI is authoritative
- Rust MSRV 1.88
- `cargo fmt --check`
- Clippy all targets/features with warnings denied
- full test suite
- physical Apache WebDAV W1–W18 acceptance
- physical MinIO transfer-queue retry acceptance
- fail closed when identity/capability/safety is ambiguous
- no fake progress, total, rate, ETA, capacity, or provider semantics
- one JobManager/runtime source of truth; no duplicate schedulers
- provider-native identity must not be reconstructed from presentation strings
- destructive or remote mutation paths require truthful transaction/cancellation
  semantics
- docs (`README.md`, `ROADMAP.md`, `ARCHITECTURE.md`, release notes when applicable)
  must match implemented reality
- releases build once and reuse validated artifacts; do not rebuild between validation
  and publication

## 9. Collaboration model

The development workflow used for this repository is:

- **ChatGPT is the primary programmer/reviewer.** It owns architecture, code review,
  GitHub changes, CI interpretation, and decisions.
- **Hermes Agent is a secondary Linux executor.** It is useful for local filesystem,
  Cargo, packaging, or `gh` operations that cannot be performed through the connected
  GitHub tooling.
- Hermes runs a weaker HY3 model and must receive deterministic, shell-oriented
  instructions with exact branch/SHA guards, stop-on-failure behavior, and explicit
  expected output.
- Do not delegate architecture, ambiguous fixes, or safety decisions to Hermes.
- Work in coherent large batches, while preserving exact-head review gates.

## 10. Release baseline and policy

v0.20.0 is a productive release. Its publication contract includes:

- tag target `3db9cee78d056e4e568e9d9a7f08fb0f579ea707`
- Linux x86_64 tar.gz
- Debian `.deb`
- RPM `.rpm`
- `SHA256SUMS`
- all package binaries from the same validated ELF
- exact package payload checks
- generated third-party license report

Future releases keep this one-build/no-rebuild validation/publication contract unless a
separately reviewed release design changes it.

## 11. Product backlog after architecture sequence

Do not lose the remaining product backlog while P/Q/R are in progress:

- #7 tmux/screen — screen discovery + real-terminal attach/detach hardening
- #10 mouse — pane wheel, Shift+Click range selection, provider-aware context
  availability
- #16 split panes — horizontal mode, resize, explicit close semantics
- #13 WebDAV post-MVP — auth/interoperability/recursive-operation work under explicit
  safe contracts
- cross-backend Move
- SFTP → SFTP workspace sync
- recursive remote delete
- binary remote editing
- additional Linux architectures / package-repository distribution if justified
- S3 read-only inspector/analytics with explicit evidence source and freshness

Architecture work should make these later features easier to integrate, not silently
expand their scope.

## 12. New-session startup checklist

A fresh development session should do this before changing code:

1. Read this file and `ROADMAP.md`.
2. Read issue #180.
3. Fetch current `main` SHA; never assume the snapshot SHA above is still current.
4. Inspect #178 and PR #179. If #179 is already merged/closed, continue from the next
   incomplete pack in #180 rather than repeating PACK O.
5. Check exact-head CI/workflow evidence for the currently active PR.
6. Compare current docs/source against any review supplied by the user; reviews may be
   stale.
7. Continue as primary programmer. Use Hermes only for deterministic mechanical work
   that connected tooling cannot execute.

A useful minimal handoff prompt is:

> Continue development of `github.com/mrAibo/arx`. Read
> `docs/DEVELOPMENT_HANDOFF.md`, `ROADMAP.md`, issue #180, and the current state of
> #178/#179. Treat current GitHub state as authoritative and continue from the first
> unfinished pack. You are the primary programmer; Hermes is mechanical Linux-only
> assistance.
