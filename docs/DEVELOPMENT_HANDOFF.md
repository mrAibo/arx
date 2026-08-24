# ARX Development Handoff

This document is the canonical continuation point for active ARX development. It is
intentionally written so a new development session can recover the current state,
working rules, and next sequence from GitHub without reconstructing chat history.

> **Authority rule:** current GitHub state and exact SHAs win over any stale chat,
> cached review, or old local checkout — and over this snapshot itself. Always
> re-fetch current `main` before making changes; this snapshot is evidence, not
> eternal truth.

## 1. Current baseline — 2026-08-24 (post-#10)

Repository: `mrAibo/arx`

- Current published release: **v0.20.0**
  (tag target `3db9cee78d056e4e568e9d9a7f08fb0f579ea707`; verify on GitHub if a newer
  release has shipped since this snapshot).
- Rust MSRV: **1.88**
- Product platform: **Linux only**; published artifact target is Linux x86_64.
- Accepted #10 runtime merge baseline: `4eadaa1c5bc3e230374ed80a1bd66540c40ca01f`
  (#10 / PR #236). After it, docs-only PR #237 landed and the branch state entering
  #7 is `80093fc5cdd1f4bb6e1bd16c55e117d257d327dc`. No self-referential "future
  current main" claim is made here.
- #10 mouse follow-up: COMPLETE via PR #236, accepted merge
  `4eadaa1c5bc3e230374ed80a1bd66540c40ca01f`; baseline entering #10 was
  `dad59ac7ae8b80c680767de7c7f95f506078bf44`.
- #214 configurable effective keymap: COMPLETE via PR #235, accepted merge
  `dad59ac7ae8b80c680767de7c7f95f506078bf44` (baseline entering #214 was
  `8444820bcb6b392e77a2cffad326b80b39d880b3`).
- Current product work: **#7 multiplexer lifecycle** on
  `feat/7-multiplexer-lifecycle`; baseline entering #7:
  `80093fc5cdd1f4bb6e1bd16c55e117d257d327dc` — no future merge SHA invented.
- #16 remains separate later product work; plugins remain no-GO; #233 remains separate.
- Earlier provenance — accepted PACK R merge
  `1def3adf382978491ce11590470f56d844bcce07` (PR #232 / #231).
- Provenance entering R: `578d52d685dec7454025521e6d06491273503239`. Its tree was
  exactly the accepted Q3 tree (`bf53c33807be918996d35b45fb4ebbfa7f37abf6`); the two
  housekeeping commits between accepted Q3 merge
  `c2f30feaf76a405c111f2996387113d6a93d2064` and that entering-R baseline had
  a NET zero file diff.

Important merge evidence (historical record):

- PACK M / PR #175 → `d8abad76623b9fb22190de15249acbfb6fa12673`
- PACK N / PR #177 → `eec15d91d264a40760b6772135de516d40f1b95c`
- PACK Q1 / #225 → merge `872298913640a7add5ed4a19748a66f9a8de3113`
- PACK Q2 / #227 → merge `b8e5f9a5761875d6b25dfa047d9b93af5619b1f6`
- PACK Q3 / #229 → accepted merge `c2f30feaf76a405c111f2996387113d6a93d2064`
- PACK R / PR #232 / #231 → accepted merge `1def3adf382978491ce11590470f56d844bcce07`
- #214 / PR #235 → accepted merge `dad59ac7ae8b80c680767de7c7f95f506078bf44`
- #10 / PR #236 → accepted merge `4eadaa1c5bc3e230374ed80a1bd66540c40ca01f`

PACK N deliberately removed unsupported runtime surface (no `DISPLAY` guessing, no
`src/plugins` Lua prototype, no `pub mod plugins`, no `mlua`). Any review claiming
current `main` still contains those is stale.

## 2. Architecture pack status

Provenance: the O → P → Q → R sequence was tracked by umbrella issue **#180**
(CLOSED completed); PACK Q by umbrella issue **#224** (CLOSED completed).

| Pack | Scope | Status |
|---|---|---|
| PACK O | typed local Quick Actions (#178 / PR #179) | COMPLETE |
| PACK P | TUI decomposition into controllers/runtime | COMPLETE |
| PACK Q1 | concrete-location capability authority (#225) | COMPLETE |
| PACK Q2 | remove legacy Location execution bridge (#227) | COMPLETE |
| PACK Q3 | finalize resolver authority + docs truth (#229) | COMPLETE (accepted merge `c2f30feaf76a405c111f2996387113d6a93d2064`) |
| PACK R | internal feature/command registration (#231) | COMPLETE (accepted merge `1def3adf382978491ce11590470f56d844bcce07`) |

Deferred by explicit decision, not forgotten (status updated):

- #214 configurable effective keymap — COMPLETE via PR #235 (merge
  `dad59ac7ae8b80c680767de7c7f95f506078bf44`).
- #10 mouse follow-up — COMPLETE via PR #236 (merge
  `4eadaa1c5bc3e230374ed80a1bd66540c40ca01f`).
- external plugin runtime — **no GO** (see §5).

## 3. VFS authority model (final after PACK Q)

`src/vfs/mod.rs` documents this at its top; the summary:

```text
Location          = typed identity / address / navigation information
ProviderRegistry  = explicit provider execution authority
CapabilitySet     = exact-location / concrete-instance capability truth
VfsProvider       = backend provider interface
```

Copy/Move execution lives in `TransferPlanner` / transfer queue / executor; mutations
live in `MutationService` plus the typed Registry/provider mutation seams.

Two deliberate resolver seams exist and are kept distinct by design decision:

- `ProviderRegistry::provider_for_location` — string-path backend operations.
  Evaluates `Location::legacy_listing_path` first, so S3 fails closed before any
  provider construction.
- `ProviderRegistry::provider_for_page_location` — typed page/native-identity
  operations. S3 resolves its exact configured `S3Target`, WebDAV its exact
  `WebDavTarget`; Local/SFTP/Archive delegate to the string-path resolver.

Do NOT merge them, add a third resolver, or flatten typed identity. The legacy
`VfsOps` trait, the hidden thread-local `PROVIDER_REGISTRY`, `set_global_registry`,
and `with_registry_mut` were removed in PACK Q2 and a source-contract test keeps them
out (`tests/async_vfs_contracts.rs::vfs_module_has_no_legacy_execution_bridge`).

## 4. After PACK R

The O → P → Q → R architecture sequence is complete at accepted PACK R merge
`1def3adf382978491ce11590470f56d844bcce07`. Return to the product backlog (§8)
and explicit, separately reviewed future decisions:

- #214 configurable effective keymap — COMPLETE (PR #235); the KeyRouter/effective
  Keymap architecture is frozen and must not regress.
- #10 mouse follow-up — COMPLETE (PR #236); mouse remains a second input path into the
  same typed action/availability/selection truth and must not grow a parallel mutation model.
- #7 tmux/screen and #16 split-pane follow-ups remain separate product work.
- External plugins remain **no GO** (see §5). Any evaluation after R is a fresh
  decision gate, not a scheduled implementation.
- #233 remains a separate transfer-queue reliability observation; do not change
  transfer semantics merely to satisfy a timing-sensitive assertion.

## 5. External plugins — explicitly deferred

There is **no GO** for an external plugin runtime.

Do not reintroduce Lua, WASM, or Rust `.so` plugins without a fresh reviewed decision.
External plugins may be evaluated only if real user/ecosystem demand exists; the preferred direction would be read-only first and
either a genuinely sandboxed out-of-process protocol or capability-constrained
WASI-style execution.

Important security rule:

> A manifest permission such as `network = false` or `filesystem_write = false` is
> documentation, not enforcement, when an arbitrary native plugin runs as the same OS
> user. JSON over stdin/stdout gives crash/ABI isolation, but not a security sandbox.

Core-owned authority must remain intact for mutations. Provider plugins and
mutation-capable third-party plugins are late-stage concerns.

## 6. Engineering and safety invariants

These rules apply across all packs unless a separately reviewed design explicitly
changes them:

- exact SHA pinning for acceptance evidence; exact-head CI is authoritative
- Rust MSRV 1.88
- `cargo fmt --check` (CI formats with the pinned toolchain)
- Clippy all targets/features with warnings denied
- full test suite
- physical Apache WebDAV W1–W18 acceptance where WebDAV behavior changes
- physical MinIO transfer-queue retry acceptance where S3 transfers change
- fail closed when identity/capability/safety is ambiguous
- no fake progress, total, rate, ETA, capacity, or provider semantics
- one JobManager/runtime source of truth; no duplicate schedulers
- provider-native identity must not be reconstructed from presentation strings
- destructive or remote mutation paths require truthful transaction/cancellation
  semantics
- capability truth flows only through `ProviderRegistry` exact-location queries;
  no caller-side independent fallback authority
- docs (`README.md`, `ROADMAP.md`, `ARCHITECTURE.md`, release notes when applicable)
  must match implemented reality
- releases build once and reuse validated artifacts; do not rebuild between validation
  and publication

## 7. Collaboration model

- **ChatGPT is the primary programmer/reviewer.** It owns architecture, code review,
  GitHub changes, CI interpretation, and decisions.
- **Hermes Agent is a secondary Linux executor** used for deterministic, shell-oriented
  work with exact branch/SHA guards, stop-on-failure behavior, and explicit expected
  output. Do not delegate architecture, ambiguous fixes, or safety decisions to it.

## 8. Release baseline and policy

v0.20.0 remains the current published release as of this snapshot. Its publication
contract:

- tag target `3db9cee78d056e4e568e9d9a7f08fb0f579ea707`
- Linux x86_64 tar.gz, Debian `.deb`, RPM `.rpm`, `SHA256SUMS`
- all package binaries from the same validated ELF, exact payload checks
- generated third-party license report

Future releases keep this one-build/no-rebuild validation/publication contract unless a
separately reviewed release design changes it.

## 9. Product backlog after #214 and #10

Completed product follow-ups retained as provenance:

- #214 configurable effective keymap — COMPLETE via PR #235, merge
  `dad59ac7ae8b80c680767de7c7f95f506078bf44`
- #10 mouse — COMPLETE via PR #236, merge
  `4eadaa1c5bc3e230374ed80a1bd66540c40ca01f`

Remaining product backlog:

- #7 tmux/screen — screen discovery + real-terminal attach/detach hardening
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

## 10. New-session startup checklist

A fresh development session should do this before changing code:

1. Read this file and `ROADMAP.md`.
2. Treat closed issues #180 (architecture sequence) and #224 (PACK Q) as provenance,
   not active work.
3. Fetch current `main` SHA; never assume any SHA in this snapshot is still current.
4. Continue from the product backlog or an explicitly approved future decision; the
   O → P → Q → R architecture sequence is complete.
5. Check exact-head CI/workflow evidence for any currently active PR.
6. Compare current docs/source against any review supplied by the user; reviews may be
   stale.
7. Act as primary programmer. Use Hermes only for deterministic mechanical work that
   connected tooling cannot execute.

A useful minimal handoff prompt is:

> Continue development of `github.com/mrAibo/arx`. Read
> `docs/DEVELOPMENT_HANDOFF.md` and `ROADMAP.md`; treat closed issues #180 and #224
> as architecture provenance. Treat current GitHub state as authoritative and continue
> from the approved product backlog or an explicit future decision. You are the primary
> programmer; Hermes is mechanical Linux-only assistance.
