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
- [x] P1 — pure presentation-model extraction into `src/tui/` submodules
- [x] P2 — frame/render-only extraction
- [x] P3 — feature-controller extraction in independently green slices
- [x] P4 — keyboard/mouse routing extraction
- [x] P5 — Effect/Job response handling extraction
- [x] P6 — thin runtime/event-loop composition root
- [x] P7 — docs/final exact-head acceptance and PACK P closure

## Completed: P0 + P1a

The first code transaction was deliberately narrow and is complete.

PR #186 accepted exact head `43f70e8eb535c0eab7a658b7368a3970a131540a` with CI #644 / run `32589046943` fully green: quality, Rust 1.88 MSRV, Apache mod_dav W1–W18 physical acceptance, and MinIO safe-read retry physical acceptance.

Squash merge on `main`: `32f649816adb3e04b0d9f415ac352601a9b699a4`.

P0 added semantic characterization for the workspace ribbon while existing session-callout, footer/command-bar, Quick Action / Remote Edit safety, and S3 identity/selection regressions remained green.

P1a created `src/tui/presentation.rs` and moved only the workspace ribbon and session-callout presentation model. Frame rendering, routing, Action dispatch, Effect/Job handling, provider execution, and feature-controller ownership remained in `src/tui.rs`.

P1 remained open at that point only to allow later slices to reveal another meaningful presentation-model seam.

## P1 boundary decision

The final PACK P audit closes P1 at the accepted P1a boundary. No additional pure presentation-model seam emerged that justifies a separate P1 transaction: later visual work was correctly owned by P2 rendering/geometry extraction or by feature/routing modules with non-presentation responsibilities. Creating another P1 module solely to increase module count would not improve the architecture.

`src/tui/presentation.rs` therefore remains the focused pure presentation-model boundary, while P2 and later phases own the distinct rendering, controller and routing concerns exposed during decomposition.

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

## Completed: P3 orchestration-controller macro-batch

Tracked by canonical #202 through PR #205. Duplicate tracker creations #203/#204 were closed immediately without code changes.

The transaction extracted three large feature-orchestration seams while preserving the P3/P5 boundary:

1. Quick Actions orchestration into `src/tui/quick_actions.rs` — implementation commit `bfde833b0a21204baf6fd53e68c7690a7f2f58a2`;
2. Remote Edit lifecycle/initiation/deferred editor into `src/tui/remote_edit.rs` — implementation commit `cbbd772e4eb65438e06cf90eb52e805d787c3c88`;
3. Workspace Sync action orchestration and feature rendering into `src/tui/workspace.rs` — implementation commit `897a4716511a3e8c4273121d654191ace886c33c`.

Quick Actions keeps generic command input, mkdir, and shell execution in the parent; only an already-frozen `QuickActionPrompt` is completed through the feature helper. Remote Edit preserves the one shared Job lifecycle, phase ordering, stale-origin fail-closed behavior, deferred editor placement, and existing `EffectLane::RemoteEdit`. Workspace preserves immutable-plan, confirmation, launch supersession, queue-boundary, and verification presentation truth while reusing the existing `WorkspaceSyncController` and JobManager.

The P3/P5 ownership boundary remains explicit: `handle_effect_response`, generic `EffectEvent` application, `workspace_scan_rx`, `sync_launch_rx`, `verification_rx`, and `job_rx` remain parent-owned in `src/tui.rs`. No second EffectDispatcher, JobManager, TransferQueueRuntime, WorkspaceSyncController, scheduler, provider registry, or response loop was introduced.

The implementation head `897a4716511a3e8c4273121d654191ace886c33c` passed the full locked local suite reported as 1204 passed / 12 ignored and exact-head CI #669 / run `32636005836`: quality, Rust 1.88 MSRV, Apache mod_dav W1–W18 physical acceptance, and MinIO transfer retry physical acceptance.

Squash merge on `main`: `8dad8f5746ad3010bcd61fb8a4d41c54884c6dfe`.

## Completed: P3 final feature-controller macro-batch

Tracked by #207 through PR #208.

