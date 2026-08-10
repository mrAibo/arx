# ARX Remote F4 Edit — Design Audit

Feature branch: `feature/remote-edit`  
Date: 2026-08-11  
Status: AUDIT ONLY — no implementation

---

## 1. Current State: Local F4

### 1.1 Keybinding → Action

`Action::EditFile` triggered by edit key (configurable, defaults to F4).  
**File:** `src/tui.rs:4033-4055`

### 1.2 Guards (in order)

| Guard | File:Line | |
|---|---|---|
| File type | `tui.rs:4034` | `entry.kind == EntryKind::File` |
| Provider | `tui.rs:4038` | `matches!(location, Location::Local)` **← HARD BLOCK** |
| Editor configured | `tui.rs:4043` | `configured_editor` from `DesktopService::resolve_editor()` |

### 1.3 Availability

**File:** `src/app/availability.rs:176-188`

```rust
ActionId::EditFile if ctx.active_provider != ProviderId::Local => Disabled {
    reason: "Remote editing is not supported yet"
}
```

Blanket provider-type check. No capability check. Compare: F3 (ViewFile) already uses `Capability::Read` for SFTP (lines 155-170).

### 1.4 Editor Launch

```rust
terminal_session.suspend_while(|| DesktopService::open_editor(editor, &path)).await?
```

- Suspends terminal raw mode, spawns editor as child process, waits for exit
- Editor receives **original file path** — no temp copy, no change detection
- `DesktopService::open_editor()` → `editor_argv()` → `ProcessService::status()` — no shell interpolation
- After exit: `schedule_active_pane_load()` refreshes pane listing

### 1.5 What Local F4 Does NOT Do

- No temp copy — edits in-place on original file
- No change detection — always reloads listing
- No write-back verification
- No conflict handling

---

## 2. Existing Infrastructure Reusable for Remote Edit

### 2.1 SFTP Transactional Upload Pattern

**File:** `src/transfer/sftp_copy.rs:87-186` (`upload_file`)

Phase | Operation | Safety
---|---|---
Stage | `session.create(temp)` → `copy_stream` → `flush` → `shutdown` | Temp `.arx-part-{token}`, removed on failure
Verify | `session.metadata(temp)` → compare `.len()` with local source | Size mismatch → abort + cleanup
Backup | `session.rename(target → backup)` if target exists | `.arx-bak-{token}` suffix
Commit | `session.rename(temp → target)` | On failure: restore backup from `.arx-bak-{token}`
Cancel | `check_cancelled()` after stage, after verify, after backup | Each stage cleans up its artifacts

**Assessment:** Proven, battle-tested atomic SFTP write. Directly reusable for remote edit write-back. Currently tied to `TransferPlan`/`TransferIntent::Copy` — needs extraction into `SftpProvider::write_file_bytes()`.

### 2.2 SFTP Read (Bounded)

**File:** `src/vfs/sftp.rs:285-327` (`read_prefix_bytes`)

