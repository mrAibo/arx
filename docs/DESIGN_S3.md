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
    target: String,       // matches [[s3.targets]].id
    bucket: Option<String>, // None at target root (ListBuckets); Some at bucket root+
    prefix: String,       // "" at bucket root; internal nav prefix, stored without leading/trailing slash as a UI/navigation convention only — this is NOT normalization of any S3 object key (see §9 KEY vs NAVIGATION PREFIX)
}
```

- **[ARX DESIGN DECISION]** `bucket` is `Option<String>`, **not** an empty-string sentinel. `None` = target root (ListBuckets); `Some(name)` = bucket root or deeper. This avoids the `""`-vs-`bucket` ambiguity the prior draft used.
- **[ARX DESIGN DECISION — EXACT LISTED-OBJECT IDENTITY (S3-DESIGN-FINAL-02)]** The generic `Entry { name, kind, size, modified_unix_ms }` is **presentation + listing metadata only**. ARX must never reconstruct an S3 operation target from `parent location + Entry.name` — unsafe for opaque keys (`foo/../bar`, `foo//bar`). Every listed S3 object/prefix carries an **exact provider-native reference**:

```rust
// conceptual only — no code change in this audit
struct ListedEntry {
    entry: Entry,            // presentation + metadata (name, kind, size, mtime)
    identity: EntryIdentity, // exact operational reference
}

enum EntryIdentity {
    S3Object(S3ObjectRef),
    S3Prefix(S3PrefixRef),
    S3Bucket(S3BucketRef),
    Other, // Local/SFTP/Archive keep their existing identity model
}

struct S3ObjectRef {
    target: String, // [[s3.targets]].id
    bucket: String,
    key: String,    // stored EXACTLY as returned by S3; never normalized
}

struct S3PrefixRef {
    target: String,
    bucket: String,
    prefix: String, // exact listing/navigation prefix, e.g. "foo/bar"
}

struct S3BucketRef {
    target: String,
    bucket: String, // exact bucket name as returned by ListBuckets
}
```

  - **Required invariant**: `Entry.name` is **presentation only** and **MUST NEVER be the authority** for S3 object operations. The `*Ref` is the authority. This holds for **bucket entries too**: a bucket's `Entry.name` is presentation only; entering a bucket from the target root MUST use `S3BucketRef`, never `target + Entry.name` reconstruction.
  - **Target-root flow (ListBuckets)**: `ListBuckets` → `ListedEntry { entry: Entry (presentation), identity: EntryIdentity::S3Bucket(S3BucketRef) }`. Entering a bucket → `Location::S3 { target, bucket: Some(exact bucket from ref), prefix: "" }`. No F3/F5/F8 semantics apply to bucket entries in MVP; F7 at target root remains disabled; bucket create/delete remain out of MVP.
  - **F3 (preview)**: focused `ListedEntry` → `S3ObjectRef` → `GetObject` `Range` on the **exact key**.
  - **F5 (download)**: `S3ObjectRef` → `TransferPlan` source identity (not a reconstructed path).
  - **F8 (delete)**: confirmation **freezes the exact** `target + bucket + key`; never re-derived from `parent + display name`.
  - **Navigation**: a `CommonPrefixes` entry becomes an `S3PrefixRef` (exact navigation prefix). The virtual `..` is a **navigation operation only**, never an S3 object key.
  - **Awkward-key test model** (must be preserved exactly): `foo//bar`, `foo/../bar`, `foo/./bar`, Unicode names, and the folder marker `foo/` — display may be abbreviated, but the stored `key`/`prefix` remains the exact server value. ARX never silently retargets an awkward key to a different object.
  - **Current helpers**: `validated_child_path()` and generic `Location::child(name)` **must not** be used to reconstruct existing S3 object identities (they remain valid for Local/SFTP). Newly created S3 prefix/object names use an S3-specific exact-key construction rule (see §17 for prefix markers).
- **[ARX DESIGN DECISION — KEY vs NAVIGATION PREFIX (S3-DESIGN-FINAL-03)]** Distinguish two concepts explicitly:
  - **`S3ObjectKey`**: the exact, opaque object key as returned by S3 (`S3ObjectRef.key`). Never normalized, never FS-interpreted.
  - **`S3NavigationPrefix`**: the prefix ARX uses for `ListObjectsV2` (`prefix=`) and for `..`/enter navigation (`S3PrefixRef.prefix`). A navigation prefix may have a canonical UI representation (e.g. trailing-slash display).
  - Storing a navigation prefix **without** a trailing slash is a UI/navigation representation choice — it does **not** normalize or modify any existing object key. If `ListObjectsV2` requires `foo/bar/` (trailing slash) while ARX stores `foo/bar` internally, that slash is **protocol/navigation construction**, not a change to object identity. The object key stays exactly what S3 returned.

