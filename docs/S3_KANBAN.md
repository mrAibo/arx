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

- **READY** (1): S3-08
- **DOING** (0): —
- **REVIEW** (0): —
- **BLOCKED** (0): —
- **DONE** (8): S3-00, S3-01, S3-02, S3-03, S3-04, S3-05, S3-06, S3-07
- **BACKLOG** (72): S3-08, S3-09, S3-10, S3-11, S3-12, S3-13, S3-14, S3-15, S3-16, S3-17, S3-18, S3-19, S3-20, S3-21, S3-22, S3-23, S3-24, S3-25, S3-26, S3-27, S3-28, S3-29, S3-30, S3-31, S3-32, S3-33, S3-34, S3-35, S3-36, S3-37, S3-38, S3-39, S3-40, S3-41, S3-42, S3-43, S3-44, S3-45, S3-46, S3-47, S3-48, S3-49, S3-50, S3-51, S3-52, S3-53, S3-54, S3-55, S3-56, S3-57, S3-58, S3-59, S3-60, S3-61, S3-62, S3-63, S3-64, S3-65, S3-66, S3-67, S3-68, S3-69, S3-70, S3-71, S3-72, S3-73, S3-74, S3-75, S3-76, S3-77, S3-78, S3-79
- **PARKED** (10): S3-80, S3-81, S3-82, S3-83, S3-84, S3-85, S3-90, S3-91, S3-92, S3-93

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
- **Status:** BACKLOG
- **Depends on:** S3-04
- **Allowed files:** src/vfs/mod.rs
- **Acceptance:** Add ProviderInstanceKey::S3Target(String). No registry client impl. Tests: two target IDs !=, S3 target != SFTP host, hash/map identity stable.
- **Stop conditions:** Registry client implementation.
- **Hermes prompt:** Add ProviderInstanceKey::S3Target(target_id). No client/registry impl. Tests: distinct ids differ, S3Target != SftpHost, map/hash identity stable.

### S3-09 — Location::S3
- **Phase:** P4
- **Status:** BACKLOG
- **Depends on:** S3-08
- **Allowed files:** src/vfs/mod.rs
- **Acceptance:** Add Location::S3{target, bucket: Option<String>, prefix}. bucket None=>target root; Some+empty prefix=>bucket root. Do NOT fs-normalize prefix. Tests: target root, bucket root, nested prefix, Unicode prefix, awkward slash preservation.
- **Stop conditions:** AWS calls. Normalization.
- **Hermes prompt:** Add Location::S3{target, bucket: Option<String>, prefix}. Rules: None=bucket list root, Some+empty=bucket root. No fs-normalization. Tests: target root / bucket root / nested / Unicode / double-slash preserved.

### S3-10 — S3 display identity
- **Phase:** P4
- **Status:** BACKLOG
- **Depends on:** S3-09
- **Allowed files:** src/vfs/mod.rs (Display)
- **Acceptance:** Render S3 locations truthfully: target root [S3 name]; bucket root s3://bucket/; prefix s3://bucket/prefix/. Never creds. No nav logic. Tests: Display output only.
- **Stop conditions:** Navigation logic changes.
- **Hermes prompt:** Implement Display for Location::S3 only. target root=>[S3 name]; bucket root=>s3://bucket/; prefix=>s3://bucket/prefix/. No creds, no nav logic. Tests: Display strings.

### S3-11 — ListedEntry / EntryIdentity core
- **Phase:** P5
- **Status:** BACKLOG
- **Depends on:** S3-09, S3-10
- **Allowed files:** src/vfs/mod.rs
- **Acceptance:** Add ListedEntry{entry, identity} and EntryIdentity variants S3Bucket/S3Object/S3Prefix (+ safe Other for existing). Do NOT convert all consumers. Do NOT break Local/SFTP. Tests: presentation name != operational identity; awkward key preserved exactly.
- **Stop conditions:** Converting all consumers. Breaking Local/SFTP.
- **Hermes prompt:** Introduce ListedEntry{entry: Entry, identity: EntryIdentity} with S3Bucket/S3Object/S3Prefix (+Other for existing providers). Do NOT migrate all consumers; do NOT break Local/SFTP. Tests: name!=identity; awkward key exact.

