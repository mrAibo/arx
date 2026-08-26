from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected exactly one anchor, found {count}: {old[:140]!r}")
    p.write_text(text.replace(old, new, 1))


replace_once(
    "Cargo.toml",
    'name = "arx"\nversion = "0.23.0"',
    'name = "arx"\nversion = "0.24.0"',
)

replace_once(
    "Cargo.lock",
    '[[package]]\nname = "arx"\nversion = "0.23.0"\ndependencies = [',
    '[[package]]\nname = "arx"\nversion = "0.24.0"\ndependencies = [',
)

replace_once(
    "README.md",
    "**Current release: [v0.23.0](https://github.com/mrAibo/arx/releases/tag/v0.23.0)**  \nLinux x86_64 · Rust MSRV 1.88 · MIT",
    "**Current release: [v0.23.0](https://github.com/mrAibo/arx/releases/tag/v0.23.0)**  \n**Release candidate in source:** `v0.24.0` — not published until the immutable tag and GitHub Release exist.\nLinux x86_64 · Rust MSRV 1.88 · MIT",
)

replace_once(
    "ROADMAP.md",
    "## CURRENT — v0.23.0 published / SFTP→SFTP accepted on main",
    "## CURRENT — v0.23.0 published / v0.24.0 release preparation",
)
replace_once(
    "ROADMAP.md",
    "**Current main:** `fe413aecfdc3bf5685849e73b396800f7f3ab7e0` — accepted/unreleased SFTP→SFTP Workspace Sync (#269 / PR #270)",
    "**Current main:** `cd5f1b147ac4ca78d7fdab134d548b97a7c20a00` — public-truth baseline after PR #272\n**Active release candidate:** `v0.24.0` via #271 / `release/v0.24.0-prep`; source version metadata does not imply publication",
)
replace_once(
    "ROADMAP.md",
    "- **Remote Workspace:** published v0.23.0 provides Compare → Preview → Execute → Verify for Local→Local, Local→SFTP, and SFTP→Local; current `main` additionally contains accepted/unreleased SFTP→SFTP sync with same-host/cross-host bounded streaming and real two-endpoint OpenSSH acceptance.",
    "- **Remote Workspace:** published v0.23.0 provides Compare → Preview → Execute → Verify for Local→Local, Local→SFTP, and SFTP→Local; the v0.24.0 release candidate adds the already accepted SFTP→SFTP sync with same-host/cross-host bounded streaming and real two-endpoint OpenSSH acceptance.",
)
replace_once(
    "ROADMAP.md",
    "Issue #269 was completed by PR #270 and is now accepted on `main` at `fe413aecfdc3bf5685849e73b396800f7f3ab7e0`. It is **not part of published v0.23.0**.",
    "Issue #269 was completed by PR #270 and feature-merged as `fe413aecfdc3bf5685849e73b396800f7f3ab7e0`. Public-truth baseline `cd5f1b147ac4ca78d7fdab134d548b97a7c20a00` retains the feature. It is **not part of published v0.23.0** and is the sole major feature targeted by the v0.24.0 release candidate.",
)

replace_once(
    "docs/DEVELOPMENT_HANDOFF.md",
    "- Current main: `fe413aecfdc3bf5685849e73b396800f7f3ab7e0` — accepted/unreleased SFTP→SFTP Workspace Sync (#269 / PR #270)",
    "- Current main: `cd5f1b147ac4ca78d7fdab134d548b97a7c20a00` — public-truth baseline after PR #272\n- Active release candidate: **v0.24.0** on `release/v0.24.0-prep` via #271; source version metadata is not publication",
)
replace_once(
    "docs/DEVELOPMENT_HANDOFF.md",
    "SFTP → SFTP Workspace Sync has completed its feature cycle and is accepted on `main`; it remains **unreleased relative to v0.23.0**. The active phase is the focused **v0.24.0 release package**, tracked in #271.",
    "SFTP → SFTP Workspace Sync has completed its feature cycle and is accepted on `main`; it remains **unreleased relative to v0.23.0**. Public-truth sync Task A is complete at `cd5f1b147ac4ca78d7fdab134d548b97a7c20a00`. The active phase is freezing the focused **v0.24.0 release candidate** on `release/v0.24.0-prep`, tracked in #271, with no new runtime feature.",
)
replace_once(
    "docs/DEVELOPMENT_HANDOFF.md",
    "2. synchronize public truth: v0.23.0 is published; SFTP→SFTP is accepted/unreleased on current `main`\n3. prepare v0.24.0 as release-truth/version metadata only; add no new runtime feature\n4. require one exact release-candidate head to pass standard CI, Rust 1.88, SFTP physical, WebDAV interoperability and Release validation",
    "2. public-truth sync is complete: PR #272 → `cd5f1b147ac4ca78d7fdab134d548b97a7c20a00`, with post-merge CI and SFTP physical success\n3. freeze v0.24.0 release-truth/version metadata only; add no new runtime feature\n4. require the resulting exact release-candidate head to pass standard CI, Rust 1.88, SFTP physical, WebDAV interoperability and Release validation",
)
replace_once(
    "docs/DEVELOPMENT_HANDOFF.md",
    "- squash merge / current feature main: `fe413aecfdc3bf5685849e73b396800f7f3ab7e0`\n- accepted tree: `57463d53718fd1cdd2b838dbee5524d99a9de59c`\n- post-merge CI: run `33002995697` — success\n- post-merge SFTP physical: run `33002995516` — success",
    "- feature squash merge: `fe413aecfdc3bf5685849e73b396800f7f3ab7e0`\n- accepted feature tree: `57463d53718fd1cdd2b838dbee5524d99a9de59c`\n- feature post-merge CI: run `33002995697` — success\n- feature post-merge SFTP physical: run `33002995516` — success\n- public-truth baseline: `cd5f1b147ac4ca78d7fdab134d548b97a7c20a00`\n- public-truth post-merge CI: run `33007674598` — success\n- public-truth post-merge SFTP physical: run `33007674646` — success",
)

notes = Path("docs/releases/v0.24.0.md")
if not notes.is_file() or notes.stat().st_size == 0:
    raise SystemExit("docs/releases/v0.24.0.md missing or empty")

print("v0.24.0 release prep anchors applied")