The final P3 transaction removes the remaining cohesive stateful feature orchestration from the parent TUI composition root:

1. `src/tui/transfers.rs` owns Copy/Move planning and enqueue orchestration while preserving exact S3 `S3ObjectRef` identity, WebDAV planning, executor/capability truth, and the one existing `TransferQueueRuntime` — implementation commit `1eac8dd5b15174c7bb895b5972414e26b10e5973`;
2. `src/tui/mutations.rs` owns Mkdir initiation/prompt completion, local Trash, frozen SFTP/S3 delete plans, fail-closed Remote Delete preflight/execution, and confirmation rendering while reusing the existing JobManager/event channel and provider registry — implementation commit `1e193fc8dcfe77610e01ac7d075a1a2c92f8b68c`;
3. `src/tui/embedded_terminal.rs` owns embedded-terminal toggle, active key translation, and right-pane rendering while parent runtime code keeps PTY drain timing and the same high-priority input/render predicates — implementation commit `bcdbea4a24e2bb3219e4f76ae62e0788866d58f6`.

A fourth commit, `9967daa635786b1e696650ef908b52791e2c21b3`, is test-only mechanical repair after the full suite exposed two source-contract fixtures that still expected the pre-extraction Copy/Move location. The retarget keeps product delegation through `sync.transfers.enqueue(...)`; independent review confirmed the authoritative `TransferQueueRuntime` still obtains the cancellation token from its one JobManager and the behavioral transfer-queue contracts continue to exercise real queue cancellation.

Hermes reported final locked local acceptance at 1210 passed / 12 ignored with fmt/check/clippy/Rust 1.88/diff-check green. Final docs head `e0e339b3b29f371bd4f508784d90007d21ef326b` passed exact-head CI #673 / run `32639162619`: quality, Rust 1.88 MSRV, Apache mod_dav W1–W18 physical acceptance, and MinIO safe-read retry physical acceptance all succeeded with exact-SHA evidence. PR #208 squash-merged as `a6b275e22b6d8e45b3f3f78f82e0eadc852353a5`.

## P3 boundary decision

P3 is complete at merge `a6b275e22b6d8e45b3f3f78f82e0eadc852353a5`.

Stateful feature ownership is now explicit for SSH Hosts, Bookmarks, Hosts, Jobs, User Menu, Quick Actions, Remote Edit, Workspace Sync, Transfers, Mutations/Remote Delete, and Embedded Terminal. Transfer Center, Storage Inspector, and Filesystems already have their own existing UI modules and are deliberately not wrapped again solely to increase module count.

The remaining parent-owned action arms are intentionally small composition/leaf dispatch such as local View/Edit, shell/link command seeding, simple overlay toggles, and one-shot effect launches. Turning each into a one-use module would not create a meaningful controller boundary and is deferred to later composition-root thinning only if it materially improves P6.

The later-phase boundaries remain frozen:

- P4 owns keyboard/mouse routing, Help/Viewer/Command Center interaction, Which-Key, command-bar hitboxes, Hotlist interaction, and the pre-existing duplicate legacy `Ctrl+\\` routing collision recorded as #206.
- P5 owns async response application: `effect_rx`, `job_rx`, workspace scan/launch/verification receivers, `handle_effect_response`, generic `EffectEvent` application, and `handle_job_event`.
- P6 owns final event-loop/composition-root thinning after P4/P5 establish those boundaries.

No new provider/VFS semantics, keybindings, scheduler, JobManager, TransferQueueRuntime, EffectDispatcher, ProviderRegistry, terminal runtime, or generic controller framework were introduced by P3.

## Completed: P4a/P4b authoritative Ctrl+\\ routing and Hotlist interaction

Tracked by #206 through PR #209.

This is an explicitly reviewed P4 bug-correction slice rather than a behavior-preserving extraction: three legacy `Ctrl+\\` arms had accumulated for external-open, Hotlist, and split, and match ordering made only the first reachable. Repository history plus the current README/#16 contract established **Ctrl+\\ = Toggle split pane** as the authoritative user-visible behavior.

