# ARX S3 MVP — Kanban

> Single-card execution. One card in DOING. Architecture authority: `docs/DESIGN_S3.md`.

## Authority

- **Architecture authority:** `docs/DESIGN_S3.md`
- **`S3_KANBAN.md` is execution/orchestration guidance only** and **cannot override `DESIGN_S3.md`**.
  If a card instruction appears to disagree with `DESIGN_S3.md`, stop and treat
  `DESIGN_S3.md` as the source of truth — do not reinterpret the design.

> If card instructions disagree with DESIGN_S3.md → STOP, do not reinterpret.

> Orchestrator gate every 3–6 cards.


## Column summary

- **READY** (1): S3-31
- **DOING** (0): —
- **REVIEW** (0): —
- **BLOCKED** (0): —
- **DONE** (36): S3-00..S3-25, S3-24P (PR #98 merged, merge SHA 19d4c04), S3-20R (PR #99 merged, merge SHA 2e7fe21), S3-24 (PR #100 merged, merge SHA f38273a), S3-25 (PR #101 merged, merge SHA 85afb6a), S3-26A (PR #102 merged, merge SHA 6967ba2; STOP GATE C PASS), S3-27R (PR #105 merged, merge SHA 1568e35), S3-26 (PR #104 merged, merge SHA 9c27a84), S3-27 (PR #106 merged, merge SHA a49b94b), S3-28 (PR #107 merged, merge SHA 1fd6bec), S3-29 (PR #108 merged, merge SHA 242dff6), S3-30 (PR #109 merged, merge SHA d74fb55; P11 READ/F3 GATE PASS)
- **BACKLOG** (49): S3-32..S3-80
- **PARKED** (13): S3-81, S3-82, S3-83, S3-84, S3-85, S3-90, S3-91, S3-92, S3-93, S3-94, S3-95, S3-96, S3-97



---


## READY

### S3-00 — Merge approved architecture
- **Phase:** P0
- **Status:** DONE
- **Depends on:** PR #64 approved; HEAD 2531d10
- **Allowed files:** docs/DESIGN_S3.md (only, before merge)
- **Acceptance:** 1) Apply only approved NIT wording (ProviderContinuation generic for ListBuckets/ListObjectsV2; Location::S3 prefix wording must not imply normalization). 2) No arch changes. 3) quality SUCCESS + msrv SUCCESS + mergeable. 4) Merge PR #64. 5) Fetch main, record merge SHA.
- **Stop conditions:** Any architecture change. Any capability flip. Any Rust.
- **Hermes prompt:** Finish design phase: apply ONLY the two approved NIT wording fixes to docs/DESIGN_S3.md (ProviderContinuation generic; prefix wording non-normalizing). No arch changes, no Rust. Verify exact-head quality+msrv SUCCESS and mergeable. Mark PR #64 Ready, then merge. Fetch main, record MAIN_SHA and PR64_MERGE_SHA. STOP after merge.

### S3-01 — AWS SDK MSRV disposable spike
- **Phase:** P1
- **Status:** DONE
- **Depends on:** S3-00 merged
- **Allowed files:** DISPOSABLE branch/worktree only (no production merge)
- **Acceptance:** cargo generate-lockfile; cargo +1.88 check --locked; --all-features; cargo +1.88 test --locked; cargo +1.88 build --locked; cargo tree. Record resolved aws-*/aws-smithy*/tokio/hyper/rustls/aws-lc/ring. PASS/FAIL + graph + dep count + build-time delta + binary-size delta.
- **Stop conditions:** Any resolved crate requires Rust >1.88 (STOP, FAIL).
- **Hermes prompt:** Disposable spike: add aws-sdk-s3="=1.109.0" aws-config="=1.8.8" to a throwaway branch. Run cargo generate-lockfile, cargo +1.88 check --locked, --all-features, test --locked, build --locked, cargo tree. Record exact resolved graph + counts. STOP if ANY crate needs Rust>1.88. No production merge.

