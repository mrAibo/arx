from pathlib import Path
import subprocess

SOURCE = "4a4c96a29c3f9d5d76dd13876b5be740a39bb6c5"
FILES = ["README.md", "ROADMAP.md", "docs/DEVELOPMENT_HANDOFF.md"]

for path in FILES:
    data = subprocess.check_output(["git", "show", f"{SOURCE}:{path}"])
    Path(path).write_bytes(data)


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected exactly one anchor, found {count}: {old!r}")
    p.write_text(text.replace(old, new, 1))

# README: current release notes must point at the current release.
replace_once(
    "README.md",
    "- [docs/releases/v0.23.0.md](docs/releases/v0.23.0.md) — v0.23.0 release notes",
    "- [docs/releases/v0.24.0.md](docs/releases/v0.24.0.md) — v0.24.0 release notes",
)

# ROADMAP / Handoff: recursive copy is the next slice; Move remains a later, separately proven semantic.
replace_once(
    "ROADMAP.md",
    "- [ ] WebDAV→WebDAV recursive/cross-target copy or move with truthful target/recovery semantics",
    "- [ ] WebDAV→WebDAV recursive/cross-target copy with exact source/target identity and truthful staging/recovery semantics\n- [ ] WebDAV→WebDAV move only after copy → verify → delete-source semantics are separately proven",
)
replace_once(
    "docs/DEVELOPMENT_HANDOFF.md",
    "- WebDAV→WebDAV recursive/cross-target copy or move",
    "- WebDAV→WebDAV recursive/cross-target copy with exact source/target identity and truthful staging/recovery semantics\n- WebDAV→WebDAV move only after copy → verify → delete-source semantics are separately proven",
)

old_prompt = "> Continue development of `github.com/mrAibo/arx`. Read `docs/DEVELOPMENT_HANDOFF.md`, `ROADMAP.md`, and `ARCHITECTURE.md`; treat live GitHub state as authoritative. Current public release is v0.23.0 at `f66a25f3f2b4fb66832ecc50d85f9f105ebba086`, shipping the accepted S3 Object & Bucket Inspector. Preserve frozen architecture authorities. The next major slice is SFTP→SFTP workspace synchronization: freeze its dedicated issue/contract around the existing Compare → Preview → Execute → Verify and Transfer Queue authorities before implementation. Use Hermes only for deterministic Linux-local execution."
new_prompt = "> Continue development of `github.com/mrAibo/arx`. Read `docs/DEVELOPMENT_HANDOFF.md`, `ROADMAP.md`, and `ARCHITECTURE.md`; treat live GitHub state as authoritative. Current public release is v0.24.0 at tag target `6d413fac5d5b493859bfadfbedbeb436b1140e0b`, shipping SFTP→SFTP Workspace Sync and retaining the v0.23.0 S3 Inspector plus released WebDAV recursive operations. Preserve frozen architecture authorities. The next recommended major slice is WebDAV→WebDAV recursive/cross-target copy under issue #13: freeze exact source/target identity, copy semantics, confirmation, cancellation, staged destination and recovery truth before implementation. Keep Move as a separate later copy → verify → delete-source decision. Use Hermes only for deterministic Linux-local execution."
replace_once("docs/DEVELOPMENT_HANDOFF.md", old_prompt, new_prompt)

readme = Path("README.md").read_text()
roadmap = Path("ROADMAP.md").read_text()
handoff = Path("docs/DEVELOPMENT_HANDOFF.md").read_text()

assert "**Current release: [v0.24.0]" in readme
assert "docs/releases/v0.24.0.md" in readme
assert "Release candidate in source" not in readme
assert "**Current public release:** `v0.24.0`" in roadmap
assert "## RELEASED — SFTP → SFTP WORKSPACE SYNC" in roadmap
assert "WebDAV→WebDAV recursive/cross-target copy with exact source/target identity" in roadmap
assert "Current public release: **v0.24.0**" in handoff
assert "Current public release is v0.24.0" in handoff
assert "The next major slice is SFTP→SFTP" not in handoff
assert "WebDAV→WebDAV recursive/cross-target copy with exact source/target identity" in handoff
assert "33012256020" in handoff

print("corrected v0.24.0 canonical truth staged")