### S3-12 — S3 exact reference types
- **Phase:** P5
- **Status:** BACKLOG
- **Depends on:** S3-11
- **Allowed files:** src/vfs/s3.rs (or mod.rs)
- **Acceptance:** Implement S3BucketRef{target,bucket}, S3ObjectRef{target,bucket,key}, S3PrefixRef{target,bucket,prefix}. No path normalization. Tests exact preservation: foo//bar, foo/../bar, foo/./bar, Unicode, foo/.
- **Stop conditions:** Normalization.
- **Hermes prompt:** Implement S3BucketRef/S3ObjectRef/S3PrefixRef exactly as DESIGN_S3 §9/§11. No normalization. Tests preserve foo//bar, foo/../bar, foo/./bar, Unicode, foo/ verbatim.

### S3-13 — ProviderListingPage
- **Phase:** P6
- **Status:** BACKLOG
- **Depends on:** S3-11, S3-12
- **Allowed files:** src/vfs/mod.rs
- **Acceptance:** Add ProviderListingPage{entries, continuation} + ProviderContinuation{opaque token}. No PaneLoadId. No S3 network yet. Existing providers use continuation=None. Tests: Local/SFTP listing unchanged.
- **Stop conditions:** PaneLoadId in provider. S3 network code.
- **Hermes prompt:** Add provider-side ProviderListingPage{entries, continuation: Option<ProviderContinuation>} + opaque ProviderContinuation{token}. No PaneLoadId, no S3 calls. Existing providers: continuation=None. Tests: Local/SFTP unchanged.

### S3-14 — PaneListingContinuation
- **Phase:** P6
- **Status:** BACKLOG
- **Depends on:** S3-13
- **Allowed files:** src/services/pane_loader.rs, src/app/mod.rs
- **Acceptance:** Add PaneListingContinuation{provider_continuation, provider_instance, location, generation}. PaneLoadId stays in PaneLoader/AppState. No S3 code.
- **Stop conditions:** S3 code.
- **Hermes prompt:** Add PaneListingContinuation wrapping ProviderContinuation + provider_instance + location + generation. PaneLoadId remains pane-layer only. No S3.

### S3-15 — Stale page rejection
- **Phase:** P6
- **Status:** BACKLOG
- **Depends on:** S3-14
- **Allowed files:** src/services/pane_loader.rs, src/app/mod.rs
- **Acceptance:** Make paginated append generation-safe: page1 loc A, navigate B, old page2 A=>discard; refresh A, old gen page2=>discard; provider instance change=>discard. Tests around pane loader/app generation. No S3 calls.
- **Stop conditions:** S3 calls.
- **Hermes prompt:** Tests only: verify stale-page rejection in pane loader/app generation (navigate, refresh, provider-instance change all discard old continuation). No S3.

### S3-16 — S3 client factory
- **Phase:** P7
- **Status:** BACKLOG
- **Depends on:** STOP GATE B (S3-13..15 PASS, Local/SFTP no regress, no S3 caps)
- **Allowed files:** src/vfs/s3.rs
- **Acceptance:** Create AWS SDK client for one S3TargetConfig: region/profile/endpoint_url/force_path_style, standard credential chain, approved retry policy from S3-03. No list ops. Never log creds. Unit-test config translation.
- **Stop conditions:** List operations. Logging credentials.
- **Hermes prompt:** S3 client factory from S3TargetConfig (region/profile/endpoint_url/force_path_style, std credential chain, S3-03 retry policy). No list ops. Never log creds. Test config translation.

### S3-17 — Client registry lifecycle
- **Phase:** P7
- **Status:** BACKLOG
- **Depends on:** S3-16
- **Allowed files:** src/vfs/mod.rs, src/vfs/s3.rs
- **Acceptance:** ProviderInstanceKey::S3Target => correct target config => correct client. A!=B, separate endpoint/profile, no singleton S3 client. No listing yet.
- **Stop conditions:** Singleton S3 client. Listing.
- **Hermes prompt:** Wire S3Target(target_id) => target config => dedicated client. A!=B, no singleton. No listing yet.