### S3-02 — SDK API compile probe
- **Phase:** P1
- **Status:** DONE
- **Depends on:** S3-01 PASS
- **Allowed files:** Throwaway code only
- **Acceptance:** Compile references for every API in spec (list_buckets, max_buckets, continuation_token, list_objects_v2, get/put/head/delete_object, create/upload/complete/abort multipart, endpoint_url, force_path_style, AWS profile loading). PASS = compiler proves surface. No real AWS calls.
- **Stop conditions:** Real network/AWS calls. Production code.
- **Hermes prompt:** Throwaway compile-probe: reference (don't call) every SDK API required by DESIGN_S3.md (list_buckets, list_objects_v2 w/ prefix/delimiter/continuation, get_object Range, put/head/delete_object, multipart 4 ops, endpoint_url, force_path_style, profile loading). PASS = it compiles. No AWS calls, no production code. STOP after report.

### S3-03 — AWS retry ownership audit
- **Phase:** P1
- **Status:** DONE
- **Depends on:** S3-01 PASS
- **Allowed files:** No production code
- **Acceptance:** Inspect pinned SDK RetryConfig: default max attempts, retryable transport/service errors, backoff. Choose A (disable/minimize SDK retries, ARX owns) or B (narrow SDK transport retries w/ specified ops/max/cancellation/Job truth). Return recommended retry policy.
- **Stop conditions:** Production code. Making real calls.
- **Hermes prompt:** Audit pinned SDK RetryConfig (aws-config/aws-sdk-s3 1.109.0/1.8.8). Determine defaults. Decide A vs B. Key invariant: ARX must never report one physical attempt when SDK may perform materially different repeated mutations (audit PutObject/UploadPart/CompleteMultipartUpload/DeleteObject). Return recommended policy. No production code.


## BACKLOG

### S3-04 — Add exact AWS dependencies
- **Phase:** P2
- **Status:** DONE
- **Depends on:** STOP GATE A (S3-01 PASS, S3-02 PASS, S3-03 accepted)
- **Allowed files:** Cargo.toml, Cargo.lock
- **Acceptance:** Add aws-sdk-s3="=1.109.0" aws-config="=1.8.8". cargo +1.88 check --locked --all-features; cargo test --locked --all-features; cargo clippy --all-targets --all-features -- -D warnings. Cargo.lock contains expected approved graph. No S3 code.
- **Stop conditions:** Adding S3 code. Any other dep. Broadening scope.
- **Hermes prompt:** Add approved SDK deps to Cargo.toml (exact =1.109.0 / =1.8.8) + Cargo.lock. Run check/test/clippy --locked --all-features. Verify resolved graph matches S3-01. No S3 code. Prefer one dependency-only PR.

### S3-05 — S3 target config types
- **Phase:** P3
- **Status:** DONE
- **Depends on:** S3-04 merged
- **Allowed files:** src/config.rs (or src/config/s3.rs)
- **Acceptance:** Implement S3TargetConfig data model: id, name, bucket: Option<String>, region: Option<String>, profile: Option<String>, endpoint_url: Option<String>, force_path_style: bool. No creds/secrets, no AWS client, no TUI. Unit tests: minimal, bucket-bound, MinIO-style, multiple targets.
- **Stop conditions:** AWS client. Credential storage. TUI.
- **Hermes prompt:** Implement only the S3 target config DATA MODEL (no secrets, no client, no TUI). Unit-test minimal / bucket-bound / MinIO-style / multiple-targets. Authority: DESIGN_S3 §9/§10.

### S3-06 — S3 config parsing
- **Phase:** P3
- **Status:** DONE
- **Depends on:** S3-05
- **Allowed files:** src/config.rs
- **Acceptance:** Parse S3 targets from ARX config using S3-05 types. Reject duplicate IDs, empty id, empty bucket if Some(""), invalid endpoint if existing layer validates URLs. Tests: valid AWS, bucket-bound, MinIO, duplicate ID, multiple targets. No credential tests, no SDK clients.
- **Stop conditions:** Testing credentials. Creating SDK clients.
- **Hermes prompt:** Parse S3 target definitions from ARX config via S3-05 types. Reject: duplicate id, empty id, Some("") bucket, bad endpoint (if existing validation exists). Tests: valid AWS / bucket-bound / MinIO / dup id / multi. No creds, no clients.

### S3-07 — Config redaction truth
- **Phase:** P3
- **Status:** DONE
- **Correction:** S3-07R (PR #75) redacted `endpoint_url` from `Debug` output — manual `impl Debug` renders `Some("<configured>")`/`None`; storage verbatim. Closes a `Debug` leak MAJOR found post-merge.
- **Depends on:** S3-05, S3-06
- **Allowed files:** src/config.rs
- **Acceptance:** Audit Debug/Display/config-error for S3 target output. Allow: id, bucket, region, endpoint host. Never: AWS secret, session token, signed-URL query creds. Tests where practical. No credential storage added.
- **Stop conditions:** Adding credential storage.
- **Hermes prompt:** Audit S3 config/log/Debug/Display/error output. Ensure only id/bucket/region/endpoint-host appear; never secret/token/signed-query. No new credential storage. Tests where practical.

### S3-08 — ProviderInstanceKey::S3Target
- **Phase:** P4
- **Status:** DONE
- **Depends on:** S3-04
- **Allowed files:** src/vfs/mod.rs
- **Acceptance:** Add ProviderInstanceKey::S3Target(String). No registry client impl. Tests: two target IDs !=, S3 target != SFTP host, hash/map identity stable.
- **Stop conditions:** Registry client implementation.
- **Hermes prompt:** Add ProviderInstanceKey::S3Target(target_id). No client/registry impl. Tests: distinct ids differ, S3Target != SftpHost, map/hash identity stable.

### S3-09 — Location::S3
- **Phase:** P4
- **Status:** DONE
- **Depends on:** S3-08
- **Allowed files:** src/vfs/mod.rs
- **Acceptance:** Add Location::S3{target, bucket: Option<String>, prefix}. bucket None=>target root; Some+empty prefix=>bucket root. Do NOT fs-normalize prefix. Tests: target root, bucket root, nested prefix, Unicode prefix, awkward slash preservation.
- **Stop conditions:** AWS calls. Normalization.
- **Hermes prompt:** Add Location::S3{target, bucket: Option<String>, prefix}. Rules: None=bucket list root, Some+empty=bucket root. No fs-normalization. Tests: target root / bucket root / nested / Unicode / double-slash preserved.
- **Note:** S3-09R safety correction merged and approved.

### S3-10 — S3 display identity
- **Phase:** P4
- **Status:** DONE
- **Depends on:** S3-09
- **Allowed files:** src/vfs/mod.rs (Display)
- **Acceptance:** Render S3 locations truthfully: target root [S3 name]; bucket root s3://bucket/; prefix s3://bucket/prefix/. Never creds. No nav logic. Tests: Display output only.
- **Stop conditions:** Navigation logic changes.
- **Hermes prompt:** Implement Display for Location::S3 only. target root=>[S3 name]; bucket root=>s3://bucket/; prefix=>s3://bucket/prefix/. No creds, no nav logic. Tests: Display strings.

### S3-11 — S3 exact reference types
- **Phase:** P5
- **Status:** DONE
- **Depends on:** S3-09, S3-10
- **Allowed files:** src/vfs/s3.rs
- **Acceptance:** S3BucketRef{target, bucket}, S3ObjectRef{target, bucket, key}, S3PrefixRef{target, bucket, prefix}; all provider-native strings stored verbatim; no filesystem interpretation, no path normalization, no trimming, no canonicalization, no collapsing //, no resolving . or .., no trailing-slash removal, Unicode preserved exactly; tests pin the exact-identity invariant.
- **Stop conditions:** Any normalization of provider-native identity; AWS network/client code; capability changes; consumer migration; EntryIdentity implementation; listing implementation.
- **Hermes prompt:** Implement S3BucketRef/S3ObjectRef/S3PrefixRef exactly as DESIGN_S3 §9/§11 in src/vfs/s3.rs. No normalization. Tests preserve foo//bar, foo/../bar, foo/./bar, foo/, Unicode verbatim.

### S3-12 — ListedEntry / EntryIdentity core
- **Phase:** P5
- **Status:** DONE
- **Depends on:** S3-11
- **Allowed files:** src/vfs/mod.rs
- **Acceptance:** Add ListedEntry{entry, identity} and EntryIdentity variants S3Bucket(S3BucketRef)/S3Object(S3ObjectRef)/S3Prefix(S3PrefixRef) (+ safe Other for existing). Do NOT convert all consumers. Do NOT break Local/SFTP. Tests: presentation name != operational identity; awkward key preserved exactly.
- **Stop conditions:** Converting all consumers. Breaking Local/SFTP.
- **Hermes prompt:** Introduce ListedEntry{entry: Entry, identity: EntryIdentity} with S3Bucket/S3Object/S3Prefix (+Other for existing providers). Do NOT migrate all consumers; do NOT break Local/SFTP. Tests: name!=identity; awkward key exact.

### S3-13 — ProviderListingPage
- **Phase:** P6
- **Status:** DONE
- **Depends on:** S3-11, S3-12
- **Allowed files:** src/vfs/mod.rs
- **Acceptance:** Add ProviderListingPage{entries, continuation} + ProviderContinuation{opaque token}. No PaneLoadId. No S3 network yet. Existing providers use continuation=None. Tests: Local/SFTP listing unchanged.
- **Stop conditions:** PaneLoadId in provider. S3 network code.

### S3-14 — PaneListingContinuation
- **Phase:** P6
- **Status:** DONE
- **Depends on:** S3-13
- **Allowed files:** src/services/pane_loader.rs, src/services/mod.rs, src/app/mod.rs
- **Acceptance:** Add PaneListingContinuation{provider_continuation, provider_instance: ProviderInstanceKey, location: Location, generation: PaneLoadId}. PaneLoadId stays in pane_loader. Re-export from src/services/mod.rs. No S3 code, no pagination behavior.
- **Stop conditions:** S3 code. Pagination behavior (S3-15).

### S3-15 — Stale page rejection
- **Phase:** P6
- **Status:** DONE
- **Depends on:** S3-14
- **Allowed files:** src/app/mod.rs, docs/S3_KANBAN.md, src/services/pane_loader.rs (unchanged unless type placement needs it)
- **Acceptance:** Implement the pane-layer stale-continuation acceptance guard and generation lifecycle that make future paginated append safe. Add AppState.pane_listing_generations (per-pane persistent current listing generation); register_pane_load also advances it; finish_pane_load preserves it (only removes pending request state). Add accepts_pane_listing_continuation(pane, &continuation) -> bool requiring generation + exact location + concrete provider instance; stale = silent discard. Tests: current accept, finish-page1 keeps continuation valid, refresh invalidates old generation, navigation invalidates old, exact-location mismatch, provider-instance mismatch (independent), left/right pane isolation. No page fetching, no append, no S3 calls.
- **Stop conditions:** S3 calls. Pagination fetch/append (after S3 provider pagination exists).
- **Hermes prompt:** Implement the pane-layer stale-continuation acceptance guard + generation lifecycle (persistent per-pane listing generation advanced on register, preserved on finish). accepts_pane_listing_continuation checks generation + exact location + concrete ProviderInstanceKey; stale silently discarded. Tests for navigation, refresh, provider-instance mismatch, post-first-page continuation lifetime. No page fetching, no S3.

### S3-16 — AWS S3 Client Factory
- **Phase:** P7
- **Status:** DONE
- **Depends on:** STOP GATE B (S3-13..15 PASS, Local/SFTP no regress, no S3 caps)
- **Allowed files:** src/vfs/s3.rs
- **Acceptance:** Create AWS SDK client for one S3TargetConfig: region/profile/endpoint_url/force_path_style, standard credential chain, approved retry policy from S3-03. No list ops. Never log creds. Unit-test config translation. Endpoint security: reject embedded credentials (userinfo, X-Amz-* query) fail-closed.
- **Stop conditions:** List operations. Logging credentials.
- **Hermes prompt:** S3 client factory from S3TargetConfig (region/profile/endpoint_url/force_path_style, std credential chain, S3-03 retry policy). No list ops. Never log creds. Test config translation. Endpoint validation.
- **Orchestrator review:** efc397a — BLOCKER 0 / MAJOR 0 / NIT 1 (non-blocking, Debug-based endpoint tests). APPROVED. Merged.

### S3-17 — Client registry lifecycle
- **Phase:** P7
- **Status:** DONE
- **Depends on:** S3-16 (client_for_target factory), S3-06 (validate_s3), S3-13..15 (provider registry + guarded TUI)
- **Allowed files:** src/vfs/mod.rs, src/vfs/s3.rs, src/tui.rs, src/config.rs, docs/S3_KANBAN.md
- **Acceptance:** ProviderInstanceKey::S3Target => correct target config => correct lazy client. A!=B, separate endpoint/profile, no singleton S3 client. Startup registers target inventory only (no AWS load/network/client). Typed page route target-aware but S3 list_page stays Unsupported. No listing yet.
- **Stop conditions:** Singleton S3 client. Listing. AWS client at startup.
- **Orchestrator review:** 540ea1f — BLOCKER 0 / MAJOR 0 / NIT 2 / APPROVED. Merged. (NIT-1: client() map_err redundant; NIT-2: src/config.rs not in initial Allowed files — actual scope added it via pub(crate) sanitize_diag reuse; corrected here.)
- **Hermes prompt:** Wire S3Target(target_id) => target config => dedicated lazy client. A!=B, no singleton. No listing yet. Scope correction: add src/tui.rs (register_s3_targets startup wiring, no AWS calls).

### S3-18 — ListBuckets first page
- **Phase:** P8
- **Status:** DONE
- **Depends on:** S3-17
- **Allowed files:** src/vfs/s3.rs, src/vfs/mod.rs (tests only), docs/S3_KANBAN.md
- **Acceptance:** Target-root first page via ListBuckets. Map bucket=>ListedEntry{presentation=bucket name, identity=S3BucketRef}. No Entry.name reconstruction. No create/delete bucket. Tests w/ mocked SDK boundary.
- **Stop conditions:** Bucket create/delete. Entry.name reconstruction.
- **Hermes prompt:** Implement target-root first page: ListBuckets => ListedEntry with identity=S3BucketRef (presentation=bucket name only). No name reconstruction. No bucket create/delete. Mock SDK boundary in tests.

### S3-19 — ListBuckets pagination
- **Phase:** P8
- **Status:** DONE
- **Depends on:** S3-18
- **Allowed files:** src/vfs/s3.rs, docs/S3_KANBAN.md, docs/DESIGN_S3.md
- **Acceptance:** Incremental ListBuckets pagination via ProviderContinuation.
  - ProviderContinuation.token passed verbatim as ListBuckets ContinuationToken (no trim/decode/normalize).
  - Every request bounded by LIST_BUCKETS_PAGE_SIZE (1000).
  - Returned ContinuationToken == None => end-of-list.
  - Returned ContinuationToken == Some(token) => another page available.
  - Empty returned token => ProtocolError (InvalidData).
  - Returned token identical to consumed token => ProtocolError (non-advancing).
  - Empty consumed token => local InvalidData before client/AWS request.
  - Exactly one ListBuckets request per list_page invocation.
  - No loop / no eager account enumeration.
  - NO independent IsTruncated/has-more signal for ListBuckets — do not invent one.
- **Stop conditions:** Eager loop, paginator helper, full-account load, ListObjectsV2 (S3-20/21).
- **Hermes prompt:** Implement incremental ListBuckets pagination via exact ContinuationToken. None=end; advancing Some=next page; reject empty/non-advancing tokens. One bounded request per invocation. No IsTruncated/has-more signal, no eager loop.

### S3-20 — ListObjectsV2 first page
- **Phase:** P9
- **Status:** DONE
- **Depends on:** S3-17, S3-18, S3-19
- **Allowed files:** src/vfs/s3.rs, src/vfs/mod.rs (tests only), docs/S3_KANBAN.md
- **Acceptance:** One bounded ListObjectsV2 first-page request for Bucket scope.
  - Navigation prefix vs wire-prefix vs exact identity:
    - bucket-root nav "" => wire Prefix ""
    - non-empty nav without trailing "/" => append exactly one "/"
    - non-empty nav WITH trailing "/" => append exactly one more "/" (the existing trailing "/" is literal namespace structure, NOT a protocol delimiter to skip; see S3-20R)
    - no trim / no "//" collapse / no "." / ".." resolution / no canonicalize
  - Delimiter="/" groups child keys into CommonPrefixes.
  - max_keys=1000; exactly ONE ListObjectsV2 .send() per list_page.
  - Exact Contents => S3ObjectRef (key authoritative, never transformed).
  - Exact CommonPrefixes => S3PrefixRef (delimiter slash kept).
  - Presentation name = prefix-stripped for display only (never operational identity).
  - Folder-marker dedup: zero-byte self marker (key==wire prefix) suppressed; zero-byte child marker matched by exact CommonPrefix deduped. Non-zero slash objects preserved; unmatched zero-byte slash objects preserved as S3ObjectRef. No blanket ends_with('/') drop.
  - First-page IsTruncated / NextContinuationToken truth: false+no token=>end; true+token=>continuation; true+(missing|empty)=>InvalidData; false+token=>InvalidData (contradictory); missing IsTruncated=>InvalidData.
  - continuation input (Some) remains S3-21: Unsupported before client init.
  - ListBuckets / TargetRoot behavior unchanged; exact bound bucket initializes lazy client.
- **Stop conditions:** Normalization. Continuation input consumption (S3-21).
- **Hermes prompt:** ListObjectsV2 first page: wire prefix from nav (no normalization), Delimiter="/", max_keys=1000, one send. Contents=>S3ObjectRef, CommonPrefixes=>S3PrefixRef. Exact-evidence folder-marker dedup only. First-page IsTruncated/NextContinuationToken truth. Continuation input is S3-21.

### S3-21 — ListObjectsV2 pagination
- **Phase:** P9
- **Status:** DONE
- **Depends on:** S3-20
- **Allowed files:** src/vfs/s3.rs, docs/S3_KANBAN.md
- **Acceptance:**
  - Bucket-scope `ProviderContinuation.token` is passed verbatim as `ListObjectsV2` `ContinuationToken`.
  - No trim/decode/re-encode/normalization.
  - Empty consumed token => `InvalidData` before client/AWS.
  - Request retains exact bucket + wire prefix + `Delimiter="/"` + `max_keys=1000`.
  - Exactly one `ListObjectsV2` request per `list_page` invocation.
  - No paginator, eager loop, or `StartAfter`.
  - `IsTruncated=false` + no token => end.
  - `IsTruncated=false` + token => `InvalidData`.
  - `IsTruncated=true` requires non-empty `NextContinuationToken`.
  - Returned token identical to consumed token => `InvalidData`.
  - Advancing token preserved exactly.
  - Same `Contents`/`CommonPrefixes`/folder-marker mapper for every page.
  - Continuation token values never appear in diagnostics.
- **Stop conditions:** Eager loop.
- **Hermes prompt:** ListObjectsV2 pagination via NextContinuationToken. ProtocolError on missing/non-advancing token. No eager full-bucket loop.

### S3-22 — Awkward key listing tests
- **Phase:** P9
- **Status:** DONE
- **Depends on:** S3-20, S3-21
- **Allowed files:** src/vfs/s3.rs (TESTS ONLY), docs/S3_KANBAN.md
- **Acceptance:**
  - Test `ListObjectsV2` fixture mapping for awkward keys.
  - Exact `S3ObjectRef.key` / `S3PrefixRef.prefix` always remains the exact provider value.
  - `Entry.name` remains presentation-only.
  - No test may derive expected operational identity from `Entry.name`.
  - No `Path` / `PathBuf` / `components` / `canonicalize` / filesystem normalization.
  - No AWS client/network.
  - No production behavior modification.
  - Existing S3-20/S3-21 folder-marker and pagination tests stay green.
- **Stop conditions:** AWS integration required (must allow mocked).
- **Hermes prompt:** Tests: awkward keys (foo//bar, foo/../bar, foo/./bar, spaces, Unicode, emoji, zero-byte, folder marker) preserve exact identity; display may simplify presentation only. Mocked.

### S3-20R — Reversible S3 navigation/wire-prefix delimiter seam
- **Phase:** P9 correction / prerequisite for P10 prefix navigation
- **Status:** DONE (PR #99 merged; merge SHA pending; independent review APPROVED 0/0/0/0, quality+msrv SUCCESS)
- **Depends on:** S3-20, S3-21, S3-22
- **Allowed files:** src/vfs/s3.rs, docs/DESIGN_S3.md, docs/S3_KANBAN.md
- **Acceptance:**
  1. `list_objects_wire_prefix`: for every non-empty navigation prefix, append EXACTLY ONE `/` UNCONDITIONALLY — the `ends_with('/') => preserve` branch is removed.
  2. Round-trip invariant holds for repeated-delimiter structure: `nav(P) = P` with exactly one final `/` removed; `wire(nav(P)) == P`. Prove: `"foo/"`→`"foo"`, `"foo//"`→`"foo/"`→`"foo//"`; `"foo/bar/"`→`"foo/bar"`; `"foo/../bar/"`→`"foo/../bar"`; Unicode. No production nav helper introduced (test-only `strip_suffix('/')`).
  3. CommonPrefix mapping requires `prefix.ends_with('/')` (Delimiter="/" always terminates a returned CommonPrefix). Missing delimiter => `InvalidData` with a safe message that does NOT echo the prefix value and does NOT invent an identity.
  4. Exact `S3PrefixRef.prefix` remains the unchanged provider value. No normalization, trim, `//` collapse, `Path`/`PathBuf`/`canonicalize`, dot resolution.
  5. Folder-marker dedup (incl. repeated-slash self/child markers) stays correct: `foo//` self marker suppressed; `foo//child/` child marker deduped to exactly one `S3PrefixRef`.
  6. `Location::S3.prefix` is the navigation representation = exact `CommonPrefix` with exactly ONE final protocol delimiter removed; it MAY end in `/` for repeated-delimiter providers. DESIGN_S3.md §9 reflects this.
- **Stop conditions:** Normalization. Capability flip. Production nav-conversion helper in s3.rs (S3-24 owns that in actions.rs). Touching TUI/actions.rs/PaneLoader/Cargo.
- **Hermes prompt:** Make the wire-prefix seam reversible: append exactly one `/` to every non-empty nav prefix; require CommonPrefix to end in `/` (else InvalidData, no echo); update DESIGN_S3.md §9 wording; tests: wire matrix (empty/plain/literal-trailing-slash/repeated-slashes/nested/dotdot/dot/unicode) + roundtrip table + CommonPrefix validation (exact identity, missing-delimiter rejected, awkward + unicode valid) + repeated-slash marker regression.

### S3-23 — Pane ListedEntry seam + Enter S3BucketRef
- **Phase:** P10
- **Status:** DONE
- **Depends on:** S3-13..15, S3-18..22
- **Allowed files:** src/services/pane_loader.rs, src/services/mod.rs, src/app/mod.rs, src/app/actions.rs, src/tui.rs, tests/async_vfs_contracts.rs (test-only compile consumer), docs/S3_KANBAN.md
- **Acceptance:**
  1. Pane first-page loading uses `ProviderRegistry::list_page`, not legacy `list_location_async`.
  2. `PaneLoadResponse` carries `ListedEntry` identity end-to-end.
  3. Provider continuation from the first page is wrapped as `PaneListingContinuation` with the exact provider instance, exact `Location`, and exact `PaneLoadId` generation.
  4. Current pane continuation may be retained for a future next-page card, but S3-23 does not fetch or append page 2.
  5. Local/SFTP/Archive retain existing presentation and navigation behavior; their page-adapter identity remains `EntryIdentity::Other`.
  6. TUI backing data never separates `Entry` from `EntryIdentity` by name, hash, or parallel vectors that can drift during sort/filter.
  7. Enter on `S3Bucket` uses the exact `S3BucketRef`: `Location::S3 { target: exact ref.target, bucket: Some(exact ref.bucket), prefix: "" }`.
  8. S3 bucket navigation never uses `Entry.name`.
  9. `S3Prefix` remains non-navigable until S3-24 and never falls through to `Location::child(entry.name)`.
  10. `S3Object` remains non-directory navigation.
  11. No S3 virtual-parent behavior; S3-25 owns it.
  12. No capability flip.
  13. No page-next fetch or append.
- **Stop conditions:** Identity reconstruction from presentation, parallel sortable entry/identity storage, page-2 fetch, capability flip, or any required file outside the allowed list.
- **Hermes prompt:** Preserve provider-listed identity through the pane first-page seam. Enter an S3 bucket only from exact `S3BucketRef`; retain legacy `EntryIdentity::Other` navigation. Store but do not consume continuation. No S3 prefix or parent navigation and no capability change.

### S3-24 — Enter S3PrefixRef
- **Phase:** P10
- **Status:** DONE (PR #100 merged; merge SHA pending; independent two-axis review APPROVED 0/0/0/0, quality+msrv SUCCESS)
- **Depends on:** S3-23, S3-20R
- **Allowed files:** src/app/actions.rs, src/tui.rs (tests only)
- **Acceptance:** Navigate into `CommonPrefix` using exact `S3PrefixRef`; never reconstruct from presentation name. Remove exactly one final protocol `/` from the exact ref to form `Location::S3.prefix`, preserving every preceding byte, including `//`, `.`, `..`, spaces, and Unicode. Update the existing TUI integration regression so prefix rows navigate while S3 objects and S3 `Other` remain fail-closed. Prove exact ref `foo//` -> nav `foo/`; S3-20R separately proves nav `foo/` -> wire `foo//` (the composed round trip is exact). Because S3-20R now rejects any `CommonPrefix` lacking a trailing `/` (Delimiter="/"), S3-24 may accept only refs that passed that invariant and strip exactly one delimiter — no fallback for a `S3PrefixRef` without trailing `/`.
- **Stop conditions:** Name reconstruction, removing more than one final delimiter, changing the provider wire seam, or production changes outside `src/app/actions.rs` (+ test-only `src/tui.rs`).
- **Hermes prompt:** After S3-20R merges, navigate into CommonPrefix from exact `S3PrefixRef`: strip exactly one final protocol `/`, preserve all preceding bytes, and never use presentation name. Production only in actions.rs; update the stale TUI assertion test-only. S3Object and S3 Other remain non-navigable.

### S3-24P — Pane incremental pagination
- **Phase:** P10
- **Status:** DONE (PR #98 merged; merge SHA 19d4c04; independent two-axis review APPROVED_FOR_COMMIT 29/29, quality+msrv SUCCESS)
- **Depends on:** S3-23, S3-19, S3-21
- **Allowed files:** src/services/pane_loader.rs, src/services/mod.rs, src/app/mod.rs, src/tui.rs, tests/async_vfs_contracts.rs
- **Acceptance:**
  1. Consume the stored `PaneListingContinuation` incrementally; do not reconstruct it.
  2. Allow at most one pending next-page request per pane (`PanePageRequestId`, separate counter).
  3. Correlate each page by exact listing generation, `Location`, provider instance, and page-request identity.
  4. Silently discard stale/duplicate/foreign responses without appending rows or mutating continuation state.
  5. Append complete `ListedEntry` values without separating presentation from operational identity.
  6. Atomically replace the consumed continuation with the returned continuation.
  7. `None` means end-of-list and no `Load more…` row.
  8. A page error keeps already loaded rows and the original continuation for retry.
  9. No eager or automatic pagination (only explicit Enter on the `LoadMore` virtual row triggers `schedule_next_page`, the sole `load_next` caller).
  10. No capability flip.
  11. Local/SFTP/Archive behavior remains unchanged.
- **Stop conditions:** Eager enumeration, duplicate append, stale page append, Entry/identity separation, or capability flip.
- **Hermes prompt:** Add explicit, stale-safe pane next-page loading from the exact stored continuation. Preserve listing generation and `ListedEntry` identity, append once, replace continuation atomically, retain rows/token on page error, and render `Load more…` only while a continuation exists. No eager pagination or capability change.

### S3-25 — Contextual virtual S3 parent
- **Phase:** P10
- **Status:** DONE (PR #101 merged; merge SHA 85afb6a; independent two-axis review APPROVED 0/0/0/0, quality+msrv SUCCESS)
- **Depends on:** S3-23, S3-24
- **Allowed files:** src/vfs/mod.rs, src/app/actions.rs, src/app/mod.rs, src/tui.rs, docs/S3_KANBAN.md
- **Acceptance:** Contextual S3 virtual-parent navigation that preserves account-style target-root and bucket-bound least-privilege semantics, awkward repeated-slash prefixes, literal `.`/`..` segments, and Local/SFTP/Archive parent behavior, using exactly ONE authoritative configured-target inventory (ProviderRegistry). Virtual `..` is UI/navigation state only — never S3ObjectRef, S3PrefixRef, object key, or provider-listed EntryIdentity.
  - `Location::parent()` stays fail-closed for `Location::S3` (context-free; must not guess account-root or read S3TargetConfig). Add a contextual app-level resolver instead.
  - ProviderRegistry gains a narrow read-only accessor `s3_target_binding(target_id) -> Option<S3TargetBinding>` (AccountRoot | BucketBound(exact bucket)), reading the EXISTING `s3_targets` inventory only — no second inventory, no config/SDK exposure.
  - `navigation_parent_target(current, registry)` in actions.rs: Local/SFTP/Archive delegate to existing `current.parent()` unchanged; S3 looks up binding, enforces current bucket == bound exactly (bucket-bound), computes parent prefix via `rfind('/')` segment removal (`"foo//"` -> `"foo/"`, `"foo/../bar"` -> `"foo/.."`), target-root and bucket-bound-root are terminal (`None`), unknown target fails closed. No `trim_end_matches`/`Path`/canonicalize/`//`-collapse/Unicode-normalize.
  - TUI: all three parent decisions (virtual-Parent visibility, Parent-row Enter, Backspace) MUST use the same contextual resolver — no divergence between visibility and navigation.
  - Least-privilege: a bucket-bound target NEVER navigates from its bucket root to `bucket: None` (would enable ListBuckets/s3:ListAllMyBuckets). Tested directly.
- **Stop conditions:** Turning `..` into an object key/EntryIdentity; guessing account-root in `Location::parent()`; a second target inventory; AWS/network/client/pagination/capability change; production edits outside the allowed files.
- **Hermes prompt:** One writer implements S3-25 across vfs/mod.rs + app/actions.rs + tui.rs: narrow registry binding accessor, contextual `navigation_parent_target`, and unify TUI's three parent decisions onto it. LUNA runs the parallel read-only S3-26 capability gate audit. No merge, no S3-26.

### S3-26 — Enable List capability
- **Phase:** P10
- **Status:** DONE (PR #104 merged; merge SHA 9c27a84; exact-head quality+msrv SUCCESS; full gates pass)
- **Depends on:** S3-18..25, S3-24P, S3-26A
- **Allowed files:** src/vfs/capabilities.rs, src/vfs/s3.rs
- **Acceptance:** Only now enable S3 List capability. Preconditions: S3-18..25 complete and tested; consumer pagination complete and tested through S3-24P. Change only capability declaration/tests. No Read/Write/Delete/Mkdir yet.
- **Stop conditions:** Enabling Read/Write/Delete/Mkdir.
- **Hermes prompt:** Flip S3 List capability ONLY after S3-18..25, S3-24P, and S3-26A are done and tested, and STOP GATE C passes. No other capability. Change capability declaration/tests only.

### S3-26A — List-only action/selection surface hardening
- **Phase:** P10
- **Status:** DONE (PR #102 merged; merge SHA 6967ba2; independent two-axis review APPROVED 0/0/0/0, quality+msrv SUCCESS; STOP GATE C PASS on exact tree 6967ba2)
- **Depends on:** S3-25, S3-24P
- **Allowed files:** src/app/availability.rs, src/tui.rs, docs/S3_KANBAN.md
- **Acceptance:** Make a hypothetical S3 capability set of ONLY `{List}` expose navigation/listing only — no transfer, mutation, workspace sync, or identity-unsafe selection.
  - **A1 Copy matrix:** replace `active==Local || passive==Local` with the EXACT implemented transfer matrix — `Local->Local`, `Local->SFTP`, `SFTP->Local` AVAILABLE; everything else (incl. S3<->Local, S3<->S3, S3<->SFTP, Archive<->Local) DISABLED. Private helper `copy_pair_supported(active, passive)`. Do not change TransferPlanner.
  - **A2 S3 selection fail-closed:** while selection state is name-based, `ActionId::ToggleSelect` is Disabled when `active_provider == S3`. No selection-storage redesign.
  - **A3 Workspace compare/sync block:** when EITHER pane provider is S3, disable `ToggleWorkspaceComparison`, `PreviewWorkspaceSync`, `ReverseWorkspaceDirection`, `ToggleWorkspaceSyncMode`, `ExecuteWorkspaceSync`, `ConfirmWorkspaceSync`. Keep lifecycle actions for existing jobs (`CancelWorkspaceSync`, `ShowWorkspaceSyncDetails`, `ShowWorkspaceVerificationDiff`, `ReturnToWorkspaceSyncPreview`) state-driven.
  - **A4 Assert with hypothetical S3 {List}:** View/Edit/Copy/Move/Mkdir/Delete/ToggleSelect/Symlink/Chmod/Hardlink/Chown all Disabled; Enter/Back/Refresh remain available.
  - **B1 Shift+F6 rename:** `Location::Local` preserves current rename; SFTP/Archive/S3 do NOT populate `state.cmd` / enter cmd_input — message "Rename is currently local-only". No S3/SFTP rename, no identity reconstruction from `Entry.name`.
  - **B2 Direct S3 selection bypasses:** for active `Location::S3`, no-op mouse-drag/glob-select/`*`-invert selection (no `state.toggle_selection(... entry.name ...)`). Local/SFTP/Archive unchanged.
  - **B3 LoadMore / virtual Parent** remain non-selectable/non-rename targets.
  - **B4 Context menu** is static presentation with no independent dispatch lane — record as UX NIT `UI-CONTEXT-AVAILABILITY`, do not redesign.
- **Stop conditions:** Enabling any S3 capability; S3 rename; redesigning selection storage; touching `src/vfs/*`, `src/services/*`, `src/transfer/*`, `src/app/mod.rs`, `src/app/actions.rs`, `Cargo.toml`, `Cargo.lock`, `docs/DESIGN_S3.md`.
- **Hermes prompt:** Disjoint writers — TERRA on `src/app/availability.rs` only (Copy matrix, S3 ToggleSelect disable, workspace compare/sync block); SOL on `src/tui.rs` only (Shift+F6 Local-only, direct S3 selection no-op). No shared production files. One integration owner (SOL) cherry-picks both onto `s3/s3-26a-list-surface-hardening` after review. STOP IN REVIEW, no merge, no S3-26.

### S3-27R — exact ListedEntry preview identity seam
- **Phase:** P10
- **Status:** DONE (PR #105 merged; merge SHA 1568e35; exact-head quality+msrv SUCCESS)
- **Depends on:** S3-25
- **Allowed files:** src/vfs/mod.rs, src/effects.rs, src/process/mod.rs, src/tui.rs, docs/S3_KANBAN.md
- **Acceptance:**
  1. `VfsProvider::read_listed_prefix_bytes` default trait method: `Other` -> legacy `read_prefix_bytes` via validated path; structured identities (S3Object/S3Prefix/S3Bucket) -> Unsupported (fail closed).
  2. `ProviderRegistry::read_listed_prefix_bytes_at` uses `provider_for_page_location` resolver (same as `list_page`), calls provider's `read_listed_prefix_bytes`. No name flattening.
  3. `Effect::PreviewLocation` carries `ListedEntry` (presentation + exact `EntryIdentity`) — no `name`/`total_size` fields.
  4. `ProcessService` executes via `registry.read_listed_prefix_bytes_at(&location, &listed, MAX_TEXT_PREVIEW_BYTES)`. Preview formatter uses `listed.entry.name`/`listed.entry.size` for title/formatting only.
  5. TUI `dispatch_ui_action` uses `VisiblePaneRow::Listed(&ListedEntry)` for remote preview — never reduces to `&Entry` before constructing effect.
  6. Tests A–H (identity survives UI/effect boundary; SFTP Other uses legacy; S3 identity never invokes name path; S3Prefix/S3Bucket fail closed; mismatched target fails closed untouched; Parent/LoadMore no preview target; duplicate names distinct identities).
- **Stop conditions:** Any S3 GetObject/Read implementation; capability flips; touching files outside allowed list.
- **Hermes prompt:** Add identity-aware preview boundary so S3 preview later uses exact S3ObjectRef identity, never Entry.name. Keep legacy behavior intact. 4 production files + docs + tests.

### S3-27 — S3 bounded Range GET
- **Phase:** P11
- **Status:** DONE (PR #106 merged; merge SHA a49b94b; exact-head quality+msrv SUCCESS; bounded/fail-closed GetObject)
- **Depends on:** STOP GATE C, S3-27R
- **Allowed files:** src/vfs/s3.rs
- **Acceptance:** read_prefix_bytes for exact S3ObjectRef via GetObject Range: bytes=0..N. Respect bounded-read semantics. No full download. No F3 availability yet.
- **Stop conditions:** Full object download. F3 enable.
- **Hermes prompt:** Implement read_prefix_bytes for exact S3ObjectRef: GetObject Range bytes=0..N, bounded. No full download, no F3 enable.

### S3-28 — S3 bounded-read metadata
- **Phase:** P11
- **Status:** DONE (PR #107 merged; merge SHA 1fd6bec; exact-head quality+msrv SUCCESS; truthful BoundedRead, no POSIX, boundary tests)
- **Depends on:** S3-27
- **Allowed files:** src/vfs/s3.rs
- **Acceptance:** Return truthful BoundedRead: bytes read, truncated/full where provable. Do NOT invent POSIX mode/uid/gid. No RemoteEditRevision.
- **Stop conditions:** Inventing mode/uid/gid. RemoteEditRevision.
- **Hermes prompt:** Return BoundedRead for S3: bytes + truncated flag (truthful). Do NOT fake mode/uid/gid. No RemoteEditRevision.

### S3-29 — Enable S3 F3
- **Phase:** P11
- **Status:** DONE (PR #108 merged; merge SHA 242dff6; exact-head quality+msrv SUCCESS; F3 Read-gated identity-aware, F4 disabled, E2E regression)
- **Depends on:** S3-28, S3-27R
- **Allowed files:** src/app/availability.rs
- **Acceptance:** Allow F3 for regular S3 object when Read implemented. Use exact S3ObjectRef. Reuse existing text/line limit, binary refusal, invalid UTF-8 handling. No F4.
- **Stop conditions:** Enabling F4.
- **Hermes prompt:** Enable F3 for S3 regular object via exact S3ObjectRef. Reuse existing preview limits/refusals. No F4.

### S3-30 — Enable Read capability
- **Phase:** P11
- **Status:** DONE (PR #109 merged; merge SHA d74fb55; P11 READ/F3 GATE PASS; POST-FLIP frozen-tree audit A/B/C BLOCKER 0 MAJOR 0; F3 handler-wiring debt resolved by S3-30R)
- **Depends on:** S3-29
- **Allowed files:** src/vfs/capabilities.rs
- **Acceptance:** Flip S3 Read capability only after F3 passes. No Write.
- **Stop conditions:** Write enable.
- **Hermes prompt:** Flip S3 Read capability ONLY after S3-29 passes. No Write.

### S3-30R — S3 F3 dispatch correction
- **Phase:** P11
- **Status:** DONE (PR #111 merged; P11 F3 fully user-visible)
- **Depends on:** S3-30
- **Allowed files:** src/tui.rs, docs/S3_KANBAN.md
- **Acceptance:** S3 regular `S3Object` row + Read => `Action::ViewFile` dispatches `Effect::PreviewLocation { location, listed }` carrying the exact `S3ObjectRef` (same lane as SFTP). Local/Archive keep their own preview paths. No separate S3 preview impl; no `Entry.name` as operation target.
- **Stop conditions:** S3 F3 using `Entry.name`; separate S3 preview implementation.
- **Hermes prompt:** Change the `ProviderId::Sftp` preview-route condition to `ProviderId::Sftp | ProviderId::S3`. Correct old wording that called the missing wiring merely "MINOR debt": P11 safety gate was fail-closed, real user-visible F3 completed only after S3-30R.

> ### ✅ STOP GATE C+ — P11 SAFE S3 READ / F3 MILESTONE PASS (frozen tree d74fb55)
> - **Capability flip:** S3 `NONE.with(List)` → `NONE.with(List).with(Read)` (S3-30).
> - **Surface:** Enter/Back/Refresh + F3 preview read on regular S3Object. All mutations/transfer/workspace/name-selection remain disabled.
> - **Identity:** F3 availability routes via exact `S3ObjectRef` (S3-27R seam); S3-27 bounded/fail-closed GetObject; S3-28 truthful BoundedRead (no POSIX/F4).
> - **POST-FLIP frozen-tree audit** (3 parallel tracks, exact tree e63e714): A PASS, B PASS (1 MINOR at the time: S3 F3 handler not yet wired through identity seam — fail-closed no-op, safe), C PASS → **BLOCKER 0, MAJOR 0, MINOR 1, NIT 0**.
> - **Resolved by S3-30R:** the S3 F3 handler-wiring debt is closed — `ProviderId::S3` now routes `Action::ViewFile` through `Effect::PreviewLocation { location, listed }` carrying the exact `S3ObjectRef` (see S3-30R). Old wording calling this merely "MINOR debt" was corrected: P11 safety gate was fail-closed, but real user-visible F3 became complete only after S3-30R.
> - **Axis safety:** LIST_SAFE ✅ · READ_SAFE ✅ · F3_EXACT_IDENTITY ✅ · BOUNDED_NETWORK ✅ · F4_DISABLED ✅ · TRANSFER_DISABLED ✅ · MUTATION_DISABLED ✅ · SELECTION_SAFE ✅ · WORKSPACE_SAFE ✅.
> - **Exact-head CI:** quality SUCCESS, msrv SUCCESS.
> - **Debt (recorded, not in scope):** STOP GATE C NIT (Mkdir/Symlink/HardLink availability-only guard) still pending until S3 first gains a mutation capability. S3 F3 handler-wiring debt was resolved by S3-30R.

### S3-31 — TransferMethod::S3
- **Phase:** P12
- **Status:** READY (S3-30 DONE; P11 READ/F3 GATE PASS on d74fb55)
- **Depends on:** S3-30
- **Allowed files:** src/transfer/mod.rs
- **Acceptance:** Add S3 as explicit TransferMethod. No executor yet. Planner still returns unsupported until availability says executor exists. No shell fallback. No aws CLI.
- **Stop conditions:** Executor. Shell fallback. aws CLI.
- **Hermes prompt:** Add TransferMethod::S3 (no executor). Planner returns unsupported until executor availability exists. No shell/aws CLI.

### S3-32 — Local→S3 planner route
- **Phase:** P12
- **Status:** BACKLOG
- **Depends on:** S3-31
- **Allowed files:** src/transfer/mod.rs
- **Acceptance:** Recognize Local→S3 as candidate TransferMethod::S3. Reject if executor unavailable. Tests: correct route; S3→S3 unsupported; SFTP→S3 unsupported. No upload impl.
- **Stop conditions:** Upload implementation. Allowing S3→S3/SFTP→S3.
- **Hermes prompt:** Planner: Local→S3 => TransferMethod::S3 candidate; reject if executor unavailable. Tests: route OK; S3→S3 unsupported; SFTP→S3 unsupported. No upload yet.

### S3-33 — S3→Local planner route
- **Phase:** P12
- **Status:** BACKLOG
- **Depends on:** S3-31
- **Allowed files:** src/transfer/mod.rs
- **Acceptance:** Recognize S3→Local. Same constraints. Keep unsupported: S3→S3, SFTP↔S3, S3 move.
- **Stop conditions:** Enabling S3→S3/SFTP↔S3/S3 move.
- **Hermes prompt:** Planner: S3→Local => candidate. Keep unsupported: S3→S3, SFTP↔S3, S3 move. No impl.

### S3-34 — PutObject executor
- **Phase:** P13
- **Status:** BACKLOG
- **Depends on:** S3-33
- **Allowed files:** src/transfer/s3_upload.rs (new) or src/transfer/executor.rs
- **Acceptance:** Upload one small local regular file to S3. Exact dest: frozen S3 target/bucket/key. PutObject. No multipart, no recursive dir, no F5 exposure yet. Truthful physical result.
- **Stop conditions:** Multipart. Recursive. F5 exposure.
- **Hermes prompt:** Implement small-file upload executor: PutObject to frozen S3ObjectRef. No multipart, no recursion, no F5. Return truthful result.

### S3-35 — Upload overwrite semantics
- **Phase:** P13
- **Status:** BACKLOG
- **Depends on:** S3-34
- **Allowed files:** src/transfer/s3_upload.rs
- **Acceptance:** Define/enforce overwrite per frozen TransferPlan. No silent conflict invention. Tests: new object; existing dest; permission denied; network failure.
- **Stop conditions:** Silent conflict resolution.
- **Hermes prompt:** Upload overwrite follows frozen TransferPlan exactly. No invented conflict resolution. Tests: new/existing/perm-denied/network-fail.

### S3-36 — Upload Job integration
- **Phase:** P13
- **Status:** BACKLOG
- **Depends on:** S3-34, S3-35
- **Allowed files:** src/jobs/mod.rs, src/transfer/executor.rs
- **Acceptance:** Run small upload through Job lifecycle: Queued/Running/Completed/Failed/Cancelled. No fake progress % if SDK can't prove bytes.
- **Stop conditions:** Fake progress.
- **Hermes prompt:** Wire small upload into Job lifecycle (Queued/Running/Completed/Failed/Cancelled). No fake % if bytes unproven.

### S3-37 — GetObject download to staged file
- **Phase:** P14
- **Status:** BACKLOG
- **Depends on:** S3-33
- **Allowed files:** src/transfer/s3_download.rs (new)
- **Acceptance:** Download exact S3ObjectRef to temporary/staged local file. Never stream directly into final if failure could expose partial success. No final rename yet.
- **Stop conditions:** Final rename. Direct-to-final stream.
- **Hermes prompt:** Download exact S3ObjectRef to STAGED temp file (not final). No final rename yet.

### S3-38 — Safe local commit
- **Phase:** P14
- **Status:** BACKLOG
- **Depends on:** S3-37
- **Allowed files:** src/transfer/s3_download.rs
- **Acceptance:** After success: close, flush/fsync per project policy, rename staged=>final. Failure/cancel: clean staged. Cleanup failure: report factual leftover path.
- **Stop conditions:** Leaving partial final. Hiding leftover.
- **Hermes prompt:** On success: fsync+rename staged=>final. On fail/cancel: remove staged. If cleanup fails: report leftover path factually.

### S3-39 — Download Job integration
- **Phase:** P14
- **Status:** BACKLOG
- **Depends on:** S3-37, S3-38
- **Allowed files:** src/jobs/mod.rs, src/transfer/executor.rs
- **Acceptance:** S3→Local through Job lifecycle. Cancellation must not leave final path falsely looking successful.
- **Stop conditions:** False success on cancel.
- **Hermes prompt:** S3→Local via Job lifecycle. Cancel must not leave final looking successful.

### S3-40 — Enable Local→S3 F5
- **Phase:** P15
- **Status:** BACKLOG
- **Depends on:** S3-36, S3-39
- **Allowed files:** src/app/availability.rs
- **Acceptance:** Expose F5 only for implemented Local→S3 route. Availability via TransferPlanner/executor, NOT Capability::Copy. Physical smoke: single small file upload.
- **Stop conditions:** Capability::Copy. S3→S3.
- **Hermes prompt:** Enable F5 for Local→S3 only (via planner/executor availability, NOT Capability::Copy). Smoke: single small upload.

### S3-41 — Enable S3→Local F5
- **Phase:** P15
- **Status:** BACKLOG
- **Depends on:** S3-39
- **Allowed files:** src/app/availability.rs
- **Acceptance:** Expose F5 for implemented S3→Local route. Exact object identity. Physical smoke: single small object download.
- **Stop conditions:** S3→S3.
- **Hermes prompt:** Enable F5 for S3→Local only. Exact object identity. Smoke: single small download.

### S3-42 — Write capability review
- **Phase:** P15
- **Status:** BACKLOG
- **Depends on:** S3-40, S3-41
- **Allowed files:** src/vfs/capabilities.rs
- **Acceptance:** Decide if Write semantics satisfied. If yes enable Write. If ambiguous (provider mutation beyond transfer dest) document interpretation before flip. Do NOT enable Copy.
- **Stop conditions:** Enabling Copy capability.
- **Hermes prompt:** Review Write capability: enable if satisfied; document interpretation if ambiguous. Do NOT enable Copy.

### S3-43 — Multipart threshold calculation
- **Phase:** P16
- **Status:** BACKLOG
- **Depends on:** STOP GATE D
- **Allowed files:** src/transfer/s3_multipart.rs (new, pure)
- **Acceptance:** Pure logic: part sizing per S3 constraints. No network. Tests: threshold boundary; large; very large; part-count limit.
- **Stop conditions:** Network calls.
- **Hermes prompt:** Pure multipart part-sizing logic (S3 constraints). Tests: threshold/large/very-large/part-limit. No network.

### S3-44 — CreateMultipartUpload
- **Phase:** P16
- **Status:** BACKLOG
- **Depends on:** S3-43
- **Allowed files:** src/transfer/s3_multipart.rs
- **Acceptance:** Start multipart, preserve upload_id in op state. No UploadPart loop. Failure: truthful.
- **Stop conditions:** Part loop.
- **Hermes prompt:** CreateMultipartUpload: start, store upload_id in state. No part loop. Truthful failure.

### S3-45 — Upload one multipart part
- **Phase:** P16
- **Status:** BACKLOG
- **Depends on:** S3-44
- **Allowed files:** src/transfer/s3_multipart.rs
- **Acceptance:** Upload exactly one numbered part. Return ETag/part completion evidence required by Complete. No loop.
- **Stop conditions:** Loop.
- **Hermes prompt:** Upload ONE numbered part; return ETag/evidence. No loop.

### S3-46 — Sequential multipart loop
- **Phase:** P16
- **Status:** BACKLOG
- **Depends on:** S3-45
- **Allowed files:** src/transfer/s3_multipart.rs
- **Acceptance:** Upload multiple parts sequentially (no concurrency). Record progress only from bytes actually accepted.
- **Stop conditions:** Concurrency.
- **Hermes prompt:** Sequential multipart loop (no concurrency). Progress only from bytes accepted.

### S3-47 — CompleteMultipartUpload
- **Phase:** P16
- **Status:** BACKLOG
- **Depends on:** S3-46
- **Allowed files:** src/transfer/s3_multipart.rs
- **Acceptance:** Complete only after all parts succeeded. Failure: do not claim object completed.
- **Stop conditions:** Claiming complete on partial.
- **Hermes prompt:** CompleteMultipartUpload only after all parts ok. Never claim complete on failure.

### S3-48 — AbortMultipartUpload
- **Phase:** P16
- **Status:** BACKLOG
- **Depends on:** S3-44, S3-47
- **Allowed files:** src/transfer/s3_multipart.rs
- **Acceptance:** Best-effort abort after failure/cancel once upload_id exists. Distinguish: confirmed / failed / unknown. Never hide possible orphan.
- **Stop conditions:** Hiding orphan.
- **Hermes prompt:** AbortMultipartUpload: distinguish confirmed/failed/unknown. Never hide possible orphan upload.

### S3-49 — Multipart cancellation
- **Phase:** P16
- **Status:** BACKLOG
- **Depends on:** S3-48
- **Allowed files:** src/jobs/mod.rs, src/transfer/s3_multipart.rs
- **Acceptance:** Connect Job cancel to multipart state: stop scheduling new parts, attempt abort, preserve physical truth. Stubbed-failure tests.
- **Stop conditions:** Silent partial success.
- **Hermes prompt:** Job cancel => stop new parts, attempt abort, preserve truth. Stubbed-failure tests.

### S3-50 — Multipart progress
- **Phase:** P16
- **Status:** BACKLOG
- **Depends on:** S3-46, S3-49
- **Allowed files:** src/transfer/s3_multipart.rs, src/jobs/mod.rs
- **Acceptance:** Report factual bytes progress. No fake ETA. No % if total unknown. Local upload total known: bytes_done/total_bytes.
- **Stop conditions:** Fake ETA / fake %.
- **Hermes prompt:** Multipart progress: factual bytes_done/total. No fake ETA, no % if unknown.

### S3-51 — S3 checksum evidence model
- **Phase:** P17
- **Status:** BACKLOG
- **Depends on:** STOP GATE E
- **Allowed files:** src/vfs/s3.rs
- **Acceptance:** Represent S3 integrity evidence: size, ETag, VersionId, checksum fields. Do NOT treat ETag as universal content hash. No WorkspaceFingerprint yet.
- **Stop conditions:** ETag=>content_hash. WorkspaceFingerprint.
- **Hermes prompt:** Model S3 evidence (size/ETag/VersionId/checksum). ETag is NOT universal content hash. No WorkspaceFingerprint.

### S3-52 — Upload verification
- **Phase:** P17
- **Status:** BACKLOG
- **Depends on:** S3-51, S3-47
- **Allowed files:** src/transfer/s3_upload.rs
- **Acceptance:** After physical upload, verify with strongest trustworthy evidence. Return Verified/Inconclusive/Failed. Completion != verification success. Do not rewrite Job physical outcome.
- **Stop conditions:** Rewriting Job outcome.
- **Hermes prompt:** Post-upload verification: Verified/Inconclusive/Failed. Completion != verification. Don't rewrite physical outcome.

### S3-53 — Download verification
- **Phase:** P17
- **Status:** BACKLOG
- **Depends on:** S3-51, S3-38
- **Allowed files:** src/transfer/s3_download.rs
- **Acceptance:** Verify downloaded bytes via available trustworthy S3 evidence. If insufficient: Inconclusive. Never fake Verified from size alone unless design allows.
- **Stop conditions:** Fake Verified from size.
- **Hermes prompt:** Download verification via trustworthy S3 evidence. Insufficient=>Inconclusive. No fake Verified from size.

### S3-54 — Prefix creation primitive
- **Phase:** P18
- **Status:** BACKLOG
- **Depends on:** S3-50 / STOP GATE E
- **Allowed files:** src/vfs/s3.rs
- **Acceptance:** Create empty virtual folder marker: PutObject key ending '/', empty body. Inside bucket only. No bucket creation.
- **Stop conditions:** Bucket creation.
- **Hermes prompt:** Prefix-marker primitive: PutObject key ending '/', empty body, inside bucket only. No bucket create.

### S3-55 — S3 F7 availability
- **Phase:** P18
- **Status:** BACKLOG
- **Depends on:** S3-54
- **Allowed files:** src/app/availability.rs
- **Acceptance:** Enable F7 inside bucket/prefix. Disabled at target root. Never create bucket. Confirmation only if existing ARX mkdir UX requires.
- **Stop conditions:** Target-root F7. Bucket create.
- **Hermes prompt:** Enable F7 inside bucket/prefix only. Disabled at target root. No bucket create.

### S3-56 — Enable Mkdir capability
- **Phase:** P18
- **Status:** BACKLOG
- **Depends on:** S3-54, S3-55
- **Allowed files:** src/vfs/capabilities.rs
- **Acceptance:** Enable only after prefix creation works physically. Document: Mkdir = virtual prefix marker for S3.
- **Stop conditions:** Enabling before works.
- **Hermes prompt:** Enable Mkdir only after S3-54 works. Document: S3 Mkdir = virtual prefix marker.

### S3-57 — Head bucket versioning status
- **Phase:** P19
- **Status:** BACKLOG
- **Depends on:** S3-50
- **Allowed files:** src/vfs/s3.rs
- **Acceptance:** Determine versioning state if perms permit: Enabled/Suspended/Disabled/Unknown. Do not fail listing if permission unavailable.
- **Stop conditions:** Failing listing on missing perm.
- **Hermes prompt:** Detect bucket versioning state (Enabled/Suspended/Disabled/Unknown). Don't fail listing if unreadable.

### S3-58 — Single-object DeleteObject primitive
- **Phase:** P19
- **Status:** BACKLOG
- **Depends on:** S3-57
- **Allowed files:** src/vfs/s3.rs
- **Acceptance:** Delete one exact S3ObjectRef. No prefix recursion. No bucket delete. No delete-many.
- **Stop conditions:** Prefix recursion. Bucket delete.
- **Hermes prompt:** DeleteObject for one exact S3ObjectRef. No recursion, no bucket delete, no multi.

### S3-59 — Permanent delete confirmation
- **Phase:** P19
- **Status:** BACKLOG
- **Depends on:** S3-58
- **Allowed files:** src/app/actions.rs
- **Acceptance:** Show exact destructive target. Freeze target/bucket/key. Never reconstruct key from Entry.name. Message depends on known versioning state. Unknown: do NOT say 'cannot be undone'.
- **Stop conditions:** Entry.name reconstruction. 'cannot be undone' on unknown.
- **Hermes prompt:** F8 confirmation freezes exact target/bucket/key (never Entry.name). Message per versioning state; unknown => no 'cannot be undone'.

### S3-60 — Disable prefix recursive delete
- **Phase:** P19
- **Status:** BACKLOG
- **Depends on:** S3-58
- **Allowed files:** src/app/actions.rs, tests
- **Acceptance:** Test ensuring F8 on non-empty S3PrefixRef is unavailable. No accidental generic dir recursion.
- **Stop conditions:** Allowing recursive delete.
- **Hermes prompt:** Test: F8 on non-empty S3PrefixRef unavailable. No generic dir recursion.

### S3-61 — Enable Delete capability
- **Phase:** P19
- **Status:** BACKLOG
- **Depends on:** S3-58, S3-59, S3-60
- **Allowed files:** src/vfs/capabilities.rs
- **Acceptance:** Enable after safe single-object delete impl. Capability does NOT imply recursive prefix delete. Tests preserve distinction.
- **Stop conditions:** Recursive prefix delete.
- **Hermes prompt:** Enable Delete after S3-58/59/60. Delete != recursive prefix delete. Tests enforce.

### S3-62 — AWS basic acceptance
- **Phase:** P20
- **Status:** BACKLOG
- **Depends on:** STOP GATE F
- **Allowed files:** manual / tests with disposable AWS bucket
- **Acceptance:** Disposable AWS bucket: target root, bucket-bound root, prefix nav, Unicode, zero-byte, folder marker, F3, small upload, small download. No production bucket.
- **Stop conditions:** Production bucket.
- **Hermes prompt:** Physical AWS acceptance (disposable bucket): root/bucket-bound/prefix/Unicode/zero-byte/marker/F3/upload/download.

### S3-63 — AWS pagination acceptance
- **Phase:** P20
- **Status:** BACKLOG
- **Depends on:** S3-62
- **Allowed files:** tests
- **Acceptance:** Fixture >1000 objects (or forces page size). Verify page1/continuation/page2, no dup, no missing, no UI freeze.
- **Stop conditions:** UI freeze.
- **Hermes prompt:** AWS pagination acceptance: >1000 objs, page1/continuation/page2, no dup/missing, no freeze.

### S3-64 — AWS multipart acceptance
- **Phase:** P20
- **Status:** BACKLOG
- **Depends on:** S3-62
- **Allowed files:** tests
- **Acceptance:** Large disposable object: init/parts/progress/complete/verify. Second run: cancel/abort/no silent orphan.
- **Stop conditions:** Silent orphan.
- **Hermes prompt:** AWS multipart acceptance: large obj init/parts/complete/verify; cancel/abort/no orphan.

### S3-65 — AWS permission failure matrix
- **Phase:** P20
- **Status:** BACKLOG
- **Depends on:** S3-62
- **Allowed files:** tests
- **Acceptance:** Controlled denial: ListBuckets/ListObjects/Get/Put/Delete denied. Bucket-bound target still works without ListAllMyBuckets when bucket perms allow.
- **Stop conditions:** —
- **Hermes prompt:** AWS perm matrix: deny ListBuckets/Objects/Get/Put/Delete. Bucket-bound works without ListAllMyBuckets.

### S3-66 — MinIO target
- **Phase:** P21
- **Status:** BACKLOG
- **Depends on:** S3-65
- **Allowed files:** tests / config
- **Acceptance:** Disposable MinIO via endpoint_url (+force_path_style if needed). Prove connection, bucket listing/bound, prefix listing.
- **Stop conditions:** —
- **Hermes prompt:** MinIO target: endpoint_url (+force_path_style), connection + bucket/prefix listing.

### S3-67 — MinIO transfer acceptance
- **Phase:** P21
- **Status:** BACKLOG
- **Depends on:** S3-66
- **Allowed files:** tests
- **Acceptance:** F3, small upload, small download, multipart, cancel, prefix marker, single delete, Unicode.
- **Stop conditions:** —
- **Hermes prompt:** MinIO transfer: F3/upload/download/multipart/cancel/marker/delete/Unicode.

### S3-68 — Compatibility truth
- **Phase:** P21
- **Status:** BACKLOG
- **Depends on:** S3-66, S3-67
- **Allowed files:** docs/DESIGN_S3.md or README
- **Acceptance:** After AWS+MinIO: mark AWS S3=SUPPORTED, MinIO=SUPPORTED. Do NOT claim R2/Wasabi/generic unless physically tested.
- **Stop conditions:** Claiming untested compatibility.
- **Hermes prompt:** Mark AWS=SUPPORTED, MinIO=SUPPORTED only. No R2/Wasabi/generic claim without test.

### S3-69 — Commander labels
- **Phase:** P22
- **Status:** BACKLOG
- **Depends on:** STOP GATE F
- **Allowed files:** src/tui.rs, src/app/actions.rs
- **Acceptance:** S3 actions readable: keep F3/F5/F7/F8. Provider badge [S3 target-name]. No new shortcut system, no command-bar redesign.
- **Stop conditions:** Redesigning command bar.
- **Hermes prompt:** Commander labels for S3: keep F3/F5/F7/F8, badge [S3 name]. No redesign.

### S3-70 — Error wording
- **Phase:** P22
- **Status:** BACKLOG
- **Depends on:** S3-69
- **Allowed files:** src/vfs/s3.rs, src/app
- **Acceptance:** Audit user-visible S3 errors: translate SDK noise to factual ARX outcome, retain technical cause in details/log. Examples: access denied, bucket not found, object changed, transfer failed, verification inconclusive, multipart abort failed. Never print creds.
- **Stop conditions:** Printing credentials.
- **Hermes prompt:** S3 error wording: factual outcome + technical cause in details. Never creds.

### S3-71 — Help documentation
- **Phase:** P22
- **Status:** BACKLOG
- **Depends on:** S3-69, S3-70
- **Allowed files:** docs/
- **Acceptance:** Add S3 section: Browse, F3, F5 up/download, F7 virtual folder, F8 permanent single-object delete. Explicitly state unsupported: F4, F6, recursive prefix delete, S3→S3, Workspace Sync. No overclaim.
- **Stop conditions:** Overclaim.
- **Hermes prompt:** Docs S3 section: browse/F3/F5/F7/F8 + explicit unsupported table (F4/F6/recursive/S3→S3/Workspace).

### S3-72 — Local regression
- **Phase:** P23
- **Status:** BACKLOG
- **Depends on:** S3-71
- **Allowed files:** tests
- **Acceptance:** Full Local commander matrix. S3 must not change Local semantics: F3/F4/F5/F6/F7/F8, selection, mouse, nav, jobs. STOP on regression.
- **Stop conditions:** —
- **Hermes prompt:** Local regression matrix: F3-F8/selection/mouse/nav/jobs unchanged. STOP on regress.

### S3-73 — SFTP regression
- **Phase:** P23
- **Status:** BACKLOG
- **Depends on:** S3-72
- **Allowed files:** tests
- **Acceptance:** Existing SFTP matrix: F3, F4 conflict-safe edit, F5 Local↔SFTP, F7, F8, pagination infra must not alter pane behavior. STOP on regression.
- **Stop conditions:** —
- **Hermes prompt:** SFTP regression: F3/F4-safe/F5/F7/F8 + pagination infra unchanged. STOP on regress.

### S3-74 — Archive regression
- **Phase:** P23
- **Status:** BACKLOG
- **Depends on:** S3-73
- **Allowed files:** tests
- **Acceptance:** Archive provider tests/browsing. ListedEntry changes must not break archive identities. STOP.
- **Stop conditions:** —
- **Hermes prompt:** Archive regression: ListedEntry changes don't break archive identities.

### S3-75 — MVP architecture audit
- **Phase:** P24
- **Status:** BACKLOG
- **Depends on:** S3-74
- **Allowed files:** docs/DESIGN_S3.md
- **Acceptance:** Fresh review: exact object identity, no path norm, no Copy misuse, no hidden S3→S3, no recursive delete, no bucket delete, no F4, multipart truthful cancel, verification separate, no cred leak, no singleton client, pagination gen-safe. Required: 0 BLOCKER 0 MAJOR.
- **Stop conditions:** —
- **Hermes prompt:** MVP arch audit vs DESIGN_S3: all invariants. 0 BLOCKER 0 MAJOR.

### S3-76 — Full quality gate
- **Phase:** P24
- **Status:** BACKLOG
- **Depends on:** S3-75
- **Allowed files:** repo
- **Acceptance:** cargo fmt --check; clippy --all-targets --all-features -- -D warnings; cargo test --locked --all-features; cargo +1.88 check --locked --all-features; cargo build --locked --release; git diff --check.
- **Stop conditions:** —
- **Hermes prompt:** Full gate: fmt/clippy/test/check/build --locked + git diff --check.

### S3-77 — Physical MVP demo
- **Phase:** P24
- **Status:** BACKLOG
- **Depends on:** S3-76
- **Allowed files:** manual
- **Acceptance:** [LOCAL] ~/downloads <-> [S3 artifacts] s3://arx-test/releases/. Story: browse, enter prefix, F3, F5 download, F5 upload, large upload, F7 prefix, F8 single delete w/ confirm. No hidden failures.
- **Stop conditions:** —
- **Hermes prompt:** Physical demo: browse/enter/F3/F5down/F5up/large/F7/F8. No hidden failures.

### S3-78 — MVP docs truth
- **Phase:** P24
- **Status:** BACKLOG
- **Depends on:** S3-76, S3-77
- **Allowed files:** README, ARCHITECTURE, ROADMAP, DEMO
- **Acceptance:** Describe S3 as object storage. Never call prefixes POSIX dirs. Explicit unsupported table. STOP.
- **Stop conditions:** Calling prefixes POSIX dirs.
- **Hermes prompt:** Update README/ARCH/ROADMAP/DEMO: S3=object storage, no POSIX-dir claim, unsupported table.


## PARKED

### S3-80 — Workspace fingerprint evidence
- **Phase:** PK
- **Status:** PARKED
- **Depends on:** S3 MVP released
- **Allowed files:** docs/DESIGN_S3.md (design)
- **Acceptance:** Design only: map S3 evidence into WorkspaceFingerprint. Never ETag→content_hash blindly.
- **Stop conditions:** Implementation.
- **Hermes prompt:** DESIGN only: map S3 evidence -> WorkspaceFingerprint. No blind ETag->content_hash.

### S3-81 — S3 Workspace Compare
- **Phase:** PK
- **Status:** PARKED
- **Depends on:** S3-80
- **Allowed files:** design
- **Acceptance:** Read-only Compare Local↔S3. No execution.
- **Stop conditions:** Execution.
- **Hermes prompt:** DESIGN: read-only Local↔S3 Compare. No exec.

### S3-82 — Local→S3 Update Preview
- **Phase:** PK
- **Status:** PARKED
- **Depends on:** S3-81
- **Allowed files:** design
- **Acceptance:** Frozen plan, copies only initially, no deletes.
- **Stop conditions:** Deletes.
- **Hermes prompt:** DESIGN: frozen update plan, copies only, no deletes.

### S3-83 — Local→S3 Update execution
- **Phase:** PK
- **Status:** PARKED
- **Depends on:** S3-82
- **Allowed files:** design/impl (later)
- **Acceptance:** Execute frozen plan through S3 Jobs.
- **Stop conditions:** —
- **Hermes prompt:** DESIGN/impl later: execute frozen plan via S3 Jobs.

### S3-84 — Workspace verification
- **Phase:** PK
- **Status:** PARKED
- **Depends on:** S3-83
- **Allowed files:** design
- **Acceptance:** Execution complete != synchronized. Separate read-only verification.
- **Stop conditions:** —
- **Hermes prompt:** DESIGN: verify separate from execution.

### S3-85 — S3 Update product flow
- **Phase:** PK
- **Status:** PARKED
- **Depends on:** S3-84
- **Allowed files:** design
- **Acceptance:** Compare→Preview→Execute→Verifying→Synchronized.
- **Stop conditions:** —
- **Hermes prompt:** DESIGN: Compare/Preview/Execute/Verifying/Synchronized flow.

### S3-90 — ServerSideCopy
- **Phase:** PK
- **Status:** PARKED
- **Depends on:** S3 MVP released
- **Allowed files:** src/vfs/s3.rs, src/transfer
- **Acceptance:** Implement real CopyObject. Only then Capability::ServerSideCopy=YES.
- **Stop conditions:** CopyObject before impl.
- **Hermes prompt:** Implement real CopyObject; then ServerSideCopy=YES.

### S3-91 — S3 logical rename
- **Phase:** PK
- **Status:** PARKED
- **Depends on:** S3-90
- **Allowed files:** src/vfs/s3.rs
- **Acceptance:** Copy dest→verify→delete source. Truthful partial: CopiedButSourceRetained. Never atomic rename.
- **Stop conditions:** Atomic rename semantics.
- **Hermes prompt:** Logical rename: copy+verify+delete; partial=CopiedButSourceRetained. No atomic.

### S3-92 — Conditional F4 design
- **Phase:** PK
- **Status:** PARKED
- **Depends on:** S3 MVP released
- **Allowed files:** docs/DESIGN_S3.md
- **Acceptance:** Design only: S3 revision ETag/VersionId/checksum; no Unix metadata model; no impl without separate approval.
- **Stop conditions:** Implementation.
- **Hermes prompt:** DESIGN only: S3 F4 revision model (ETag/VersionId/checksum). No Unix meta.

### S3-93 — Recursive prefix delete design
- **Phase:** PK
- **Status:** PARKED
- **Depends on:** S3 MVP released
- **Allowed files:** docs/DESIGN_S3.md
- **Acceptance:** Design only: full preview, object count, bytes, versioning consequences, typed confirmation, background Job, partial outcome. Not auto-approved.
- **Stop conditions:** Auto-approval.
- **Hermes prompt:** DESIGN only: recursive prefix delete with preview/count/bytes/versioning/confirm/Job/partial. Not auto-approved.

### S3-94 — Read-only Object Inspector
- **Phase:** PK (POST-MVP)
- **Status:** PARKED
- **Depends on:** S3 MVP released
- **Allowed files:** docs/DESIGN_S3.md
- **Acceptance:** Ctrl+I read-only details for exact S3ObjectRef: target, bucket, key, size, last-modified, storage-class, ETag (not a content checksum), checksums, content-type, content-encoding, cache-control, metadata, encryption, version-id, restore status. PermissionDenied/Unknown explicit. On-demand only; ordinary listing must not depend on property calls.
- **Stop conditions:** Mutation. Making browse depend on all property calls.
- **Hermes prompt:** Read-only S3 Object Inspector fields; ETag≠checksum; PermissionDenied explicit; on-demand.

### S3-95 — Object tags / versions / lock details
- **Phase:** PK (POST-MVP)
- **Status:** PARKED
- **Depends on:** S3-94
- **Allowed files:** docs/DESIGN_S3.md
- **Acceptance:** Read-only optional queries: tags, versions, version-id, delete-marker state, retention, legal-hold, Object Lock. Permission failures must not break ordinary browsing. No mutation.
- **Stop conditions:** Mutation.
- **Hermes prompt:** Read-only object tags/versions/lock; permission failures non-fatal; no mutation.

### S3-96 — Bucket Inspector
- **Phase:** PK (POST-MVP)
- **Status:** PARKED
- **Depends on:** S3 MVP released
- **Allowed files:** docs/DESIGN_S3.md
- **Acceptance:** Read-only bucket properties where supported: region, versioning, default encryption, tags, Object Lock, public-access config, Requester Pays, transfer acceleration, lifecycle summary. AWS-specific fields may be Unsupported on MinIO; never fake compatibility.
- **Stop conditions:** Mutation. Faking AWS-only fields on MinIO.
- **Hermes prompt:** Read-only S3 Bucket Inspector; AWS-only fields Unsupported on MinIO; no fake compat.

### S3-97 — S3 Usage Analytics
- **Phase:** PK (POST-MVP)
- **Status:** PARKED
- **Depends on:** S3 MVP released
- **Allowed files:** docs/DESIGN_S3.md
- **Acceptance:** du-like: object count, total bytes, prefix aggregation, storage-class breakdown. Evidence source MUST be displayed (LiveScan / StorageLens / Inventory / OtherProvider / Unavailable). Stale data shows as-of timestamp. Live recursive scan is a cancellable background Job. No silent scan of millions of objects from pane render.
- **Stop conditions:** Blocking full-tree scan from pane render. Hiding evidence source.
- **Hermes prompt:** S3 usage analytics with explicit evidence source + freshness; live scan = cancellable Job.
