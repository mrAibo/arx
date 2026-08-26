from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected exactly one anchor, found {count}: {old[:120]!r}")
    p.write_text(text.replace(old, new, 1))


replace_once(
    "README.md",
    "Remote Workspace synchronization currently supports Local → Local, Local → SFTP, and SFTP → Local. SFTP → SFTP workspace synchronization remains intentionally unsupported.\n\n**v0.23.0 ships the read-only S3 Object & Bucket Inspector**",
    "Published v0.23.0 supports Local → Local, Local → SFTP, and SFTP → Local Workspace Sync. Current `main` additionally contains accepted, unreleased **SFTP → SFTP Workspace Sync** from [#269](https://github.com/mrAibo/arx/issues/269) / [#270](https://github.com/mrAibo/arx/pull/270), including same-host and cross-host bounded remote → ARX → remote streaming with real two-endpoint OpenSSH acceptance.\n\n**v0.23.0 ships the read-only S3 Object & Bucket Inspector**",
)
replace_once(
    "README.md",
    "| Workspace Sync | Compare → Preview → Execute → Verify for supported Local/SFTP directions |",
    "| Workspace Sync | Compare → Preview → Execute → Verify; v0.23.0 covers Local/Local↔SFTP, while current `main` additionally contains accepted SFTP→SFTP sync (unreleased) |",
)

replace_once(
    "ROADMAP.md",
    "## CURRENT — v0.23.0 published\n",
    "## CURRENT — v0.23.0 published / SFTP→SFTP accepted on main\n",
)
replace_once(
    "ROADMAP.md",
    "**Previous immutable release:** `v0.22.0` → `8737bbd2afaf0d6e7146a5d8c59ee1a0606325bf`\n\nv0.23.0 ships",
    "**Previous immutable release:** `v0.22.0` → `8737bbd2afaf0d6e7146a5d8c59ee1a0606325bf`  \n**Current main:** `fe413aecfdc3bf5685849e73b396800f7f3ab7e0` — accepted/unreleased SFTP→SFTP Workspace Sync (#269 / PR #270)\n\nv0.23.0 ships",
)
replace_once(
    "ROADMAP.md",
    "- **Remote Workspace:** Compare → Preview → Execute → Verify for Local→Local, Local→SFTP, and SFTP→Local. SFTP→SFTP workspace sync remains unsupported in v0.23.0.",
    "- **Remote Workspace:** published v0.23.0 provides Compare → Preview → Execute → Verify for Local→Local, Local→SFTP, and SFTP→Local; current `main` additionally contains accepted/unreleased SFTP→SFTP sync with same-host/cross-host bounded streaming and real two-endpoint OpenSSH acceptance.",
)
replace_once(
    "ROADMAP.md",
    "## SELECTED NEXT PRODUCT DIRECTION — SFTP → SFTP WORKSPACE SYNC\n\nThe next major feature is **SFTP → SFTP workspace synchronization**. Freeze a dedicated issue/contract before implementation and extend the existing Compare → Preview → Execute → Verify model rather than creating a second synchronization engine.\n\nRequired boundaries:\n\n- exact source and destination SFTP host/path identities\n- explicit same-host vs cross-host execution truth\n- reuse of the existing workspace-sync controller, `ProviderRegistry`, Transfer Queue, `JobManager`, retry policy, and verification model\n- frozen Preview before destructive Mirror consequences\n- deterministic copy/mutation ordering\n- real destination verification after execution\n- cooperative cancellation and truthful partial completion\n- explicit ambiguity/recovery boundaries\n- never treat a cross-host transfer as server-side rename/move\n- do not bundle general cross-provider Move or WebDAV→WebDAV recursive copy into this slice\n\n## RECOMMENDED FEATURE SEQUENCE\n\n1. **Next:** SFTP → SFTP workspace sync.\n2. **After that:** WebDAV → WebDAV recursive copy, only after exact source/target and recovery semantics are frozen.\n3. **Later:** general safe cross-provider Move modeled as copy → verify → delete-source.\n4. Other candidates: binary remote editing, additional Linux architectures, signed repositories, provider-specific read-only analytics where evidence is truthful.\n",
    "## ACCEPTED / UNRELEASED — SFTP → SFTP WORKSPACE SYNC\n\nIssue #269 was completed by PR #270 and is now accepted on `main` at `fe413aecfdc3bf5685849e73b396800f7f3ab7e0`. It is **not part of published v0.23.0**.\n\nAccepted truth:\n\n- exact source and destination SFTP host/path identities\n- explicit same-host and cross-host execution truth\n- bounded remote → ARX → remote file streaming through the existing SFTP transfer authority\n- no server-side copy/rename fiction and no SFTP→SFTP Move expansion\n- existing workspace-sync controller, `ProviderRegistry`, Job lifecycle, retry/recovery model, journal, frozen Preview and post-execution verification retained\n- SFTP mkdir/delete routed through existing provider mutation seams\n- stale source/destination fail closed before mutation\n- cancellation and partial/recovery truth retained\n- permanent two-endpoint OpenSSH physical lane with strict host-key checking\n- exact feature head `9656102bc679b71ca49513b37735bc79a3874a91`; squash merge `fe413aecfdc3bf5685849e73b396800f7f3ab7e0`\n- exact-head CI / WebDAV interop / SFTP physical all success; post-merge CI run `33002995697` and post-merge SFTP physical run `33002995516` success\n\nThe focused release package is tracked in #271 and targets **v0.24.0** without another major feature.\n\n## RECOMMENDED FEATURE SEQUENCE\n\n1. **Next:** publish focused v0.24.0 with the already accepted SFTP→SFTP Workspace Sync.\n2. **After v0.24.0:** WebDAV → WebDAV recursive copy, only after exact source/target and recovery semantics are frozen.\n3. **Later:** general safe cross-provider Move modeled as copy → verify → delete-source.\n4. Other candidates: binary remote editing, additional Linux architectures, signed repositories, provider-specific read-only analytics where evidence is truthful.\n",
)