P4a moves that physical shortcut into the Browser `Keymap` with exactly one binding to `Action::ToggleSplitPane`, adds typed Action Catalog entries for `ToggleSplitPane`, `OpenHotlist`, and `OpenInFileManager`, and removes all three raw legacy `Ctrl+\\` arms from `src/tui.rs`. Hotlist and Open-in-file-manager remain discoverable through the Command Center without inventing replacement global shortcuts. Open-in-file-manager is explicitly Local-only through `ActionAvailability` and retains a fail-safe dispatch guard.

P4b completes the previously incomplete Hotlist interaction as `src/tui/hotlist.rs`: entries load once on open, rendering performs no filesystem I/O, the controller owns Esc/Up/Down/Enter before KeyRouter, navigation reuses the existing PaneLoader path, and open/close transitions use the existing exclusive `OverlayKind::Hotlist` state machine. The pre-existing line-count-versus-percentage popup sizing defect was corrected to `centered_rect_lines`, including truthful empty-list rendering with a real content row.

Hermes implementation heads were `ec1aea573072a7c7c9283e6b9f041fc4489c2311` and `bf0c2809a8c65fc0865d7ea897eeff6debd22345`; independent review added the empty-state, overlay-state-machine, render-contract, and final dispatch-delegation corrections. Final code head `ac76ef772132858b058c61087d853c988723f3b8` passed local acceptance at 1225 passed / 12 ignored and exact-head CI #677 / run `32644347131`: quality, Rust 1.88 MSRV, Apache mod_dav W1–W18 physical acceptance, and MinIO safe-read retry physical acceptance all succeeded with exact-SHA evidence.

Final docs head `b6e39c20be2bb45180a29b527f8a33929bc6b4e4` passed exact-head CI #678 / run `32646058741` with the same four gates green; PR #209 squash-merged as `e648c0b110a136d73111e638001a874016aadeb6`, closing #206 as completed.

## Completed: P4c/P4d/P4e overlay routing ownership

Tracked by #210 through PR #211.

This behavior-preserving macro-batch extracts the remaining large pre-KeyRouter overlay keyboard seams while keeping runtime-heavy execution in the parent composition root:

1. P4c — `src/tui/help.rs` owns Help rendering and its pre-KeyRouter close/scroll handling. Unknown Help keys still pass through exactly as before, close keys clear any pending KeyRouter chord, and the F1 Help text was explicitly corrected to the already-approved #206 contract (`Ctrl+\\` = split pane; Open-in-file-manager remains Command Center discoverable). Implementation commit: `c7103fbdbd993d2981e8b7ffd831737b1710bd15`.
2. P4d — `src/tui/viewer.rs` owns Viewer rendering and keyboard handling. An active Viewer still consumes every keyboard event, preserves exact close/scroll bounds, and leaves Viewer mouse-wheel handling in the parent `Event::Mouse` path for the later mouse-routing slice. Implementation commit: `537a8e30b7bb7a3f46ef55a39c622779519d62b9`.
3. P4e — `src/tui/command_center.rs` owns Command Center open/render/edit/navigation state and returns a narrow `KeyOutcome::Execute(CommandTarget)` for available Enter selection; `execute_command_target`, focused/other-pane derivation, EffectDispatcher use, navigation and pane refresh remain parent-owned. `src/tui/which_key.rs` owns the existing Which-Key presentation derived only from `KeyRouter::pending/continuations` and shared `action_meta`; no mutable Which-Key state or second shortcut table was introduced. Implementation commit: `e9b83b3ceed64730afa39f3fa10398da24436aab`.

Independent review confirmed the moved Help/Command Center close paths preserve the pre-existing legacy boolean-transition semantics rather than broadening behavior through a new `close_all_overlays()` side effect. Viewer/Help consume/pass-through behavior, Command Center disabled-selection behavior, `KeyResolution::Pending => continue`, and parent runtime authority all remain unchanged.

Hermes reported local acceptance at 1250 passed / 12 ignored with fmt/check/clippy/Rust 1.88/diff-check green. Exact implementation-head CI #680 / run `32651805160` on `e9b83b3ceed64730afa39f3fa10398da24436aab` passed quality, Rust 1.88 MSRV, Apache mod_dav W1–W18 physical acceptance, and MinIO safe-read retry physical acceptance with exact-SHA evidence.

