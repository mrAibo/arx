# ARX Architecture

ARX is a terminal commander for local and remote workspaces. The architecture is designed around one rule: presentation may observe runtime truth, but it must not invent or own it.

The same principle applies to navigation, transfers, background jobs, Remote Workspace synchronization, and post-sync verification.

## Core invariants

1. The TUI does not call filesystem, SSH, rsync, or archive operations directly.
2. Keyboard routing, discovery, and contextual hints share typed `Action` and runtime `Keymap` truth.
3. `Location` identifies where data lives; providers own filesystem behavior for those locations.
4. VFS browsing and transfer/sync execution are separate layers.
5. Long-running work is represented by JobManager jobs.
6. Preview is not execution.
7. Destructive sync requires explicit confirmation of the exact frozen plan.
8. Execution completion is not proof that two workspace roots are synchronized.
9. Verification reports what the available evidence proves: `Synchronized`, `DifferencesRemain`, or `Inconclusive`.
10. OpenSSH remains authoritative for connection behavior wherever practical; ARX does not disable host-key verification or invent a parallel credential system.

## Input and application flow

Physical keys do not map directly to renderer branches as a second source of truth. The current flow is:

```text
Crossterm input
      ↓
InputContext + KeyRouter
      ↓
runtime Keymap
      ↓
typed Action
      ↓
Action availability / controller logic
      ↓
Effect / service / JobManager
      ↓
accepted runtime truth
      ↓
TUI presentation
```

`src/app/actions.rs` owns stable action identifiers and shared presentation metadata. `src/input/keymap.rs` owns the runtime bindings. `src/input/hints.rs`, Command Center, and the contextual footer derive discoverability from those same sources rather than keeping a separate shortcut table.

`AppState` contains presentation and accepted application state, but asynchronous operation ownership remains in services and JobManager.

## Pane navigation and effects

Pane navigation is transactional. A requested location is not presented as the current pane root until the matching asynchronous load succeeds.

```text
Action / navigation request
      ↓
EffectDispatcher
      ↓
Pane loader / ProviderRegistry
      ↓
correlated response
      ↓
accepted pane state
```

Stale responses are rejected by generation/correlation identifiers. Presentation-only states such as loading and `PaneLoadUiError` can remain visible without changing provider truth.

## Location and provider model

ARX uses typed locations instead of passing raw path strings through the application:

```text
file:///home/user/project
sftp://prod/srv/project
archive:///tmp/data.tar.zst!/etc
```

A `ProviderRegistry` resolves each location to provider behavior and capabilities. The currently relevant providers are local filesystems, SFTP, and archives. S3/WebDAV modules remain stubs and are not advertised as production backends.

VFS responsibilities include browsing and filesystem-oriented operations such as list/read/stat/mkdir/remove where supported. Transfer planning is deliberately separate.

## Transfer stack

Normal F5/F6 copy and move use the transfer stack:

```text
TransferRequest
      ↓
TransferPlanner
      ↓
TransferPlan
      ↓
executor selection
      ↓
Native / rsync / SFTP adapter
      ↓
JobManager
```

The planner considers source/destination locations, provider capabilities, requested intent, and executor availability. Executors report structured progress and terminal results rather than letting the TUI infer lifecycle from child-process text.

### Transactional SFTP copy

SFTP copy uses staging so a failed transfer does not silently replace the destination with a partial file:

1. upload/download to an ARX temporary path;
2. preserve an existing destination when required by the transaction;
3. commit the staged file;
4. restore/clean up on failure where the adapter can do so safely.

Remote-to-remote SFTP relay is not a hidden fallback for workspace synchronization. Unsupported transport combinations are blocked explicitly.

## Remote Workspace

Remote Workspace is a second, stricter pipeline built on top of providers, jobs, and transfer execution.

```text
WorkspaceScanner
      ↓
WorkspaceEntry fingerprints
      ↓
WorkspaceDiff
      ↓
WorkspaceSyncPlan (Preview)
      ↓
FrozenSyncPlan
      ↓
revalidation
      ↓
Execution Compiler
      ↓
JobManager / WorkspaceSyncExecutor
      ↓
post-sync Verification
```

### 1. WorkspaceScanner

Both pane roots are scanned recursively through the provider layer. Scan responses are correlated with the accepted roots; stale results do not replace newer workspace state.

The comparison model is provider-neutral. A `WorkspaceFingerprint` may contain entry kind, size, modification time, and content hash when those facts are available.

### 2. WorkspaceDiff

`WorkspaceDiff` classifies each relative path as:

- same fingerprint;
- only on the left;
- only on the right;
- left newer;
- right newer;
- different / not safely ordered.

ARX intentionally does not treat equal size alone as proof of equality. Equal hashes are proof; where hashes are unavailable, matching size plus matching timestamp can provide provider-neutral fingerprint equality. If evidence is insufficient, the entry remains different/unverified rather than being promoted to identical.

### 3. Sync Preview

`WorkspaceSyncPlan` transforms the diff into explicit operations for a direction and mode.

**Update** is the default. Source-only/source-newer entries become copies; destination-only entries are preserved.

**Mirror** may turn destination-only entries into deletes. Conflicts remain explicit. A plan with unresolved conflicts is not executable.

The preview is presentation of this plan only. No mutation has started.

### 4. Frozen plan and confirmation

Before execution, ARX freezes the exact preview into an immutable execution context with a plan identity/digest and operation preconditions.

