# ARX Roadmap

ARX has moved from “build the product foundation” into “package and release the product”.

The core systems work is now far enough ahead of the public surface that the next milestones deliberately focus on product truth, release readiness, and distribution rather than another broad TUI refactor.

## Current product baseline

Already landed on `main`:

- typed local/SFTP/archive locations behind provider/VFS boundaries;
- dual-pane commander workflow with tabs, navigation history, bookmarks, previews, shell/tmux workflows, mouse input, and file operations;
- transfer planning across native, rsync, and SFTP execution paths;
- transactional SFTP copy behavior;
- structured JobManager lifecycle, progress, cancellation, and terminal outcomes;
- Remote Workspace recursive compare;
- left → right and right → left sync policy;
- safe Update mode and explicit destructive Mirror mode;
- frozen-plan confirmation and stale-plan revalidation;
- compiled workspace execution through JobManager;
- post-sync verification with `Synchronized`, `DifferencesRemain`, and `Inconclusive` verdicts;
- canonical whole-second mtime evidence for Local/SFTP regular files, while missing evidence remains conservative;
- background-job UX that does not steal overlay/focus after navigation;
- typed Action/Keymap routing, Command Center discovery, contextual hints/footer;
- truthful empty/loading/error states and session-only first-success milestones.

This baseline is the product being prepared for the next public release.

## #38 — Product truth + killer demo

**Goal:** make the GitHub page explain the current product within the first 20 seconds.

Scope:

- reposition README around **Local ↔ Remote Workspace** rather than “Midnight Commander parity”;
- explain Compare → Preview → Execute → Verify as the primary product story;
- document the distinction between preview/execution and execution/verification;
- correct Host Center/OpenSSH wording;
- align README and architecture documentation with typed Action/Keymap, services, JobManager, frozen plans, and verification;
- replace stale static test counts with durable CI descriptions;
- create a deterministic 20–30 second non-destructive Update-mode demo storyboard;
- add the real captured GIF before #38 leaves Draft.

Out of scope:

- unrelated runtime Rust changes;
- version bump;
- release workflow changes;
- package-manager distribution.

## #39 — Verification evidence blocker ✅

**Goal:** make the real Local ↔ SFTP demo capable of ending on a truthful `Synchronized` verdict without changing verification semantics.

Landed before the hero capture:

- optional canonical modification-time evidence on VFS entries;
- Local filesystem mtime normalized to whole-second Unix resolution;
- SFTP mtime taken from existing `read_dir` attributes at the same canonical resolution;
- WorkspaceScanner preserves provider mtime evidence;
- executor stale-step validation observes the same evidence as frozen-plan scanning;
- missing mtime/hash still remains unproven rather than being guessed equal;
- no file hashing or per-entry remote stat fan-out was introduced.

Acceptance evidence:

```text
real disposable sshd
        ↓
real rsync -a Local → SSH/SFTP
        ↓
production SFTP provider + WorkspaceScanner
        ↓
WorkspaceDiff: SameFingerprint
        ↓
SyncVerificationVerdict::Synchronized
```

The acceptance fixture intentionally used non-empty regular files. Directory and symlink equality remains conservative where the current fingerprint model lacks enough comparable evidence.

## #40 — Release readiness

**Goal:** make the repository internally consistent and able to produce trustworthy release artifacts.

Planned gate:

1. Decide and lock the next release version. Given the existing public release line through `v0.14.0`, `v0.15.0` is the current natural candidate, but the bump happens only when the release is actually approved.
2. Align version truth across `Cargo.toml`, `Cargo.lock`, README/release references, and release notes.
3. Expand release artifacts deliberately rather than claiming platform support by compilation alone.
4. Add checksums (`SHA256SUMS`) for published artifacts.
5. Ensure packaging is gated by the normal Rust quality checks and release smoke evidence.
6. Document supported/unsupported targets explicitly.

Target matrix to validate:

```text
Linux
├── x86_64
└── aarch64

macOS
├── x86_64
└── aarch64
```

Windows should be added only after a real smoke pass confirms terminal behavior, filesystem semantics, external-tool assumptions, and packaging. Cross-compilation alone is not support evidence.

## #41 — Next public release

**Goal:** publish the Remote Workspace milestone as one coherent product release.

Current candidate theme:

```text
ARX v0.15 — Remote Workspace
```

Release notes should be organized by user-visible capability rather than by commit history:

- Remote Workspace;
- Safe Sync;
- Truthful Background Jobs;
- Post-sync Verification;
- Command Center & Discoverability;
- First-run UX;
- Safety / Architecture;
- Known Limitations.

The release gate is:

```text
README/product truth       ✅
real demo                  ✅
version alignment          ✅
release artifact matrix    ✅
checksums                   ✅
fresh CI                   ✅
release smoke              ✅
```

Known limitations should be stated explicitly, including at least:

- SFTP → SFTP workspace sync is intentionally unsupported;
- verification can be `Inconclusive` when available metadata cannot prove equality;
- directory/symlink equality remains conservative where comparable evidence is insufficient;
- Host Center does not auto-import all `~/.ssh/config` hosts.

## After the release — distribution and discovery

Only after the first well-packaged Remote Workspace release:

- Homebrew formula/tap;
- install script with checksum verification;
- AUR/package ecosystem work;
- GitHub About description and topics;
- release screenshots/GIF polish;
- comparison page versus MC, Yazi, Superfile, and adjacent terminal file managers;
- Show HN / Reddit / terminal-community launch material.

The packaging work should stay boring and trustworthy: no installer should outrun the release artifacts it installs.

## Future product work

These are candidates after the release track, not promises for the next release:

### Remote Workspace

- richer fingerprint evidence where providers can supply hashes/metadata efficiently;
- explicit conflict-resolution UX beyond the current conservative default;
- SFTP → SFTP execution only if a safe, observable transport design is chosen;
- richer sync speed/rate/ETA reporting when transports provide truthful measurements;
- persistent job/history views where persistence has a clear ownership model;
- optional persisted onboarding/hints only if session-only milestones prove useful.

### Remote hosts

- optional OpenSSH host discovery/import with explicit UX and deduplication rules;
- richer capability/cache presentation;
- reconnect/retry flows that preserve honest failure state.

### Providers and adapters

Potential future integrations include:

- rclone;
- restic;
- borg;
- SMB;
- WebDAV;
- object/cloud storage.

Provider additions must not weaken the existing Location/Provider capability model or Remote Workspace safety invariants.

## Explicit non-goals

ARX should continue to prefer existing operating-system capabilities and mature tools rather than reimplementing established protocols without a concrete reason.

Do not casually build custom replacements for:

- SSH protocol;
- SFTP protocol;
- rsync protocol;
- compressors;
- text editors;
- pagers;
- terminal emulators.

Do not claim features merely because a stub, target triple, or future-facing enum exists. Public documentation should describe behavior that is implemented and validated.
