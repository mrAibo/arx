# ARX S3 Design

> Status: **DESIGN / ARCHITECTURE PROPOSAL — NOT IMPLEMENTED.**
> Read-only audit deliverable. No Rust, no Cargo, no capability, no TUI changes were made.
> Return to orchestrator for architecture approval before any implementation card starts.

Every statement below is tagged with one of:

- **[WINSCP OBSERVATION]** — behavior seen/known in WinSCP's public S3 UX (GPLv3 source NOT read; behavioral reference only).
- **[AWS/S3 FACT]** — public Amazon S3 / S3-compatible API behavior.
- **[ARX CURRENT FACT]** — confirmed in `origin/main` (`bbf6313`), read directly from source.
- **[ARX DESIGN DECISION]** — proposed in this document.
- **[OPEN QUESTION]** — unresolved, needs orchestrator/product decision.

---

## 1. Goals

- Give ARX users S3 object-storage browsing and safe Local ↔ S3 transfer from inside the existing Commander UI.
- Reuse ARX's existing **Provider / TransferPlanner / Executor / Job / ActionAvailability** architecture instead of forking a parallel S3 path.
- Keep destructive operations fail-closed and capability-gated, matching ARX's existing safety invariants.
- Support AWS S3 **and** common S3-compatible endpoints (MinIO minimum; R2/Wasabi follow-ups) through configuration, not code branching.

## 2. Non-goals

- **[ARX DESIGN DECISION]** No S3→S3, SFTP↔S3, or cross-backend Move in the first MVP.
- **[ARX DESIGN DECISION]** No Workspace Synchronize against S3 in MVP-1 (preview-only follows in MVP-2).
- **[ARX DESIGN DECISION]** No bucket creation, no bucket deletion, no recursive prefix deletion at MVP.
- **[ARX DESIGN DECISION]** No S3 F4 remote edit in MVP (the current `RemoteEditRevision` is Unix-metadata oriented — see §13).
- **[ARX DESIGN DECISION]** No ARX-owned secret store; credentials come from the official AWS credential provider chain.

## 3. Source / reference boundary

- **[ARX CURRENT FACT]** WinSCP is GPLv3. This document uses WinSCP only as a **behavioral reference** for user-facing S3 file-manager semantics.
- **[ARX DESIGN DECISION]** Where a WinSCP behavior is adopted, the future implementation will be written independently against the **AWS/S3 public API** and the official **AWS SDK for Rust**, never by translating WinSCP C++ into Rust.
- **[ARX DESIGN DECISION]** Phrasing rule: say *"Behavioral reference: WinSCP"*, never *"ported from WinSCP"*.

## 4. WinSCP architecture findings

**Reference only — do not copy.**

| WinSCP concept | Role | Closest ARX concept |
|---|---|---|
| `TCustomFileSystem` | abstract FS backend | `VfsProvider` trait |
| `TS3FileSystem : TCustomFileSystem` | concrete S3 backend | future `S3Provider` |
| `IsCapable(...)` | per-operation capability flags | `CapabilitySet` |
| `TRemoteFile` | one object/file entry | `Entry` |
| current directory | open prefix | `Location` / `Target` |
| `Source()` | upload | Local→S3 `TransferMethod::S3` executor |
| `Sink()` | download | S3→Local `TransferMethod::S3` executor |
| `OperationProgress` | progress/cancel | `Job` + `CancellationFlag` |

**Evaluation:** ARX already separates provider, planner, executor, job, and availability. **[ARX DESIGN DECISION]** ARX's separation is cleaner than folding transfer lifecycle into the provider object; do **not** copy WinSCP's monolithic backend class.

## 5. WinSCP behavior adopted (behavioral reference only)

1. **[WINSCP OBSERVATION]** Buckets are listed at the S3 root; entering a bucket shows its (prefix-rooted) contents.
   → **[ARX DESIGN DECISION]** Adopt "target root → bucket list" mode (see §9, option A) as the recommended ARX default.
2. **[WINSCP OBSERVATION]** A `delimiter="/"` listing turns `CommonPrefixes` into virtual folders; `Contents` become files. A trailing-slash marker object (`foo/`) is rendered as the folder, not a duplicate file.
   → **[ARX DESIGN DECISION]** Adopt exactly this mapping for `ListObjectsV2`.
3. **[WINSCP OBSERVATION]** Pagination is handled transparently but historically eager; large buckets can be slow.
   → **[ARX DESIGN DECISION]** ARX will use **bounded asynchronous incremental listing** (see §11), not eager full pagination.
4. **[WINSCP OBSERVATION]** S3 "rename" is server-side copy + delete, never an atomic rename.
   → **[ARX DESIGN DECISION]** ARX MVP disables rename/move for S3 entirely (see §19).