## 10. Provider instance model

- **[ARX CURRENT FACT]** `ProviderInstanceKey` already distinguishes `Singleton(ProviderId)`, `SftpHost(String)`, `ArchiveFile(PathBuf)`. Capabilities are keyed by `ProviderId`; instances by the concrete resource.
- **[ARX DESIGN DECISION]** Add `ProviderInstanceKey::S3Target(String)` keyed by target `id`. This enables **multiple simultaneous targets** (AWS prod + AWS backups + MinIO lab) with no singleton `ProviderId::S3` routing.
- **[ARX DESIGN DECISION]** The `S3Provider` instance is created lazily per target `id` from the resolved config + SDK chain, and registered under `S3Target(id)`.
- **[ARX DESIGN DECISION]** UI identity: pane title shows `[S3 artifacts]` or `[S3 minio-lab]` — **never** credentials or endpoint secrets.

## 11. Listing and pagination

- **[ARX DESIGN DECISION]** Listing uses `ListObjectsV2` with `delimiter="/"`:
  - `Contents` → `Entry { kind: File, size, modified_unix_ms: LastModified }`
  - `CommonPrefixes` → `Entry { kind: Directory, size: None }` (virtual folder; no probe of child count)
  - At the **target root**, the listing operation is `ListBuckets` (API: `ListBuckets`; IAM action `s3:ListAllMyBuckets`), producing `EntryIdentity::S3Bucket` entries (see §9). Bucket entries are not paginated by `prefix`/`delimiter`; they use the same continuation model below.
- **[ARX DESIGN DECISION — TWO-LAYER PAGINATION (S3-DESIGN-FINAL-01)]** Provider pagination and pane correlation are **separate layers**. The `VfsProvider`/`S3Provider` must **not** depend on `PaneLoadId`, `Pane`, or `AppState`.
- **[ARX DESIGN DECISION — PAGINATION COVERS BOTH ROOT AND PREFIX (S3-DESIGN-AF-02)]** The `ProviderListingPage` / `ProviderContinuation` model applies to **both** listing operations; the provider continuation stays **opaque** (ARX never encodes an API-specific token type into `PaneLoader`):
  - **Target root** (`ListBuckets`): continuation is the `ListBuckets` `ContinuationToken`.
  - **Bucket/prefix** (`ListObjectsV2`): continuation is `NextContinuationToken`.
  All existing stale-generation rules (§11 pane layer) apply unchanged to either operation.

**Provider layer (owns only provider-native state):**

```rust
// conceptual only — no code change in this audit
struct ProviderListingPage {
    entries: Vec<ListedEntry>,           // see §9 identity model
    continuation: Option<ProviderContinuation>,
}

struct ProviderContinuation {
    // Opaque, provider-native only. For S3: exactly the server's
    // NextContinuationToken, verbatim. No pane/UI state lives here.
    token: String,
}
```

**Registry / pane layer (owns correlation + staleness):**

```rust
// conceptual only — no code change in this audit
struct PaneListingContinuation {
    provider_continuation: ProviderContinuation, // opaque token from provider
    provider_instance: ProviderInstanceKey,       // e.g. S3Target(id)
    location: Location,                           // exact bucket + prefix
    generation: PaneLoadId,                       // owned by PaneLoader/AppState
}
```

**Flow:**
  - **First page**: `PaneLoader` allocates a `generation` → `registry.list_page(location, None)` → provider returns a `ProviderListingPage` with a `ProviderContinuation` → `PaneLoader`/`AppState` wraps it into a `PaneListingContinuation` adding `provider_instance` + exact `Location` + `generation`.
  - **Next page**: `PaneLoader` verifies the current `generation`/`location` still match → unwraps the `provider_continuation.token` → `registry`/`provider` fetches the next page → the result is accepted **only if** the same `generation`/`location` hold; otherwise it is discarded as stale.
  - **Refresh** / **Navigation** (enter `..`, open a prefix, switch pane/target): opens a **new** `generation`; all prior `PaneListingContinuation`s become stale by construction.
  - **Cancellation**: a cancelled load/load-next drops its continuation; no background fetch resumes it.
  - **End-of-list**: `continuation == None` → UI shows no "Load more"; repeated requests are no-ops.

**NEVER:** `S3Provider` → `PaneLoadId`; `VfsProvider` → `Pane`; `VfsProvider` → `AppState`.