Destructive execution confirmation is tied to that exact frozen plan. If the workspace changes, old confirmation cannot authorize a different plan.

Immediately before queueing, ARX rescans/revalidates the frozen plan. A stale source/destination state is rejected before job creation.

### 5. Execution Compiler

The compiler translates the validated frozen plan into physical executable steps and rejects unsupported/unsafe paths or transport combinations. This is where logical sync intent becomes executor work.

The compiler does not silently invent a remote-to-remote relay when no supported executor exists.

### 6. JobManager and executor

JobManager is the runtime source of truth for sync lifecycle and cancellation.

Relevant lifecycle states include:

```text
Pending → Running → Completed
               ↘ Cancelling → Cancelled
               ↘ Failed
```

Workspace jobs also retain structured execution outcome/progress, including physical step counts and transfer bytes where those facts are available.

A visible overlay can be hidden without cancelling the job. Background completion does not take focus back from the user.

## Post-sync verification

Verification starts only after workspace execution reaches a terminal state that permits it. It is deliberately stored as a separate fact from the Job status.

```text
Job execution Completed
      ↓
scan left root again
scan right root again
      ↓
new WorkspaceDiff
      ↓
verification evidence
      ↓
Synchronized | DifferencesRemain | Inconclusive
```

### Why execution completed ≠ workspace verified

A transfer executor can successfully finish every compiled step while the final workspace state still differs because of concurrent changes, provider limitations, or evidence that cannot prove equality.

For that reason:

- `JobStatus::Completed` describes execution lifecycle;
- `SyncVerificationStatus` describes the later verification lifecycle;
- `SyncVerificationVerdict` describes the evidence-based result.

`Inconclusive` is not rewritten as execution failure, but it is also not presented as synchronized.

## Presentation-only observers

Some UX state exists solely to help the user understand accepted backend truth.

Examples include:

- contextual hints/footer;
- persistent pane load errors;
- `SessionMilestones`;
- `SessionCallout`.

Session milestones observe the first accepted compare and first strict verified-sync success in the current process. They reset with a new `AppState`; there is no persistence/config migration behind them.

These observers never start a job, modify a plan, change verification, or steal focus from a background job.

## Remote hosts and OpenSSH

ARX separates host inventory from SSH connection resolution.

### Host inventory

Host Center reads ARX metadata from:

```text
~/.config/arx/hosts.toml
```

The file can define an id/name, `ssh_alias`, hostname, port, user, groups, tags, favorite status, default path, transfer preference, and notes.

ARX currently does **not** auto-import every `Host` stanza from `~/.ssh/config` into Host Center.

### Connection resolution

Connections use system OpenSSH behavior/configuration where supported. This includes aliases, custom ports, `IdentityFile`, `ProxyJump`, ssh-agent authentication, known-host verification, and other behavior resolved by the user's OpenSSH client.

`OpenSshSftpConnection` bridges a system `ssh` subprocess to the SFTP client session so ARX can reuse the user's established SSH environment instead of maintaining a competing SSH implementation.

Credentials and private keys are not stored in `hosts.toml`.

## Safety boundaries

- Update mode never deletes destination-only workspace entries.
- Mirror/destructive plans require explicit confirmation.
- Confirmation is bound to the frozen plan rather than to a generic “yes”.
- Frozen plans are revalidated before queueing.
- Conflicts are not silently overwritten by the default policy.
- SFTP staging protects against partial replacement during copy.
- Cancellation is explicit and shared with executors.
- Host-key verification remains enabled through system OpenSSH.
- Verification may report `Inconclusive` instead of inventing equality.
- Logs must not expose passwords, private keys, or sensitive environment values.

## Module map

The important ownership boundaries in the current tree are:

```text
src/
├── app/
│   ├── actions.rs              typed Action catalog + input contexts
│   ├── availability.rs         action availability truth
│   ├── command_center.rs       typed discovery/search results
│   ├── remote_workspace.rs     accepted workspace presentation/workflow state
│   ├── workspace_sync_ux.rs    sync UX state machine
│   └── mod.rs                  AppState + presentation state
├── input/
│   ├── keymap.rs               runtime bindings + KeyRouter
│   └── hints.rs                contextual hints from Action/Keymap truth
├── effect_dispatcher.rs        async effect correlation/lanes
├── effects.rs                  effect contracts
├── services/
│   ├── pane_loader.rs          provider-backed pane loading
│   ├── workspace_scanner.rs    recursive workspace scanning
│   └── workspace_sync_controller.rs
├── workspace_sync.rs           diff + logical sync plan
├── workspace_sync_execution.rs frozen plan + validation/confirmation
├── workspace_sync_executor/    compiler + physical executor
├── workspace_sync_verification.rs
├── jobs/mod.rs                 JobManager + lifecycle/progress/results
├── transfer/                   normal copy/move planner + executors
├── vfs/                        typed locations/providers/capabilities
├── remote/                     hosts.toml + OpenSSH/SFTP integration
└── tui.rs                      event loop + rendering/presentation
```

## Testing and CI

Behavioral tests cover action/keymap routing, workspace comparison/planning, frozen-plan safety, compiler/executor behavior, JobManager lifecycle/cancellation, post-sync verification, remote transfers, and presentation regressions.

CI runs:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

Documentation intentionally avoids a hardcoded test count because the suite changes as ARX evolves.