replace_once(
    "docs/DEVELOPMENT_HANDOFF.md",
    "- Previous immutable release: **v0.22.0** → `8737bbd2afaf0d6e7146a5d8c59ee1a0606325bf`\n- Rust MSRV:",
    "- Previous immutable release: **v0.22.0** → `8737bbd2afaf0d6e7146a5d8c59ee1a0606325bf`\n- Current main: `fe413aecfdc3bf5685849e73b396800f7f3ab7e0` — accepted/unreleased SFTP→SFTP Workspace Sync (#269 / PR #270)\n- Rust MSRV:",
)
replace_once(
    "docs/DEVELOPMENT_HANDOFF.md",
    "## 2. Current phase\n\nThe release phase is complete. The next phase is **contract freeze for SFTP → SFTP workspace synchronization**.\n\nDo not start implementation by casually extending transfer code. First create a dedicated issue that freezes identity, execution, verification, cancellation, and recovery semantics around the existing Compare → Preview → Execute → Verify model.\n\nCurrent sequence:\n\n1. keep v0.23.0 release/tag immutable\n2. keep public docs and live GitHub state aligned\n3. freeze a dedicated SFTP→SFTP workspace-sync issue\n4. inspect existing workspace-sync controller, Transfer Queue, provider authority, retry, verification, and tests before designing changes\n5. implement one coherent feature slice on one branch\n6. require exact-head CI plus affected physical SFTP evidence before merge\n",
    "## 2. Current phase\n\nSFTP → SFTP Workspace Sync has completed its feature cycle and is accepted on `main`; it remains **unreleased relative to v0.23.0**. The active phase is the focused **v0.24.0 release package**, tracked in #271.\n\nCurrent sequence:\n\n1. keep v0.23.0 and all older release tags immutable\n2. synchronize public truth: v0.23.0 is published; SFTP→SFTP is accepted/unreleased on current `main`\n3. prepare v0.24.0 as release-truth/version metadata only; add no new runtime feature\n4. require one exact release-candidate head to pass standard CI, Rust 1.88, SFTP physical, WebDAV interoperability and Release validation\n5. pinned squash merge using the reviewed head, then require post-merge CI and SFTP physical success\n6. create immutable v0.24.0 tag on the accepted release commit, publish through the existing Release workflow, and independently verify packages/checksums/binary truth\n7. close #271 only after canonical docs and cleanup reflect published v0.24.0\n",
)
replace_once(
    "docs/DEVELOPMENT_HANDOFF.md",
    "Compare → Preview → Execute → Verify currently supports:\n\n- Local → Local\n- Local → SFTP\n- SFTP → Local\n\nSFTP → SFTP remains intentionally unsupported in v0.23.0 and is the selected next major feature.",
    "Published v0.23.0 supports Compare → Preview → Execute → Verify for:\n\n- Local → Local\n- Local → SFTP\n- SFTP → Local\n\nCurrent `main` additionally contains accepted/unreleased **SFTP → SFTP** Workspace Sync, for both same-host different-root and cross-host roots, using bounded remote → ARX → remote streaming and the existing verification/recovery authorities.",
)
replace_once(
    "docs/DEVELOPMENT_HANDOFF.md",
    "## 6. Next feature contract — SFTP → SFTP workspace sync\n\nFreeze a dedicated issue before implementation.\n\nThe feature must extend the existing Compare → Preview → Execute → Verify architecture, not create another synchronization engine.\n\nRequired design boundaries:\n\n- preserve exact source and destination SFTP host/path identities\n- make same-host vs cross-host execution explicit\n- reuse the existing workspace-sync controller, `ProviderRegistry`, Transfer Queue, `JobManager`, retry policy, and verification model\n- freeze Preview before destructive Mirror consequences\n- keep transfer/mutation ordering deterministic\n- verify the real destination after execution rather than treating transfer completion as synchronization proof\n- preserve cooperative cancellation and truthful partial completion\n- define ambiguity/recovery boundaries explicitly\n- never pretend a cross-host transfer is server-side rename/move\n- do not bundle general cross-provider Move or WebDAV→WebDAV recursive copy into this slice\n\nQuestions the issue must answer before implementation:\n\n1. What exact typed identity represents source and destination SFTP roots?\n2. Which operations differ for same-host and cross-host cases?\n3. How are conflicts and Mirror deletions represented in frozen Preview?\n4. What deterministic order is used for copy/create/delete actions?\n5. What destination evidence constitutes successful verification?\n6. What state is reported after cancellation or partial completion?\n7. Which failures are retryable, ambiguous, or recovery-required?\n8. What real SFTP fixture/evidence is required for acceptance?\n\n## 7. Later candidates\n\nAfter SFTP→SFTP:\n",
    "## 6. Accepted/unreleased feature — SFTP → SFTP workspace sync\n\nIssue #269 / PR #270 completed the frozen contract without introducing another synchronization engine.\n\nAccepted implementation truth:\n\n- exact source and destination SFTP host/path identities remain authoritative\n- same-host and cross-host are explicit and both stream bounded remote → ARX → remote\n- no server-side copy/rename fiction and no general Move expansion\n- existing workspace-sync controller, `ProviderRegistry`, Transfer Queue/Job lifecycle, retry/recovery authority, journal and verification model are reused\n- SFTP directory creation and deletion use existing Registry/provider mutation seams\n- frozen Preview, confirmation, stale validation, deterministic ordering, cancellation and post-execution verification are retained\n- source/destination precommit failures are classified truthfully; ambiguous mutations are not blindly replayed\n- permanent two-endpoint OpenSSH physical acceptance uses strict host-key checking\n\nEvidence:\n\n- exact feature head: `9656102bc679b71ca49513b37735bc79a3874a91`\n- squash merge / current feature main: `fe413aecfdc3bf5685849e73b396800f7f3ab7e0`\n- accepted tree: `57463d53718fd1cdd2b838dbee5524d99a9de59c`\n- post-merge CI: run `33002995697` — success\n- post-merge SFTP physical: run `33002995516` — success\n- issue #269: completed\n\nRelease decision: publish this feature as focused **v0.24.0**, tracked in #271, before starting another major feature.\n\n## 7. Later candidates\n\nAfter v0.24.0:\n",
)

print("public truth sync anchors applied")