- **[ARX CURRENT FACT]** The staleness guard already exists for single loads: `AppState::accepts_pane_load` (src/app/mod.rs) rejects a `PaneLoadResponse` unless its `PaneLoadId` is still the pending one **and** its `Location` still matches the pane target (with an extra committed-location check for `Refresh`). The S3 pagination model extends this same generation+location binding to *each page* at the **pane layer** (`PaneListingContinuation`), so a slow/late page from a previous navigation can never append to the current view.
- **[ARX DESIGN DECISION — PROTOCOL + STALE SAFETY]** Two distinct failure classes (apply to **both** `ListBuckets` and `ListObjectsV2`):
  - **ProtocolError**: a provider returns a truncated/has-more response but `ProviderContinuation` is `None`, or returns a `token` identical to the one already consumed (non-advancing). Pagination stops with a factual message; ARX never re-requests the same token. **No infinite-loop.**
  - **Stale discard**: a `PaneListingContinuation` whose `generation`/`location`/`provider_instance` no longer matches the current pane is dropped silently (no append).
- **[ARX DESIGN DECISION]** Never block the TUI awaiting a full million-object enumeration. A page cap is the unit of work; `IsTruncated` + `NextContinuationToken` drive continuation.
- **[WINSCP OBSERVATION]** WinSCP handles truncated responses and `NextMarker`; ARX adopts the continuation concept but keeps it incremental rather than eager, and binds every continuation to its listing generation.
- **[AWS/S3 FACT]** Edge cases to handle explicitly: zero keys returned, `MaxKeys` present but empty, provider returning `CommonPrefixes` without `Contents`, **Unicode keys** (characters requiring safe API/XML/URL encoding, control-character representation, percent-encoding where the SDK/API requires it). ARX must **never** hand-normalize or decode a key into a different key; the AWS SDK owns wire encoding where possible.

## 12. Root / bucket / prefix navigation (DECISION)

Three candidate root modes:

- **A. Target root → list buckets** (each configured target id lists its accessible buckets).
- **B. Each target bound to one bucket; root = bucket root.**
- **C. Support both.**

**Comparison:**

| Mode | UX | IAM implication |
|---|---|---|
| A | One S3 entry point shows all buckets for the credential | Needs `s3:ListAllMyBuckets` (IAM action for the `ListBuckets` API) + per-bucket access; broader IAM scope |
| B | Simpler, least-privilege per target | Tightest IAM; one bucket per config block |
| C | Most flexible, most complex | Mixed |

- **[ARX DESIGN DECISION]** **Default to A** (matches WinSCP's bucket-list-at-root UX and ARX's multi-target intent), but allow a target config to pin `bucket = "..."` which collapses directly to bucket root (mode B for that target). Mode C is thus achieved without a third code path.
  - **Explicit root behavior (no empty-string sentinel):** the `bucket: Option<String>` in `Location::S3` drives this directly:
    - **Target without bucket** (`bucket == None`) → `ListBuckets` (mode A). Pane shows the accessible buckets for the credential.
    - **Target with bucket** (`bucket == Some(name)`) → opens that bucket's root directly (mode B); no bucket-list step is shown.
  - **[ARX DESIGN DECISION — IAM SCOPE (S3-DESIGN-AF-03)]** Distinguish the API operation (`ListBuckets`) from its IAM policy action (`s3:ListAllMyBuckets`). A **bucket-bound target** (mode B) opens the configured bucket directly via `ListObjectsV2` and MUST NOT require `s3:ListAllMyBuckets` (nor `ListBuckets`) merely to open that one bucket. Only target-root mode (mode A) needs `s3:ListAllMyBuckets`. This keeps mode-B targets working under tightest-per-bucket IAM.
  - This is the same `Option<String>` distinction already specified in §9, applied consistently at the navigation layer.
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
| `Copy` | **NO** | **NO** | **NO** |
| `Move` | NO | NO | transaction-gated |
| `Rename` | NO | NO | transaction-gated |
| `ServerSideCopy` | NO | NO | later (real `CopyObject`) |
| `Symlink`/`Chmod` | NO | NO | NO (no POSIX semantics) |

- **[ARX DESIGN DECISION — CORRECTED `Copy` SEMANTICS]** `Capability::Copy` does **NOT** mean "F5 Local↔S3 transfer is available." SFTP already proves the distinction: SFTP's `Copy` capability is **off**, yet `Action::Copy` works for `Local↔SFTP` because the **TransferPlanner** provides a safe route (`TransferMethod::Sftp`) + a real executor. S3 follows the same rule. Therefore `Copy` stays **NO** across all MVP phases for S3.
  - **Local↔S3 F5 availability** is derived from: a safe `TransferPlanner` route (`TransferMethod::S3`) **+** an available S3 executor — **not** from `Capability::Copy`. Concretely, `action_availability(ActionId::Copy)` must check `TransferMethod::S3` reachability (one side `Local`, other side `S3` with `Capability::Write`), exactly as it currently checks `Local|Sftp` reachability.
  - **`ServerSideCopy`** turns on **only** when a real `CopyObject` implementation exists (S3→S3 or S3 intra-bucket copy). Until then it stays NO, even though transfer-based Local↔S3 copy already works.
  - This preserves the ARX invariant: *capability == implemented promise* — a capability bit is an in-provider primitive, not a cross-provider transfer route.

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
- **[ARX DESIGN DECISION — SDK/MSRV FINDING (S3-AUDIT-23)]** Research result: the **latest** `aws-sdk-s3` (**1.141.0**, 2026-08-06) and `aws-config` (**1.10.1**) declare **MSRV 1.94.1**, which is **above** ARX's current `rust-version = "1.88"`. The smithy-rs MSRV timeline: 2025-10-30 → 1.88.0 (smithy-rs#4367); 2026-02-10 → 1.91.0; 2026-07-07 → 1.94.1 (smithy-rs#4692). So a **pinned older SDK release that still targets 1.88 exists**.
  - **Exact pin contract (authoritative for `S3-01`):**