5. **[WINSCP OBSERVATION]** Multipart upload with configurable part size, progress, and abort.
   → **[ARX DESIGN DECISION]** Adopt the multipart lifecycle states for the upload executor (see §16).
6. **[WINSCP OBSERVATION]** Credentials via AWS shared config/credentials, profiles, session tokens, region, custom endpoint, path style.
   → **[ARX DESIGN DECISION]** Delegate all of this to the AWS SDK credential provider chain (see §8).
7. **[WINSCP OBSERVATION]** Capability-driven UI hides unsupported operations.
   → **[ARX DESIGN DECISION]** Already ARX's model via `ActionAvailability` + `CapabilitySet`; extend it (see §14).

## 6. WinSCP behavior rejected

1. **[ARX DESIGN DECISION]** **One monolithic S3 backend class** that owns transfer lifecycle — rejected; ARX keeps `VfsProvider` (metadata/navigation) separate from `TransferMethod::S3` executor (data movement).
2. **[ARX DESIGN DECISION]** **Recursive virtual-directory delete** in initial implementation — rejected; a "folder" is a prefix, not a node. Non-empty prefix delete is BLOCKED in MVP (§18).
3. **[ARX DESIGN DECISION]** **Filesystem-style atomic rename presented to users** — rejected; S3 has no atomic rename. If ever added, it is Copy+Delete with an explicit transaction model (§19).
4. **[ARX DESIGN DECISION]** **Manual AWS credential-chain implementation** — rejected; use the SDK chain.
5. **[ARX DESIGN DECISION]** **Provider-specific retry hidden inside a generic operation** — rejected; retry class is explicit per operation type (§21).
6. **[ARX DESIGN DECISION]** **POSIX metadata faking** (mode/uid/gid) on S3 objects — rejected; `RemoteEditRevision` Unix fields do not apply to S3 (§13).
7. **[ARX DESIGN DECISION]** **Copying GPL implementation code or comments** — explicitly prohibited by the clean-room boundary (§3).

## 7. S3 object-store semantics

- **[AWS/S3 FACT]** S3 is a key/value store. An "object" is `(key, value, metadata)`. There are no real directories.
- **[AWS/S3 FACT]** A **bucket** is the top-level namespace. A **prefix** is a key substring up to `delimiter`. A key ending in `/` is just an ordinary object (often used as a folder marker).
- **[AWS/S3 FACT]** `ListObjectsV2` with `prefix=<current>/` and `delimiter="/"` returns `Contents` (leaf objects) and `CommonPrefixes` (one level of virtual folders).
- **[AWS/S3 FACT]** Responses are paginated (`IsTruncated`, `NextContinuationToken`). A bucket may hold millions of objects.
- **[AWS/S3 FACT]** Deleting a prefix does nothing by itself; you must delete each object key. Versioning may retain prior versions.
- **[AWS/S3 FACT]** "Rename" = `CopyObject` (dest) + `DeleteObject` (source). Either step can fail independently.
- **[AWS/S3 FACT]** `ETag` is **not** universally an MD5 of content: for multipart uploads it is a compound/opaque value. Treat ETag only as an identity/change signal, never as a content hash (see §20).

## 8. ARX target / config / auth model

- **[ARX CURRENT FACT]** Current `ArxConfig` (`src/config.rs`) holds **only** `ui` (theme/show_hidden/editor). There is **no** host or S3 inventory yet. SFTP hosts are configured externally via OpenSSH `~/.ssh/config` (`src/remote/ssh_config.rs`), not in `arx.toml`.
- **[ARX DESIGN DECISION]** Add a new optional `[[s3.targets]]` table to `arx.toml` for S3 inventory (identifiers/config only — **no secrets**):

```toml
[[s3.targets]]
id = "artifacts"
name = "Artifacts"
bucket = "company-artifacts"
region = "eu-central-1"
profile = "release"          # AWS CLI profile name; resolved by SDK chain

# S3-compatible endpoint (optional)
endpoint_url = "https://minio.lab.internal"
force_path_style = true      # required by some MinIO/R2 setups
```