P4 remains open after this slice only for the parent-owned mouse / command-bar hitbox routing boundary and any final authoritative legacy-routing audit. Feature additions from mouse follow-up #10 remain explicitly out of scope for PACK P refactoring. P5 response ownership remains unchanged.

## Completed: P4 final routing ownership

Tracked by semantic correction #212 and final ownership tracker #213 through PR #215.

The final P4 transaction first resolves three proven legacy routing contradictions as an explicitly reviewed semantic correction: **Ctrl+T = New tab**, **Alt+T = Full/Brief panel mode**, and **Ctrl+I = File info/stat**. Smart Tree and Infrastructure Center remain product capabilities through stable typed Action/ActionId/Action Catalog entries and Command Center discovery, but they receive no replacement global shortcut. Their stale Ctrl+T/Ctrl+I title claims were removed, and Infrastructure gains an explicit Esc close after losing its unreachable toggle key.

The subsequent ownership extraction is behavior-preserving:

- `src/tui/browser_input.rs` classifies only the remaining post-KeyRouter legacy Browser routes and performs no provider/process/effect/navigation/job execution;
- `src/tui/mouse.rs` owns existing mouse hitbox/pane/row classification while parent code still performs action dispatch, availability messaging, Viewer scroll mutation, context-menu mutation, selection and pane activation; no feature scope from #10 was added;
- `src/tui/command_bar.rs` owns two-row command-bar rendering and hitbox geometry, with one `PositionedChip` layout driving both pixels and clickable rectangles while contextual rows still come from the shared runtime Keymap.

Hermes implementation commits were `e09cf86b0d799464e210c44e5416c81d32de6571`, `15e77a878a829d95e6e7119585f594f67d9d4fe3`, `af85f2006b471bb9c1b5f23dafe9890c094406eb`, and `c45177cabf197b6fb7710685892a34dddec4c5c5`. Independent review added `96505300db669b0a1ae1aa557492d35f92e695c2`, removing a redundant second static shortcut-owner list from the legacy classifier and restoring authoritative Alt+T precedence while Smart Tree is open. CI #684 then exposed a self-referential source-contract assertion only; test-only commit `8bc75e5e6e7d741f32340c3a6cfcaefb7ca697a6` scopes that assertion to the production half of the module.

Final implementation head `8bc75e5e6e7d741f32340c3a6cfcaefb7ca697a6` passed exact-head CI #685 / run `32656621687`: quality, Rust 1.88 MSRV, Apache mod_dav W1–W18 physical acceptance, and MinIO safe-read retry physical acceptance all succeeded; both physical jobs passed exact-SHA evidence.

## P4 boundary decision

P4 is complete through PR #215. Final docs head `4d7e55243653a4cea2c5459c839d92e2d37e4253` passed exact-head CI #686 / run `32656821491`, and PR #215 squash-merged on `main` as `f553efffe06627bf6bcc113241ae762ada4f89a5`.

Migrated shortcuts are owned by `KeyRouter`; remaining explicit legacy Browser classification is isolated in `browser_input.rs` after `KeyResolution::Unhandled`; mouse geometry/routing classification is isolated in `mouse.rs`; and command-bar rendering/hitboxes share one geometry source in `command_bar.rs`. Feature controllers retain their pre-KeyRouter ownership. The final audit found no unresolved documented shortcut collision after #212.

Mouse feature additions remain in follow-up #10. User-configurable effective keymaps are separately documented in ROADMAP issue #214 and are intentionally not implemented by PACK P.

## Completed: P5 correlated async response handling

Tracked by #216 through PR #217.

P5 extracts response application while deliberately leaving channel/runtime ownership for P6:

1. `src/tui/pane_responses.rs` owns accepted pane-load and continuation/page application while preserving `PaneLoadId`/location rejection, transactional navigation, history, selection/cursor and continuation identity truth. Implementation commit: `aaf59142c287607de638c3cf61ccc9087a57fa36`.
2. `src/tui/effect_responses.rs` owns dispatcher finalization, cancellation conversion, Effect correlation and typed `EffectEvent` application while preserving stale lane/scope rejection, Quick Action refresh truth, Remote Edit terminal outcomes and origin-only refresh. Implementation commit: `eade1db3c345bd5121da2facf30813963bf077bc`.
3. `src/tui/workspace_responses.rs` owns Workspace scan, launch and verification response application while preserving scan/root correlation, launch-id plus frozen-plan-id acceptance, rejected-verification settlement, JobManager snapshot truth and one-shot milestones. Implementation commit: `d9c579bfd20bc8a3d0538f71075691071b5666a1`.
4. `src/tui/job_responses.rs` owns already-accepted JobEvent presentation/snapshot application and returns a narrow `JobResponseOutcome`; the parent still owns desktop notification spawning, pane refresh scheduling and terminal drain timing. Implementation commit: `012cef8cd07aae10b7c35ec344fa615aa36ac388`.

Independent review confirmed the branch contains exactly these four real commits and only `src/tui.rs` plus the four response modules. No production response module constructs a JobManager, EffectDispatcher, WorkspaceSyncController, ProviderRegistry, TransferQueueRuntime, channel, scheduler or response loop; test-only fixtures may construct local managers/registries.

The P5/P6 boundary remains explicit: `tokio::select!`, `workspace_scan_rx.recv()`, `pane_load_rx.recv()`, `pane_next_page_rx.recv()`, `effect_rx.recv()`, `sync_launch_rx.recv()`, `verification_rx.recv()`, `job_rx.recv()`, tick/input polling, deferred editor launch, notification spawning, post-job pane scheduling and terminal-drain placement remain physically parent-owned in `src/tui.rs` at the P5 boundary.

Exact implementation head `012cef8cd07aae10b7c35ec344fa615aa36ac388` passed CI #690 / run `32661952951`: quality, Rust 1.88 MSRV, Apache mod_dav W1–W18 physical acceptance, and MinIO safe-read retry physical acceptance all succeeded with exact-SHA evidence.

## P5 boundary decision

P5 is complete through PR #217. Final docs head `d68de74e2aea963049ded4e76d26646d0ceddbd8` passed exact-head CI #691 / run `32662141048`, and PR #217 squash-merged on `main` as `d5b244b51c47743d61dbe3edc6ebf3c0cb208cd1`, closing #216 as completed.

Response semantics are isolated behind four tested modules. P6 subsequently moved only runtime/event-source and input-dispatch composition; it did not reopen stale/correlation/refresh semantics.

## Completed: P6 thin runtime/event-loop composition root

Tracked by #219 through PR #220 from authoritative P5 merge `d5b244b51c47743d61dbe3edc6ebf3c0cb208cd1`.

P6 completed in three implementation commits:

1. `09569d127866f1a3a2487441006d11dc5d37d285` — `src/tui/runtime.rs` owns the single existing runtime authority set, all seven async response receivers and the 50ms Tick source behind typed `RuntimeEvent` multiplexing;
2. `3cd1f18e3af713dec20d860c0a710b7818770361` — `src/tui/input_dispatch.rs` owns application of an already-read crossterm `Event` while preserving the established controller/routing priority;
3. `d4239f8d240aabe4ea85de5481d9032329ee9507` — `src/tui.rs` is thinned to the composition lifecycle around render, typed runtime-event delegation, narrow root-owned pane-entry mutation, Tick polling and deferred editor drive.

During input-dispatch extraction, the originally sketched immutable API exposed a real ownership contradiction: `SwapPanes` and `UserMenuOrSort` mutate event-loop-owned backing entry vectors that are simultaneously borrowed by visible-row views. The reviewed #219 clarification resolved this without moving ownership into `AppState` or cloning data: `InputDispatchOutcome` carries `InputFlow` plus a narrow `EntryMutation`, and the parent applies only `SwapPaneEntries` / `ResortPaneEntries` after borrowed views are released. Existing outer-loop `continue` versus deferred-editor fall-through semantics remain explicit.