### S3-18 — ListBuckets first page
- **Phase:** P8
- **Status:** BACKLOG
- **Depends on:** S3-17
- **Allowed files:** src/vfs/s3.rs
- **Acceptance:** Target-root first page via ListBuckets. Map bucket=>ListedEntry{presentation=bucket name, identity=S3BucketRef}. No Entry.name reconstruction. No create/delete bucket. Tests w/ mocked SDK boundary.
- **Stop conditions:** Bucket create/delete. Entry.name reconstruction.
- **Hermes prompt:** Implement target-root first page: ListBuckets => ListedEntry with identity=S3BucketRef (presentation=bucket name only). No name reconstruction. No bucket create/delete. Mock SDK boundary in tests.

### S3-19 — ListBuckets pagination
- **Phase:** P8
- **Status:** BACKLOG
- **Depends on:** S3-18
- **Allowed files:** src/vfs/s3.rs
- **Acceptance:** Support ContinuationToken via ProviderContinuation=>ProviderListingPage. Safety: has-more+no token=>ProtocolError; repeated/non-advancing token=>ProtocolError; no infinite loop; no eager full load.
- **Stop conditions:** Eager load all buckets.
- **Hermes prompt:** ListBuckets pagination via ProviderContinuation. ProtocolError on missing/non-advancing token. No infinite loop, no eager full load.

### S3-20 — ListObjectsV2 first page
- **Phase:** P9
- **Status:** BACKLOG
- **Depends on:** S3-17, S3-18
- **Allowed files:** src/vfs/s3.rs
- **Acceptance:** List bucket root/prefix: ListObjectsV2 Prefix=exact nav prefix, Delimiter="/". Contents=>S3ObjectRef; CommonPrefixes=>S3PrefixRef. Handle folder-marker duplication. No normalization.
- **Stop conditions:** Normalization.
- **Hermes prompt:** ListObjectsV2 first page: Prefix=exact nav prefix, Delimiter="/". Contents=>S3ObjectRef, CommonPrefixes=>S3PrefixRef. Handle folder-marker dup. No normalization.

### S3-21 — ListObjectsV2 pagination
- **Phase:** P9
- **Status:** BACKLOG
- **Depends on:** S3-20
- **Allowed files:** src/vfs/s3.rs
- **Acceptance:** Support NextContinuationToken. Same invariants: missing token while truncated=>ProtocolError; repeated token=>ProtocolError; no full-bucket eager loop.
- **Stop conditions:** Eager loop.
- **Hermes prompt:** ListObjectsV2 pagination via NextContinuationToken. ProtocolError on missing/non-advancing token. No eager full-bucket loop.