```toml
# Cargo.toml — exact-version pins, MUST use `=`
aws-sdk-s3 = "=1.109.0"
aws-config  = "=1.8.8"
```

    `Cargo.lock` is committed and **all** CI/builds use `cargo … --locked` so the resolved graph is reproducible and the MSRV contract is enforced.
  - **Disposable MSRV experiment evidence (record from `S3-01`, run in a throwaway worktree/branch only — never in the audit PR):**

```
Rust toolchain: 1.88
Exact top-level crates:
  aws-sdk-s3     = "=1.109.0"
  aws-config     = "=1.8.8"
Resolved dependency graph: cargo tree (committed Cargo.lock, ~35 normal deps)
cargo +1.88 check (--locked, release-2025-10-29 line): PASS
```

    `S3-01` must **repeat this experiment** and attach the actual `cargo +1.88 check` output (or CI log) before any production dependency commit. If a later patch release within 1.109.x/1.8.8 changes MSRV, re-pin to the highest patch that still builds on 1.88.
  - **Transitive burden:** ~35 normal deps (mostly `aws-smithy-*` internals); external heavyweights are `tokio`, `hyper`, `rustls` + `aws-lc-rs`/`ring` (native crypto), `sha2`/`hmac`. No C/C++ toolchain beyond a standard Rust build. Acceptable but heavy — justify the dependency only when `S3-01` actually starts.
  - **Fallbacks:** (A) raise ARX MSRV to ≥ 1.94.1 (breaks the 1.88 release contract — needs product sign-off), or (C) use a lighter maintained crate (`rust-s3`/`s3`) with possibly lower MSRV but incomplete S3 API and weaker credential chain (evaluate at `S3-01`).
  - **Verdict: YES, SDK compiles on 1.88 via pinned `=1.109.0` / `=1.8.8`.** No dependency is added in this audit.
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
- **[ARX DESIGN DECISION — PAGINATION SAFETY INVARIANT (provider-neutral, no unverified folklore)]** Listing correctness does **not** depend on any specific `max-keys` numeric trick (e.g. "avoid multiples of 8"). The contract is driven by the protocol, not by guesswork:
  - A truncated page (`IsTruncated == true`) **must** carry a `continuation` token, and loading the next page **must** advance past the current one.
  - A truncated page **without** a usable continuation token, or a continuation token that repeats (does not advance), is a **ProtocolError** → pagination stops with a factual message. **No infinite pagination loops.**
  - `max-keys` is chosen as a sensible page size (e.g. 1000); the value itself carries no correctness magic. Correctness comes from the `continuation`-advances invariant above, which is already enforced by the §11 continuation model.
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

1. **[ARX DESIGN DECISION — RESOLVED]** SDK/MSRV (S3-AUDIT-23): pin `aws-sdk-s3 = "=1.109.0"` + `aws-config = "=1.8.8"` (smithy-rs `release-2025-10-29` line); builds on Rust 1.88 via `--locked`. Re-verified by `S3-01` (see §25).
2. **[OPEN QUESTION]** Should `force_path_style` default per-target or be auto-detected from `endpoint_url`? (Proposed: explicit per-target config.)
3. **[OPEN QUESTION]** Should MVP-1 expose `F7` prefix creation by default or behind an opt-in capability? (Proposed: opt-in, off until proven.)
4. **[OPEN QUESTION]** For versioned buckets, should MVP-1 surface version IDs in delete confirmation, or only the versioning *state*? (Proposed: state only; IDs in MVP-3.)
5. **[OPEN QUESTION]** Region resolution: per-target `region` config vs SDK `region` auto-resolution. (Proposed: explicit per-target `region`, since wrong region is a common failure.)

---

*Audit baseline — ARX `origin/main` = `bbf6313` (v0.15.1). WinSCP reference: upstream master at audit time (behavioral reference only; GPLv3 source not read). No implementation performed.*
