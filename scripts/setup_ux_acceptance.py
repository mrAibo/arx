#!/usr/bin/env python3
"""Create a disposable fixture for the ARX UX Acceptance journeys."""

from pathlib import Path
import argparse
import shutil
import tarfile
import zipfile


def build(root: Path) -> None:
    if root.exists():
        shutil.rmtree(root)
    source = root / "source"
    destination = root / "destination"
    nested = source / "nested"
    nested.mkdir(parents=True)
    destination.mkdir()
    files = {
        "README.md": "# ARX UX fixture\nArchive preview content: hello.\n",
        "empty.txt": "",
        "file with spaces.txt": "space value\n",
        "Юникод.txt": "Привет из ARX\n",
        "nested/child.txt": "nested value\n",
    }
    for relative, text in files.items():
        path = source / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
    members = sorted(files)
    with tarfile.open(source / "fixture.tar", "w") as archive:
        for member in members:
            archive.add(source / member, arcname=member, recursive=False)
    with tarfile.open(source / "fixture.tar.gz", "w:gz") as archive:
        for member in members:
            archive.add(source / member, arcname=member, recursive=False)
    with zipfile.ZipFile(source / "fixture.zip", "w", zipfile.ZIP_DEFLATED) as archive:
        for member in members:
            archive.write(source / member, member)
    print(root)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("root", nargs="?", type=Path, default=Path("/tmp/arx-ux-acceptance"))
    args = parser.parse_args()
    build(args.root.resolve())


if __name__ == "__main__":
    main()