Independent GitHub review confirmed P5 response modules are absent from the P6 diff; `runtime.rs` performs receive/select only and no response application/crossterm polling; `input_dispatch.rs` performs no receive/select; and no duplicate JobManager, EffectDispatcher, WorkspaceSyncController, TransferQueueRuntime, ProviderRegistry, scheduler, channel family or terminal runtime was introduced.

Exact implementation head `d4239f8d240aabe4ea85de5481d9032329ee9507` passed CI #693 / run `32670337850`: quality, Rust 1.88 MSRV, Apache mod_dav W1–W18 physical acceptance, and MinIO safe-read retry physical acceptance all succeeded; both physical jobs passed Exact-SHA evidence. PR #220 squash-merged on `main` as `b8a7181589091ecb17f8205d8311ac15028e7b27`; GitHub signature verification is `verified=true`; #219 closed completed.

## P6 boundary decision

P6 is complete at merge `b8a7181589091ecb17f8205d8311ac15028e7b27`.

`src/tui.rs` remains the composition root rather than becoming empty indirection: it owns AppState/runtime setup, pane backing vectors and render ListState, frame derivation/render cadence, typed `RuntimeEvent` application to the existing P5 response modules, Tick `event::poll/read`, application of the narrow input outcome, terminal-drain/deferred-editor lifecycle and quit boundary. Runtime authority itself remains singular in `TuiRuntime`; feature, routing and response semantics remain in their established modules.

## P7 final structural audit

Tracked by #221 through PR #222. At accepted P6 `main` `b8a7181589091ecb17f8205d8311ac15028e7b27`, the final ownership map is:

- presentation model — `presentation.rs`;
- rendering/geometry — `overlays.rs`, `command_bar.rs`, and feature-owned renderers;
- feature orchestration — Bookmarks, Hosts, SSH Hosts, Jobs, User Menu, Quick Actions, Remote Edit, Workspace Sync, Transfers, Mutations/Remote Delete, Embedded Terminal, plus the already-existing Transfer Center / Storage Inspector / Filesystems UI boundaries;
- routing/interaction — authoritative `KeyRouter`, `browser_input.rs`, `mouse.rs`, Help, Viewer, Command Center, Which-Key and Hotlist boundaries;
- async response application — `pane_responses.rs`, `effect_responses.rs`, `workspace_responses.rs`, `job_responses.rs`;
- runtime event source — `runtime.rs`;
- already-read crossterm input application — `input_dispatch.rs`;
- composition lifecycle — `src/tui.rs`.

The audit found no remaining PACK P ownership contradiction that requires production movement. In particular, no second runtime authority, provider/VFS semantic change, new shortcut system or feature framework is needed to close this pack. P7 is documentation/exact-head acceptance only.

User-configurable effective keymaps remain future ROADMAP issue #214. Mouse feature additions remain #10. PACK Q ProviderRegistry / Location / CapabilitySet convergence and PACK R registration work remain subsequent scopes and are not pulled into PACK P.

## P7 closure decision

The structural-audit head `babfba0b561c172be4386fec244d2d8fe1c59370` passed exact-head CI #695 / run `32670718300`: quality, Rust 1.88 MSRV, Apache mod_dav W1–W18 physical acceptance, and MinIO safe-read retry physical acceptance all succeeded; both physical jobs passed Exact-SHA evidence.

That accepted audit resolves the final deferred P1 boundary, records completed P5/P6 evidence, and confirms the final PACK P ownership map. This final docs-only commit marks P7 and the sequence complete. It introduces no production, test, Cargo, README, provider/VFS, routing/keymap, release or packaging change and must itself pass the same exact-head CI before pinned merge and #183 closure.

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

PACK P must not introduce unreviewed:

- product actions/features or shortcut semantics outside a dedicated correction issue;
- provider/VFS behavior changes;
- duplicate JobManager / scheduler / Effect ownership;
- Lua/WASM/native plugin runtime;
- PACK Q ProviderRegistry convergence changes;
- PACK R registration abstractions.

If an extraction requires a semantic change to compile, stop and record/review that semantic change separately instead of hiding it inside the refactor.