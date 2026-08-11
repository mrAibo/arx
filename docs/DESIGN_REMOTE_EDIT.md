# ARX remote F4 editing over SFTP

Feature branch: `feature/remote-edit`
Status: implemented; merge requires the gates in this document

## User flow

F4 keeps the local in-place editor path unchanged. For SFTP files it uses a split flow:

1. The TUI dispatches `DownloadRemoteFile` on the remote-edit effect lane.
2. `ProcessService` reads one complete, stable remote revision into a private `TempDir`.
3. The TUI suspends terminal mode and opens the `working` copy in the configured editor.
4. If the copy changed, `ProcessService` revalidates the remote revision and performs guarded write-back.
5. The originating location is refreshed only after a completed write.

The remote-edit session owns the exact location, name, editor, private temp directory, immutable original revision, and working copy. Stale effect IDs or responses from another location cannot complete the session.

## Read contract

`MAX_REMOTE_EDIT_BYTES` is 16 MiB (`16_777_216` bytes). A file exactly at the limit is accepted. A file one byte larger is rejected before editor launch.

The SFTP reader:

- accepts only a regular file;
- validates a safe parent directory;
- creates a unique no-follow hardlink pin and opens that pinned inode;
- reads the metadata-sized content twice (or clones the first bounded prefix when oversized);
- compares content, size, mtime, mode, UID, and GID;
- keeps cancellation inside the provider and removes the pin before returning any terminal result.

There is no path from preview's bounded prefix reader into remote editing. Static symlinks and path-swap races are rejected without opening the symlink target. Hosts without the OpenSSH hardlink extension cannot use remote F4 editing.

The complete revision must be UTF-8 text without NUL bytes. Empty files and Unicode text are valid.

## Local temp contract

Each session uses a unique `tempfile::TempDir` with mode `0700`. It contains:

- `original`: the immutable bytes captured from SFTP;
- `working`: the copy given to the editor.

`TempDir` ownership ties cleanup to the session. Success, no-change, conflict, cancellation, editor failure, and ordinary error paths drop the session and remove the directory. A process crash may leave an operating-system temp artifact.

The edited file is read once with a 16 MiB + 1 bound. The reader checks regular-file type, inode/device identity, length, mtime, UTF-8 validity, and absence of NUL bytes before accepting it. A replaced, growing, truncated, symlinked, oversized, or binary working file is rejected.

## Write-back contract

No-change sessions do not call the provider.

Changed sessions carry `RemoteEditRevision`; there is no revision-blind public write API. The provider verifies content, mode, UID, and GID before mutation and again from the commit-time backup.

The SFTP transaction is:

1. Validate the target and its parent.
2. Create and verify an empty `0600` regular-file stage exclusively to prove the SFTP account UID.
3. Atomically create a unique `.arx-txn-*` namespace with mode `0700`, verify its UID, type, mode, emptiness, and unchanged parent, then move the empty stage inside it. SFTP v3 does not expose MKDIR attributes through the current client, so the atomic mode-bearing mkdir uses the same validated OpenSSH alias and an explicitly quoted `sh -c` command.
4. Probe `hardlink@openssh.com` inside the private namespace, then write, flush, close, and verify the staged bytes.
5. Apply the original mode, UID, and GID to the stage; verify all metadata.
6. Revalidate target content and metadata.
7. Immediately revalidate the parent and private namespace, move the target to the verified-absent backup name inside that namespace, and verify the backup against the immutable revision.
8. Link the verified stage into the target name without overwriting another entry.
9. Verify the visible target's exact content and metadata while both recovery links still exist, then remove stage, backup, and transaction namespace.

A conflict restores the original and leaves the competing content untouched. A failed commit with successful rollback returns failure. Failed or uncertain rollback returns `RecoveryRequired` with artifact paths. A successful commit followed by backup-cleanup failure returns `CommittedWithWarning`. Ambiguous transport failures invalidate the pooled SFTP connection and are never retried destructively.

## Metadata boundary

ARX preserves the SFTP metadata available through the current protocol implementation: Unix mode, UID, and GID. If it cannot apply or verify them, write-back stops before replacing the original. Physical acceptance includes a non-primary supplementary-group GID.

POSIX ACLs, extended attributes, security labels, file capabilities, birth time, and sub-second timestamps are not captured by the current SFTP API. Remote F4 must not be used for files whose security or behavior depends on that unsupported metadata. This limitation is fail-visible documentation, not a claim that those attributes survive inode replacement.

The target parent must have known Unix ownership. Group- or world-writable parents are accepted only when the sticky bit protects entries, as in `/tmp`; writable non-sticky parents are rejected because the transaction protocol cannot prove exclusive namespace control there.

## Cancellation and terminal behavior

Download and pre-commit work are cooperatively cancellable. A cancellable SFTP read owns its hardlink pin through cleanup rather than letting the caller drop the read future. Cancellation state remains registered until the TUI consumes the terminal response, so a queued download cannot open the editor after Quit. Once the target has been preserved as backup, rollback and recovery run to a terminal state instead of abandoning the transaction halfway through.

Quit never starts a new editor. A running editor exits through the existing terminal suspension lifecycle. Temp ownership and transaction cleanup do not depend on the active pane.

## Acceptance matrix

| Case | Required result | Evidence |
|---|---|---|
| E1 small UTF-8 edit | exact new remote content | physical `ProcessService` download/write-back |
| E2 file over 1 MiB | full tail preserved; over 16 MiB refused before editor | physical exact-limit, limit+1, and large-file cases |
| E3 same-size concurrent edit | conflict; external bytes survive | physical provider and `ProcessService` cases |
| E4 commit-window mutation | conflict/rollback; no silent overwrite | physical fault/race injection |
| E5 no local change | no provider write | production-flow test and mock call count |
| E6 binary input | editor event is never produced | physical NUL and invalid UTF-8 downloads |
| E7 mode `0600` | remains `0600` with original UID/GID | physical metadata assertions |
| E8 executable file | executable bits survive | physical `0755` case |
| E9 editor failure | remote untouched; temp removed | physical dispatcher/`ProcessService` download plus real nonzero editor exit |
| E10 write/commit failure | rollback or typed recovery outcome | physical fault injection |
| E11 navigation during download | stale response cannot write or steal focus | effect ID/scope tests |
| E12 Quit/terminal regression | no delayed editor, orphan, or terminal corruption | physical post-pin cleanup and queued-response cancellation through dispatcher/TUI handoff |

The physical fixture is ignored by default and runs against a disposable host:

```sh
ARX_SFTP_SMOKE_HOST=arx-demo \
  cargo test --test remote_edit_sftp_smoke -- --ignored --nocapture
```

The fault/race fixture uses the same environment variable. Remote artifacts must be absent after both fixtures.

The physical TUI lifecycle cases run with:

```sh
ARX_SFTP_SMOKE_HOST=arx-demo \
  cargo test --all-features physical_editor_nonzero_never_schedules_writeback \
  -- --ignored --nocapture
ARX_SFTP_SMOKE_HOST=arx-demo \
  cargo test --all-features physical_queued_download_cancel_never_reaches_editor \
  -- --ignored --nocapture
```

## Merge gate

Run:

```sh
cargo fmt --all
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --all-features
git diff --check
```

Remote F4 is merge-ready only after the local gate, physical SFTP acceptance, physical fault/race injection, clean remote artifact check, and an independent final review with zero BLOCKER and zero MAJOR findings.

## Non-goals

Remote editing does not provide collaborative editing, continuous sync, partial writes, binary/hex editing, directory editing, WebDAV/S3 support, or edit history beyond the configured local editor.