### S3-22 — Awkward key listing tests
- **Phase:** P9
- **Status:** BACKLOG
- **Depends on:** S3-20, S3-21
- **Allowed files:** src/vfs/s3.rs (tests)
- **Acceptance:** Fixture keys foo//bar.txt, foo/../bar.txt, foo/./bar.txt, space, Unicode, emoji, zero-byte, folder marker. Verify display may simplify presentation only; identity exact. Mocked OK.
- **Stop conditions:** AWS integration required (must allow mocked).
- **Hermes prompt:** Tests: awkward keys (foo//bar, foo/../bar, foo/./bar, spaces, Unicode, emoji, zero-byte, folder marker) preserve exact identity; display may simplify presentation only. Mocked.

### S3-23 — Enter S3BucketRef
- **Phase:** P10
- **Status:** BACKLOG
- **Depends on:** S3-18..22
- **Allowed files:** src/app/actions.rs, src/app/remote_workspace.rs (nav)
- **Acceptance:** Enter target root=>exact bucket via S3BucketRef (never Entry.name). Result Location::S3{same target, bucket exact, prefix ""}. No other nav changes.
- **Stop conditions:** Entry.name reconstruction.
- **Hermes prompt:** Nav: entering a bucket uses S3BucketRef => Location::S3{target, bucket exact, prefix ""}. Never Entry.name. No other nav changes.

### S3-24 — Enter S3PrefixRef
- **Phase:** P10
- **Status:** BACKLOG
- **Depends on:** S3-23
- **Allowed files:** src/app/actions.rs
- **Acceptance:** Navigate into CommonPrefix using exact S3PrefixRef. No string reconstruction from presentation name.
- **Stop conditions:** Name reconstruction.
- **Hermes prompt:** Nav into CommonPrefix uses exact S3PrefixRef. No presentation-name reconstruction.

### S3-25 — Virtual parent
- **Phase:** P10
- **Status:** BACKLOG
- **Depends on:** S3-23, S3-24
- **Allowed files:** src/app/actions.rs
- **Acceptance:** Virtual '..' for S3: prefix child=>parent nav prefix; bucket root=>target root only if not bucket-bound; bucket-bound root has no account escape. Virtual .. never an S3 object key. Tests.
- **Stop conditions:** Turning .. into object key.
- **Hermes prompt:** Implement virtual '..': prefix=>parent nav prefix; bucket root=>target root (unless bucket-bound). Virtual .. is nav only, never an S3 key. Tests.

### S3-26 — Enable List capability
- **Phase:** P10
- **Status:** BACKLOG
- **Depends on:** S3-18..25
- **Allowed files:** src/vfs/capabilities.rs, src/vfs/s3.rs
- **Acceptance:** Only now enable S3 List capability. Precondition S3-18..25 complete+tested. Change only capability declaration/tests. No Read/Write/Delete/Mkdir yet.
- **Stop conditions:** Enabling Read/Write/Delete/Mkdir.
- **Hermes prompt:** Flip S3 List capability ONLY after S3-18..25 done+tested. No other capability. Change capability decl/tests only.

### S3-27 — S3 bounded Range GET
- **Phase:** P11
- **Status:** BACKLOG
- **Depends on:** STOP GATE C
- **Allowed files:** src/vfs/s3.rs
- **Acceptance:** read_prefix_bytes for exact S3ObjectRef via GetObject Range: bytes=0..N. Respect bounded-read semantics. No full download. No F3 availability yet.
- **Stop conditions:** Full object download. F3 enable.
- **Hermes prompt:** Implement read_prefix_bytes for exact S3ObjectRef: GetObject Range bytes=0..N, bounded. No full download, no F3 enable.

### S3-28 — S3 bounded-read metadata
- **Phase:** P11
- **Status:** BACKLOG
- **Depends on:** S3-27
- **Allowed files:** src/vfs/s3.rs
- **Acceptance:** Return truthful BoundedRead: bytes read, truncated/full where provable. Do NOT invent POSIX mode/uid/gid. No RemoteEditRevision.
- **Stop conditions:** Inventing mode/uid/gid. RemoteEditRevision.
- **Hermes prompt:** Return BoundedRead for S3: bytes + truncated flag (truthful). Do NOT fake mode/uid/gid. No RemoteEditRevision.

### S3-29 — Enable S3 F3
- **Phase:** P11
- **Status:** BACKLOG
- **Depends on:** S3-28
- **Allowed files:** src/app/availability.rs
- **Acceptance:** Allow F3 for regular S3 object when Read implemented. Use exact S3ObjectRef. Reuse existing text/line limit, binary refusal, invalid UTF-8 handling. No F4.
- **Stop conditions:** Enabling F4.
- **Hermes prompt:** Enable F3 for S3 regular object via exact S3ObjectRef. Reuse existing preview limits/refusals. No F4.

### S3-30 — Enable Read capability
- **Phase:** P11
- **Status:** BACKLOG
- **Depends on:** S3-29
- **Allowed files:** src/vfs/capabilities.rs
- **Acceptance:** Flip S3 Read capability only after F3 passes. No Write.
- **Stop conditions:** Write enable.
- **Hermes prompt:** Flip S3 Read capability ONLY after S3-29 passes. No Write.

### S3-31 — TransferMethod::S3
- **Phase:** P12
- **Status:** BACKLOG
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