- **[ARX DESIGN DECISION]** **Credentials are never stored by ARX.** Resolution order is delegated to the AWS SDK credential provider chain:
  1. Environment (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`)
  2. AWS shared credentials file (`~/.aws/credentials`) + shared config (`~/.aws/config`) profile
  3. Web Identity / role assumption
  4. Instance/container metadata (only when running in such an environment)
- **[ARX DESIGN DECISION]** If a connection genuinely requires static keys and no chain source exists, that is an explicit user decision made **outside** ARX (e.g. exported env vars). ARX does not persist access keys.
- **[ARX DESIGN DECISION — SECURITY]** ARX must never log: secret access key, session token, or signed-URL query credentials. Redaction is mandatory in any diagnostics path (see §27).

## 9. S3 Location and path model

- **[ARX CURRENT FACT]** `Location` enum currently has `Local(PathBuf)`, `Sftp { host, path }`, `Archive { archive, inner_path }`. There is **no** `S3` variant. `ProviderId::S3` exists but is unused by `Location::provider_id()`.
- **[ARX DESIGN DECISION]** Add a non-string, typed S3 location:

```rust
// conceptual only — no code change in this audit
Location::S3 {
    target: String,   // matches [[s3.targets]].id
    bucket: String,   // "" at target root (bucket list)
    prefix: String,   // "" at bucket root; no leading/trailing slash internally
}
```

- **[ARX DESIGN DECISION]** Path normalization rules (ARX-native, not a raw `String` everywhere):
  - Internal prefix stored **without** leading or trailing `/`.
  - Display as `s3://<bucket>/<prefix>/`.
  - Reject/normalize: double `//`, empty path components, trailing `/` in `parent()`/`child()` math, literal `..` segments.
  - UTF-8 keys are passed through unchanged (S3 keys are byte sequences; ARX treats them as UTF-8 strings).
- **[OPEN QUESTION]** Root mode — see §12 decision. Recommended default is **option A** (target root lists buckets).

## 10. Provider instance model

- **[ARX CURRENT FACT]** `ProviderInstanceKey` already distinguishes `Singleton(ProviderId)`, `SftpHost(String)`, `ArchiveFile(PathBuf)`. Capabilities are keyed by `ProviderId`; instances by the concrete resource.
- **[ARX DESIGN DECISION]** Add `ProviderInstanceKey::S3Target(String)` keyed by target `id`. This enables **multiple simultaneous targets** (AWS prod + AWS backups + MinIO lab) with no singleton `ProviderId::S3` routing.
- **[ARX DESIGN DECISION]** The `S3Provider` instance is created lazily per target `id` from the resolved config + SDK chain, and registered under `S3Target(id)`.
- **[ARX DESIGN DECISION]** UI identity: pane title shows `[S3 artifacts]` or `[S3 minio-lab]` — **never** credentials or endpoint secrets.

## 11. Listing and pagination

- **[ARX DESIGN DECISION]** Listing uses `ListObjectsV2` with `delimiter="/"`:
  - `Contents` → `Entry { kind: File, size, modified_unix_ms: LastModified }`
  - `CommonPrefixes` → `Entry { kind: Directory, size: None }` (virtual folder; no probe of child count)
- **[ARX DESIGN DECISION]** **Bounded asynchronous incremental listing.** The S3 provider's `list_async` returns the first page immediately; a "Load more" / background continuation fetches subsequent pages via `NextContinuationToken`. **[ARX CURRENT FACT]** ARX already has an async pane-loading/effect model (`src/services/pane_loader.rs`, `PaneLoadId`); S3 listing plugs into it.
- **[ARX DESIGN DECISION]** Never block the TUI awaiting a full million-object enumeration. A page cap (e.g. 1000) is the unit of work; `IsTruncated` drives continuation.
- **[WINSCP OBSERVATION]** WinSCP handles truncated responses and `NextMarker`; ARX adopts the continuation concept but keeps it incremental rather than eager.
- **[AWS/S3 FACT]** Edge cases to handle explicitly: zero keys returned, `MaxKeys` present but empty, provider returning `CommonPrefixes` without `Contents`, non-UTF8-in-practice keys.

## 12. Root / bucket / prefix navigation (DECISION)

Three candidate root modes:

- **A. Target root → list buckets** (each configured target id lists its accessible buckets).
- **B. Each target bound to one bucket; root = bucket root.**
- **C. Support both.**

**Comparison:**

| Mode | UX | IAM implication |
|---|---|---|
| A | One S3 entry point shows all buckets for the credential | Needs `s3:ListBuckets` + per-bucket access; broader IAM scope |
| B | Simpler, least-privilege per target | Tightest IAM; one bucket per config block |
| C | Most flexible, most complex | Mixed |

- **[ARX DESIGN DECISION]** **Default to A** (matches WinSCP's bucket-list-at-root UX and ARX's multi-target intent), but allow a target config to pin `bucket = "..."` which collapses directly to bucket root (mode B for that target). Mode C is thus achieved without a third code path.
- **[ARX DESIGN DECISION]** Virtual `..` semantics:
  - object/prefix `foo/bar` → parent `foo`
  - bucket root `bucket/` → target root (bucket list)
  - target root → no parent (terminal)

## 13. F3 bounded preview

- **[ARX CURRENT FACT]** `VfsProvider::read_prefix_bytes` and `read_all_capped` exist with `BoundedRead { bytes, truncated, unix_mode, unix_uid, unix_gid }`. `RemoteEditRevision` carries Unix `mode/uid/gid`.
- **[ARX DESIGN DECISION]** S3 `read_prefix_bytes` issues a **`GetObject` with `Range` header** (AWS/S3 FACT: range reads are supported) to fetch a bounded prefix for F3 preview. The `unix_mode/uid/gid` fields are `None` for S3 — they are meaningless for objects.
- **[ARX DESIGN DECISION]** **S3 F4 (remote edit) is OUT OF MVP.** The existing `write_file_bytes_if_unchanged` + `RemoteEditRevision` contract is Unix-metadata oriented and MUST NOT be generalized to S3 by reusing it. A future S3 F4 needs a provider-neutral revision model (content hash + size + version id where known), defined separately.
- **[ARX DESIGN DECISION]** F3 preview availability is gated by `Capability::Read` on the S3 target (see §14).

## 14. Capability matrix

- **[ARX CURRENT FACT]** `Capability` enum: `List, Read, Write, Mkdir, Rename, Delete, Copy, Move, Symlink, Chmod, ServerSideCopy`. `S3_CAPABILITIES = CapabilitySet::NONE`. `builtin_capabilities(ProviderId::S3)` returns `NONE`.
- **[ARX CURRENT FACT]** `action_availability` (`src/app/availability.rs`) currently **hard-codes** `ProviderId::Local | ProviderId::Sftp` in several arms:
  - `ViewFile` disabled unless `Local` or `Sftp` (lines ~155–162)
  - `Copy` available only if active **or** passive provider is `Local` (lines ~201–211)
  - `Move` available only `Local↔Local` (lines ~219–235)
  - `ToggleEmbeddedTerminal` requires `Location::Local` right pane
- **[ARX DESIGN DECISION]** Future cards must extend these arms to include `ProviderId::S3` **only where a capability is actually present and a safe executor exists**. Specifically:
  - `ViewFile` (F3): enable for S3 when `Capability::Read` present.
  - `Copy` (F5): enable when the *other* pane is `Local` and a `TransferMethod::S3` executor is available (Local→S3 or S3→Local). Do **not** enable S3→S3.
  - `Move` (F6): remain **Disabled** for S3 in MVP.
  - `Mkdir` (F7): enable only when `Capability::Mkdir` present (prefix-marker creation, §17).
  - `Delete` (F8): enable only when `Capability::Delete` present and target is a single object (§18).

**Phased capability matrix (proposed):**

| Capability | MVP-1 (browse+transfer) | MVP-2 (workspace) | MVP-3 |
|---|---|---|---|
| `List` | YES | YES | YES |
| `Read` (F3) | YES | YES | YES |
| `Write` (upload) | YES (real executor) | YES | YES |
| `Mkdir` (prefix marker) | maybe (opt-in) | YES | YES |
| `Delete` (single object) | YES (per §18) | YES | YES |
| `Copy` (Local↔S3 transfer) | YES (via planner) | YES | YES |
| `Move` | NO | NO | transaction-gated |
| `Rename` | NO | NO | transaction-gated |
| `ServerSideCopy` | NO | NO | later |
| `Symlink`/`Chmod` | NO | NO | NO (no POSIX semantics) |

- **[ARX DESIGN DECISION]** A capability bit turns on **only** when (a) the provider implements it and (b) a safe executor/strategy exists. `Copy` is NOT turned on merely because the Local→S3 transfer executor exists — it turns on when `Capability::Write` on S3 + a `TransferMethod::S3` executor are both real. This preserves the ARX invariant: *capability == implemented promise*.

## 15. Upload (Local → S3)

- **[ARX DESIGN DECISION]** Upload is a **transfer**, not a `VfsProvider` mutation. Flow:
  `Action::Copy` → `TransferPlanner` → frozen `TransferPlan { intent: Copy, method: S3 }` → `TransferMethod::S3` executor → `Job` → S3 `PutObject`/`CreateMultipartUpload` → physical outcome → verification (§20).
- **[AWS/S3 FACT]** Small objects use `PutObject`. Large objects use multipart (`CreateMultipartUpload` → `UploadPart` × N → `CompleteMultipartUpload`).
- **[ARX DESIGN DECISION]** Overwrite preflight: if destination key exists, require confirmation (ARX already requires preview/confirmation for destructive sync; single-file overwrite uses the same confirmation idiom).
- **[WINSCP OBSERVATION]** WinSCP uses a multipart threshold + part sizing + progress + abort. **[ARX DESIGN DECISION]** Adopt the lifecycle states (§16); concrete part-size/threshold constants are chosen during implementation against current S3 service limits, not copied from WinSCP.

## 16. Download (S3 → Local) and multipart / cancellation

- **[ARX DESIGN DECISION — DOWNLOAD]** Stream the object to a **staged local file** `.<name>.arx-part-<id>`, then `fsync` + verify + atomic `rename` to final destination. A cancelled or failed download **never** presents the partial file as the successful destination. If staged-file cleanup fails, report a warning with the artifact path.
- **[AWS/S3 FACT]** `GetObject` streams; cancellation mid-stream simply stops reading. Partial local file is cleaned per above.
- **[ARX DESIGN DECISION — MULTIPART UPLOAD STATES]** `Initiated → UploadingPart → Completing → Completed | Aborting → Aborted | RecoveryRequired`.
- **[ARX DESIGN DECISION — CANCELLATION TRUTH]** If cancellation occurs while an upload ID exists, ARX **attempts `AbortMultipartUpload`** (best effort). Outcomes:
  - abort success → `Cancelled`
  - abort status unknown / failed → **do not** claim remote cleanup; surface `CancelledWithRemoteArtifacts` (or equivalent factual state) so the user knows orphaned parts may exist (and S3 lifecycle rules typically expire them).
- **[ARX CURRENT FACT]** `CancellationFlag` (`src/vfs/mod.rs`) already provides cooperative cancellation checked at I/O boundaries — S3 executors reuse it.

## 17. Prefix creation (F7)

- **[WINSCP OBSERVATION]** `CreateDirectory` at S3 root would create a bucket; inside a bucket it creates a prefix/folder marker.
- **[ARX DESIGN DECISION — MVP DEFAULT]** `F7` inside a bucket creates a **zero-byte trailing-slash marker object** (`<prefix>/`) via `PutObject` with empty body. This is the minimal, side-effect-free way to make a virtual folder visible.
- **[ARX DESIGN DECISION]** **Bucket creation is NOT MVP.** It carries region/policy/ownership/ACL semantics beyond file management. Keep `F7` at target root disabled (or hidden) until a deliberate bucket-admin feature is designed.
- Internal labeling: the Commander shows "MkDir" but ARX describes it internally as **Create Prefix** for S3.

## 18. Delete (F8) — safety matrix

- **[ARX DESIGN DECISION]** Single-object delete uses `DeleteObject`. The confirmation is **factual**, stating only what ARX can prove:

```
PERMANENT S3 DELETE
Target: s3://<bucket>/<key>
This deletes the object from the current S3 view.
[If versioning state is known:] Versioning: <enabled|suspended|disabled>.
  enabled  → a delete marker is created; prior versions remain recoverable.
  disabled → the object is permanently removed.
[If unknown:] ARX cannot determine versioning state; no recovery guarantee is claimed.
```

- **[ARX DESIGN DECISION]** Safety matrix (MVP):

| Target | Available? | Confirmation | Physical op | Undo | Partial outcome |
|---|---|---|---|---|---|
| single object (unversioned) | YES | required | `DeleteObject` | none | N/A |
| single object (versioned) | YES | required, shows versioning consequence | `DeleteObject` (delete marker) | prior versions remain | N/A |
| empty prefix marker (`foo/`) | YES if proven empty | required | `DeleteObject` | none | N/A |
| non-empty virtual prefix | **BLOCKED** | — | none | — | — |
| bucket | **BLOCKED** | — | none | — | — |
| recursive delete | **NOT MVP** | — | none | — | — |

- **[ARS DESIGN DECISION]** ARX never claims "this cannot be undone" unless it has actually proven versioning is disabled for that bucket. Otherwise it states the known facts only.
- **[WINSCP OBSERVATION]** WinSCP may offer recursive virtual-directory deletion. **[ARX DESIGN DECISION]** Rejected for MVP (§6).

## 19. Rename / server-side copy (future)

- **[AWS/S3 FACT]** S3 has no atomic rename. "Rename" = `CopyObject(dest)` then `DeleteObject(src)`.
- **[WINSCP OBSERVATION]** WinSCP implements S3 rename as copy+delete and rejects directory rename.
- **[ARX DESIGN DECISION]** **MVP: `Rename` and `Move` disabled for S3.** Reason: copy-success + delete-failure creates a partial outcome (object duplicated / source retained).
- **[ARX DESIGN DECISION]** If later implemented, require an explicit transaction model:
  1. `CopyObject` to destination
  2. verify destination exists with expected size/ETag
  3. `DeleteObject` source
  4. terminal truths: `Completed` | `CopiedButSourceRetained` | `FailedBeforeCopy` | `RecoveryRequired`
- **[ARX DESIGN DECISION]** Copy+Delete is **never** represented to the user as an atomic rename. `ServerSideCopy` remains a separate, later capability.

## 20. Verification / checksum evidence

- **[ARX CURRENT FACT]** `WorkspaceFingerprint { kind, size, modified_unix_ms, content_hash }` — both `modified_unix_ms` and `content_hash` are `Option`, and the comparison engine *never invents* equality when a provider can't prove it.
- **[AWS/S3 FACT]** S3 evidence sources: `ContentLength` (size), `LastModified`, `ETag`, `VersionId`, `ChecksumCRC32/CRC32C/SHA1/SHA256` (when present), multipart checksum metadata.
- **[ARX DESIGN DECISION]** Classification:
  - **size** → reliable identity/change signal
  - **LastModified** → reliable mtime-equivalent (whole-second)
  - **ETag** → identity/change signal **only**; NOT a content hash unless the object is single-part AND the API proves MD5 semantics
  - **ChecksumSHA256/et al.** → true content-hash evidence when the upload requested it
- **[ARX DESIGN DECISION]** S3 contributes to `WorkspaceFingerprint` as: `kind`, `size` (reliable), `modified_unix_ms` (from `LastModified`), and `content_hash` **only if** a genuine checksum is available. **An ETag is never placed into `content_hash`** unless its semantic contract proves it is a content hash for that exact object situation.
- **[ARX DESIGN DECISION — TRANSFER VERIFICATION]** Define separately from future Workspace Sync:
  - Upload: compare local source size + (if available) checksum to `HeadObject`/`PutObject` result.
  - Download: compare `ContentLength` + (if available) checksum of the staged file before commit.
  - Outcomes: `Transfer verified` vs `Transfer completed, verification inconclusive` (when no checksum is available). Never claim verified without evidence.

## 21. Error / retry truth

- **[ARX DESIGN DECISION]** Classify every S3 operation:
  - **safe retry** (idempotent, no side effect): `ListObjectsV2`, `HeadObject`, `GetObject`
  - **idempotent retry with care**: `DeleteObject` (deleting twice is harmless), `CompleteMultipartUpload` (retry only if upload ID still valid)
  - **ambiguous**: `PutObject` (retry may duplicate but is otherwise safe), `UploadPart`
  - **destructive / no automatic retry**: none beyond above; ARX does **not** auto-retry anything that could amplify cost or side effects without user visibility
- **[ARX DESIGN DECISION]** No provider-specific retry hidden inside a generic operation. Retry policy is explicit per operation class and respects `CancellationFlag`.
- **[AWS/S3 FACT]** Common failures to map honestly: `NoSuchBucket`, `NoSuchKey`, `AccessDenied`, `InvalidRegion` (wrong region → redirect), expired credentials, network break, object disappearing mid-operation.

## 22. Job integration

- **[ARX CURRENT FACT]** `src/jobs/mod.rs` defines `Job` lifecycle (Queued/Running/Completed/Failed/Cancelled).
- **[ARX DESIGN DECISION]** Every S3 upload/download is a `Job`. Required outcomes: `Queued → Running → Completed | Failed | Cancelled`, plus S3-specific physical detail where necessary (multipart abort state, §16).
- **[ARX DESIGN DECISION]** Cancellation while parts exist → `AbortMultipartUpload` attempted; if abort status unknown, surface the factual `CancelledWithRemoteArtifacts` state rather than claiming clean cancellation.
- **[ARX DESIGN DECISION]** Download cancellation → partial staged local file cleaned; if cleanup fails, report warning + artifact path.

## 23. ActionAvailability integration (implementation checklist)

- **[ARX CURRENT FACT]** Per §14, `src/app/availability.rs` hard-codes `Local | Sftp` in `ViewFile`, `Copy`, `Move`, `ToggleEmbeddedTerminal`.
- **[ARX DESIGN DECISION — FUTURE CARD CHECKLIST]** Extend, do **not** rewrite:
  1. `ViewFile`: add `ProviderId::S3` branch gated by `Capability::Read`.
  2. `Copy`: replace the `== Local` check with a capability+executor-aware check: available when one side is `Local` and the other is `S3` with `Capability::Write` + `TransferMethod::S3` executor available; S3→S3 stays disabled.
  3. `Move`: keep `S3` disabled in MVP.
  4. `Mkdir`: already capability-gated; S3 prefix-marker creation turns on with `Capability::Mkdir`.
  5. `Delete`: already capability-gated; S3 single-object delete turns on with `Capability::Delete`.
  6. `EditFile` (F4): keep disabled for S3 in MVP.
  7. `ToggleEmbeddedTerminal`: remains `Location::Local` only.
- No behavioral change to existing Local/SFTP gating.

## 24. TransferPlanner integration

- **[ARX CURRENT FACT]** `TransferMethod { Native, Rsync, Sftp, Scp }`. `is_local_remote_pair` matches only `(Local,Sftp)|(Sftp,Local)`. `choose_method` returns `Unsupported` for anything else.
- **[ARX DESIGN DECISION]** Add `TransferMethod::S3` and an `s3: bool` field to `ExecutorAvailability`. Extend `choose_method`:
  - `Local → S3` or `S3 → Local` with `intent: Copy` and `executors.s3` → `TransferMethod::S3`.
  - `S3 → S3`, `SFTP ↔ S3`, `Move` across backends → `Unsupported` in MVP.
- **[ARX DESIGN DECISION]** Planner invariant preserved: **if no safe executor exists → `Unsupported`**. S3 is never routed through `Rsync`/`Scp`/shell as a hidden fallback.
- **[ARX DESIGN DECISION]** First supported transfers: Local→S3 Copy, S3→Local Copy. NOT MVP: S3→S3, SFTP→S3, S3→SFTP, cross-backend Move, Workspace Synchronize.

## 25. AWS SDK / MSRV decision

- **[ARX CURRENT FACT]** ARX `Cargo.toml`: `version = "0.15.1"`, `edition = "2024"`, `rust-version = "1.88"`.
- **[AWS/S3 FACT]** The official AWS SDK for Rust (`aws-config`, `aws-sdk-s3`) is generated from Smithy; it depends on `tokio`, `hyper`/`hyper-util`, `aws-smithy-*` and a TLS impl (typically `rustls` or `aws-lc-rs`). It is a heavy transitive set (dozens of crates) but pure-Rust and widely used on stable.
- **[ARX DESIGN DECISION — SDK/MSRV FINDING (S3-AUDIT-23)]** Research result: the **latest** `aws-sdk-s3` (**1.141.0**, 2026-08-06) and `aws-config` (**1.10.1**) declare **MSRV 1.94.1**, which is **above** ARX's current `rust-version = "1.88"`. The smithy-rs changelog shows the SDK MSRV was raised *to* 1.88.0 at an earlier release, so a **pinned older SDK release that still targets 1.88 exists** and is the recommended path.
  - **Recommended path:** **(B) pin an older `aws-sdk-s3` / `aws-config` release** whose MSRV ≤ 1.88, keeping ARX's MSRV contract intact. The exact pinned version must be selected during `S3-01` by checking the SDK release notes for the last version before the 1.94.1 bump.
  - **Fallbacks if no suitable pin exists:** (A) raise ARX MSRV to ≥ 1.94.1 (breaks the current 1.88 release contract — needs product sign-off), or (C) use a lighter maintained crate (`rust-s3`/`s3`) that may have a lower MSRV (evaluate during `S3-01`).
  - **Decision deferred to `S3-01`:** concrete version pin. **No dependency is added in this audit.**
- **[ARX DESIGN DECISION]** Whatever the choice, it must support: `ListObjectsV2`, multipart upload (`Create/Upload/Complete/Abort`), `GetObject` range reads, `DeleteObject`, `CopyObject`, custom `endpoint_url` + `force_path_style`, and the AWS credential provider chain (env, shared files, profile, session token, instance metadata, web identity).

## 26. AWS / MinIO compatibility

| Provider | Classification | Notes |
|---|---|---|
| AWS S3 | **TARGET MVP** | Primary acceptance environment |
| MinIO | **TARGET MVP** | Path-style addressing; run a local/container MinIO for real tests |
| Cloudflare R2 | BEST-EFFORT / follow-up | S3-compatible; test after MVP if desired |
| Wasabi | BEST-EFFORT / follow-up | S3-compatible; test after MVP if desired |
| Other generic S3 | NOT TESTED | accepted via `endpoint_url` but unverified |

- **[ARX DESIGN DECISION]** Do **not** claim compatibility merely because the SDK accepts `endpoint_url`. Real acceptance environments: at minimum **AWS S3** + **MinIO** (locally deployed). R2/Wasabi remain compatibility follow-ups unless explicitly tested.
- **[ARX DESIGN DECISION]** Acceptance matrix (future test cards) must cover: list buckets, prefix navigation, >1000-object pagination, Unicode key, zero-byte object, folder marker, F3 Range GET, small upload, multipart upload, download, cancel multipart, single delete, access denied, wrong region, session token; plus MinIO path-style connect/list/upload/download/delete/Unicode; plus failure cases (network break, expired credentials, permission denied, bucket missing, object disappears mid-op, multipart abort failure). **No test requires production buckets.**

## 27. Security invariants

- **[ARX DESIGN DECISION]** ARX never persists AWS secrets. Credentials resolved exclusively via the SDK provider chain (§8).
- **[ARX DESIGN DECISION]** Never log: secret access key, session token, or signed-URL query credentials. Any diagnostic/error path that could echo a request must redact `X-Amz-*` query params and auth headers.
- **[ARX DESIGN DECISION]** `force_path_style` and `endpoint_url` are config-only and never contain embedded credentials.
- **[ARX DESIGN DECISION]** Destructive operations (delete, overwrite) require explicit confirmation with factual, non-invented consequences (§18).

## 28. MVP scope

**S3 MVP-1 (recommended first slice):**
1. S3 target configuration (`[[s3.targets]]`)
2. Standard AWS credential provider chain
3. Target/bucket/prefix navigation (mode A default, B via pinned bucket)
4. Paginated, bounded, asynchronous `ListObjectsV2` listing
5. F3 bounded text preview (`GetObject` Range)
6. Local → S3 upload (single + multipart)
7. S3 → Local download (staged + verified + atomic commit)
8. Multipart + progress + cancellation with abort
9. F7 prefix-marker creation (opt-in)
10. F8 single-object permanent delete (factual confirmation)
11. AWS + MinIO acceptance

**Explicitly excluded from MVP-1:** S3 F4 edit, F6 move, directory/prefix recursive delete, bucket create/delete, S3→S3, SFTP↔S3, Workspace Sync, Mirror.

**S3 MVP-2:** Workspace Compare (Local ↔ S3), Local→S3 Update Preview, Update execution, Verification.

**S3 MVP-3:** conditional S3 F4 (provider-neutral revision), server-side copy / rename transaction, advanced version-aware delete.

## 29. Future Workspace Sync

- **[ARX CURRENT FACT]** `WorkspaceSync` already compares `WorkspaceFingerprint`s across two `Location`s and produces `WorkspaceDiff` with `DiffState` (Same/OnlyLeft/OnlyRight/LeftNewer/RightNewer/Different). `WorkspaceSyncPlan` + frozen plan + verification already exist for Local↔SFTP.
- **[ARX DESIGN DECISION]** S3 enters Workspace Compare in MVP-2 by supplying `WorkspaceFingerprint`s where `size`/`modified_unix_ms` are reliable and `content_hash` is set **only** from a genuine S3 checksum (§20). Until then, comparison must not claim content equality on ETag alone.
- **[ARX DESIGN DECISION]** S3→S3 or SFTP↔S3 synchronization remains out of scope until transactional cross-backend copy (copy→verify→delete-source) is built; the planner already refuses such plans today.

## 30. Implementation sequence (future cards, no code)

```
S3-01  AWS SDK / MSRV spike (disposable worktree only)
   ↓
S3-02  config: [[s3.targets]] model + loader
   ↓
S3-03  Location::S3 + ProviderInstanceKey::S3Target + registry routing
   ↓
S3-04  listing (ListObjectsV2, delimiter, pagination)
   ↓
S3-05  navigation / UI (pane title, child/parent, "..")
   ↓
S3-06  bounded read / F3 preview (Range GET)
   ↓
S3-07  TransferPlanner: TransferMethod::S3 + ExecutorAvailability.s3
   ↓
S3-08  upload executor (PutObject + multipart)
   ↓
S3-09  download executor (staged + verified + commit)
   ↓
S3-10  Job integration + cancellation/abort states
   ↓
S3-11  transfer verification (size/checksum)
   ↓
S3-12  prefix-marker creation (F7)
   ↓
S3-13  single-object delete (F8) + factual confirmation
   ↓
S3-14  AWS + MinIO real acceptance
```

For every future card: specify dependencies, files likely touched (`src/vfs/s3.rs`, `src/vfs/mod.rs`, `src/transfer/*`, `src/app/availability.rs`, `src/config.rs`), the safety invariant, the test gate, and an explicit **STOP point** (no scope creep into the next card).

## 31. Open design questions

1. **[OPEN QUESTION]** Exact `aws-sdk-s3` version + MSRV fit for Rust 1.88 (S3-AUDIT-23) — confirm before S3-01.
2. **[OPEN QUESTION]** Should `force_path_style` default per-target or be auto-detected from `endpoint_url`? (Proposed: explicit per-target config.)
3. **[OPEN QUESTION]** Should MVP-1 expose `F7` prefix creation by default or behind an opt-in capability? (Proposed: opt-in, off until proven.)
4. **[OPEN QUESTION]** For versioned buckets, should MVP-1 surface version IDs in delete confirmation, or only the versioning *state*? (Proposed: state only; IDs in MVP-3.)
5. **[OPEN QUESTION]** Region resolution: per-target `region` config vs SDK `region` auto-resolution. (Proposed: explicit per-target `region`, since wrong region is a common failure.)

---

*Audit baseline — ARX `origin/main` = `bbf6313` (v0.15.1). WinSCP reference: upstream master at audit time (behavioral reference only; GPLv3 source not read). No implementation performed.*