- Pooled connection with 1 retry
- Bounded read via `tokio::io::AsyncReadExt::take(cap)`
- Returns `BoundedRead { bytes, truncated }` (from PR #45)
- For full download: loop on `read_prefix_bytes` or add `read_all_bytes()` variant

### 2.3 DesktopService::open_editor

**File:** `src/services/desktop.rs:34-42`

- Accepts any `&Path` — no assumption about local vs temp
- Safe command construction via `editor_argv()`
- **Zero changes needed for remote edit**

### 2.4 Capability System

**File:** `src/vfs/capabilities.rs`

- `Capability::Write` already defined (line 13, value 2)
- `SFTP_CAPABILITIES` currently: `List | Read | Mkdir | Delete` (line 59-63)
- Missing: `Write` — SFTP CAN write (proven by `upload_file`), just doesn't advertise it
- **Fix:** Add `Capability::Write` to `SFTP_CAPABILITIES`, use `Read + Write` for F4 availability gate

### 2.5 Effect Pipeline

**File:** `src/effect_dispatcher.rs`, `src/process/mod.rs`

- `EffectDispatcher` spawns tokio tasks for async I/O (proven by PR #45 PreviewLocation)
- `EffectLane` enum: `Preview`, `GlobalProcess`, `LeftPane`, `RightPane`
- Add `EffectLane::RemoteEdit` — one line
- `ProcessService::execute_with_registry()` already accepts `ProviderRegistry`

### 2.6 tempfile Crate

Already a dependency (used in 40+ test locations). Production use for remote edit temp directory: zero new dependencies.

---

## 3. VfsProvider Trait Gaps

### 3.1 Missing: `write_file_bytes`

**Current:** No write method on `VfsProvider` trait. All writes go through transfer layer.

**Needed:** 
```rust
async fn write_file_bytes(&self, path: &str, data: &[u8]) -> io::Result<()>
```
Default: `Unsupported`. SftpProvider implements via extracted atomic staging pattern.

### 3.2 Missing: `metadata`

**Current:** No `metadata()` on trait. `SftpProvider` has internal `connection.session.metadata()` access.

**Needed:**
```rust
async fn metadata(&self, path: &str) -> io::Result<FileMetadata>
```
For remote revalidation: compare snapshot size/mtime before write-back.

---

## 4. Architecture Decision: Split-Phase vs Single Effect

The terminal suspension constraint (editor must run on main thread, raw mode off) means remote edit **cannot** be a single effect.

### Recommended: Split TUI + Effects (matches F3 pattern)

```
Phase 1: TUI dispatches download effect → EffectLane::RemoteEdit
         ├─ registry.read_all_bytes_at(location, name) 
         ├─ writes to TempDir
         └─ returns EffectEvent::Downloaded { temp_path, snapshot }

Phase 2: TUI → suspend_while(open_editor(temp_path))
         ├─ editor runs on main thread
         └─ TUI resumes

Phase 3: TUI checks local change (mtime/size diff)
         ├─ if unchanged: cleanup temp, message "no changes"
         ├─ if changed: read temp content, dispatch write-back effect
         │   ├─ registry.metadata_at(location, name) → revalidate
         │   ├─ if remote changed: EffectEvent::Conflict { ... }
         │   └─ if remote unchanged: registry.write_file_bytes_at(...) → atomic upload
         └─ cleanup TempDir
```

This reuses the existing `dispatch_ui_action` → `handle_effect_response` pattern proven by PR #45.

---

## 5. Gap Summary: What Needs New Code

File | What | Est. Lines
---|---|---
`src/vfs/capabilities.rs` | Add `Write` to `SFTP_CAPABILITIES` | 2
`src/vfs/mod.rs` | Add `write_file_bytes()` + `metadata()` to trait | 15
`src/vfs/sftp.rs` | Implement `write_file_bytes` (extract from upload_file) + `metadata` | 40
`src/vfs/local.rs` | Stub `write_file_bytes` + `metadata` (or default Unsupported) | 5
`src/app/availability.rs` | Replace `!= Local` with `Read + Write` capability check | 5
`src/effects.rs` | Add `DownloadRemoteFile`, `WriteBackRemoteFile` effects | 10
`src/process/mod.rs` | Add handlers for new effects | 60
`src/effect_dispatcher.rs` | Add `EffectLane::RemoteEdit` | 1
`src/tui.rs` | Split-phase EditFile handler for SFTP locations | 50
Tests | Download, write-back, conflict, cancellation, bounds | 200+

---

## 6. Safety Contract

| Contract | Mechanism |
|---|---|
| No silent overwrite | Atomic staging (`.arx-part-{token}` → rename) |
| Remote changed during edit | `metadata()` revalidation before write-back |
| Write-back failure | Backup restoration from `.arx-bak-{token}` |
| Partial temp file | Removed on error at every stage |
| Editor crash | Temp file stays in `TempDir` → auto-cleaned by Drop |
| Large file DoS | Bounded download (1 MiB cap, same as F3) |
| Binary file edit | Reuse F3 binary detection; refuse edit of binary |
| Temp directory security | `tempfile::TempDir` with `0o700` permissions |
| No shell interpolation | `editor_argv()` + `ProcessService::status()` — proven safe |

---

## 7. Out of Scope (explicit non-goals)

- Real-time remote sync (rsync/watch)
- Collaborative editing
- Directory editing
- Partial/sector writes
- Binary hex editing
- Image/PDF editing
- WebDAV/S3 remote edit
- Symlink remote edit
- Editor plugin/integration
- Edit history/undo beyond local editor

---

## 8. Implementation Order

Card | Description | Dependencies
---|---|---
1 | Add `Write` capability to SFTP | None
2 | Add `write_file_bytes` + `metadata` to VfsProvider trait | 1
3 | Extract atomic staging into `SftpProvider::write_file_bytes` | 2
4 | Add `EffectLane::RemoteEdit` + new effects | 2
5 | Implement download + write-back handlers in ProcessService | 4
6 | Change availability gate to `Read + Write` | 1
7 | Split-phase EditFile handler in tui.rs for SFTP | 4, 5, 6
8 | Tests: download, write-back, conflict, cancellation, bounds | 7

**Ready for approval → Card 1 implementation.**
