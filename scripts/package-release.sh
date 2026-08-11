#!/usr/bin/env bash
# package-release.sh — reproducible Linux release packaging
# Usage:  scripts/package-release.sh <version> <target> <release-binary> [output-dir]
#
# Example:
#   scripts/package-release.sh 0.15.0 x86_64-unknown-linux-gnu target/release/arx dist

set -euo pipefail

VERSION="${1:?version required}"
TARGET="${2:?target required}"
BINARY="${3:?release binary path required}"
OUTDIR="${4:-dist}"

ARCHIVE_NAME="arx-v${VERSION}-${TARGET}"
ARCHIVE_FILE="${ARCHIVE_NAME}.tar.gz"
SUMS_FILE="SHA256SUMS"

# ── preflight ──────────────────────────────────────────────
[ -f "$BINARY" ]           || { echo "FAIL: binary not found: $BINARY"; exit 1; }
[ -f README.md ]           || { echo "FAIL: README.md not found";  exit 1; }
[ -f LICENSE ]             || { echo "FAIL: LICENSE not found";    exit 1; }
[ -x "$BINARY" ]           || { echo "FAIL: binary not executable: $BINARY"; exit 1; }

# ── build ──────────────────────────────────────────────────
rm -rf  "${OUTDIR:?}/${ARCHIVE_NAME}" "${OUTDIR:?}/${ARCHIVE_FILE}" "${OUTDIR:?}/${SUMS_FILE}"
mkdir -p "$OUTDIR/$ARCHIVE_NAME"

cp "$BINARY"    "$OUTDIR/$ARCHIVE_NAME/arx"
cp README.md    "$OUTDIR/$ARCHIVE_NAME/README.md"
cp LICENSE      "$OUTDIR/$ARCHIVE_NAME/LICENSE"
chmod 755       "$OUTDIR/$ARCHIVE_NAME/arx"

tar czf "$OUTDIR/$ARCHIVE_FILE" -C "$OUTDIR" "$ARCHIVE_NAME"
rm -rf "$OUTDIR/$ARCHIVE_NAME"

# ── checksum ───────────────────────────────────────────────
( cd "$OUTDIR" && sha256sum "$ARCHIVE_FILE" > "$SUMS_FILE" )

# ── verify ─────────────────────────────────────────────────
echo "=== archive contents ==="
tar tzf "$OUTDIR/$ARCHIVE_FILE"

echo "=== checksum ==="
( cd "$OUTDIR" && sha256sum -c "$SUMS_FILE" )

echo "=== ok: $OUTDIR/$ARCHIVE_FILE ==="
