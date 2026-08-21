#!/usr/bin/env bash
# package-release.sh — Linux release packaging from one validated binary
# Usage: scripts/package-release.sh <version> <target> <release-binary> [output-dir]

set -euo pipefail

VERSION="${1:?version required}"
TARGET="${2:?target required}"
BINARY="${3:?release binary path required}"
OUTDIR="${4:-dist}"
THIRD_PARTY="${THIRD_PARTY_LICENSES:-THIRD_PARTY_LICENSES.html}"

ARCHIVE_NAME="arx-v${VERSION}-${TARGET}"
ARCHIVE_FILE="${ARCHIVE_NAME}.tar.gz"
DEB_FILE="arx_${VERSION}_amd64.deb"
RPM_FILE="arx-${VERSION}-1.x86_64.rpm"
SUMS_FILE="SHA256SUMS"

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

[ "$TARGET" = "x86_64-unknown-linux-gnu" ] || fail "unsupported release target: $TARGET"
[ -f "$BINARY" ] || fail "binary not found: $BINARY"
[ -x "$BINARY" ] || fail "binary not executable: $BINARY"
[ -f README.md ] || fail "README.md not found"
[ -f LICENSE ] || fail "LICENSE not found"
[ -s "$THIRD_PARTY" ] || fail "third-party license report missing/empty: $THIRD_PARTY"
[ -f packaging/arx.spec ] || fail "RPM spec not found: packaging/arx.spec"
command -v dpkg-deb >/dev/null || fail "dpkg-deb is required"
command -v rpmbuild >/dev/null || fail "rpmbuild is required"
command -v rpm >/dev/null || fail "rpm is required"

BINARY_ABS="$(readlink -f "$BINARY")"
README_ABS="$(readlink -f README.md)"
LICENSE_ABS="$(readlink -f LICENSE)"
THIRD_PARTY_ABS="$(readlink -f "$THIRD_PARTY")"
OUTDIR_ABS="$(mkdir -p "$OUTDIR" && cd "$OUTDIR" && pwd)"
SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct 2>/dev/null || date +%s)}"
export SOURCE_DATE_EPOCH

rm -rf \
    "$OUTDIR_ABS/$ARCHIVE_NAME" \
    "$OUTDIR_ABS/$ARCHIVE_FILE" \
    "$OUTDIR_ABS/$DEB_FILE" \
    "$OUTDIR_ABS/$RPM_FILE" \
    "$OUTDIR_ABS/$SUMS_FILE" \
    "$OUTDIR_ABS/.deb-root" \
    "$OUTDIR_ABS/.rpmbuild"

# tar.gz
mkdir -p "$OUTDIR_ABS/$ARCHIVE_NAME"
install -m 0755 "$BINARY_ABS" "$OUTDIR_ABS/$ARCHIVE_NAME/arx"
install -m 0644 "$README_ABS" "$OUTDIR_ABS/$ARCHIVE_NAME/README.md"
install -m 0644 "$LICENSE_ABS" "$OUTDIR_ABS/$ARCHIVE_NAME/LICENSE"
install -m 0644 "$THIRD_PARTY_ABS" "$OUTDIR_ABS/$ARCHIVE_NAME/THIRD_PARTY_LICENSES.html"
tar \
    --sort=name \
    --mtime="@${SOURCE_DATE_EPOCH}" \
    --owner=0 \
    --group=0 \
    --numeric-owner \
    -C "$OUTDIR_ABS" \
    -cf - "$ARCHIVE_NAME" \
    | gzip -n > "$OUTDIR_ABS/$ARCHIVE_FILE"
rm -rf "$OUTDIR_ABS/$ARCHIVE_NAME"

# Debian package
DEB_ROOT="$OUTDIR_ABS/.deb-root"
mkdir -p "$DEB_ROOT/DEBIAN" "$DEB_ROOT/usr/bin" "$DEB_ROOT/usr/share/doc/arx"
install -m 0755 "$BINARY_ABS" "$DEB_ROOT/usr/bin/arx"
install -m 0644 "$README_ABS" "$DEB_ROOT/usr/share/doc/arx/README.md"
install -m 0644 "$LICENSE_ABS" "$DEB_ROOT/usr/share/doc/arx/copyright"
install -m 0644 "$THIRD_PARTY_ABS" "$DEB_ROOT/usr/share/doc/arx/THIRD_PARTY_LICENSES.html"
INSTALLED_SIZE="$(du -sk "$DEB_ROOT/usr" | awk '{print $1}')"
cat > "$DEB_ROOT/DEBIAN/control" <<EOF
Package: arx
Version: $VERSION
Section: utils
Priority: optional
Architecture: amd64
Installed-Size: $INSTALLED_SIZE
Maintainer: ARX maintainers <noreply@example.invalid>
Depends: libc6 (>= 2.35), libgcc-s1
Homepage: https://github.com/mrAibo/arx
Description: Terminal commander for local and remote workspaces
 ARX is a Linux terminal commander for local and remote workspaces with
 truthful jobs, transfer controls, storage inspection, SFTP, S3 and WebDAV.
EOF
dpkg-deb --root-owner-group --build "$DEB_ROOT" "$OUTDIR_ABS/$DEB_FILE" >/dev/null
rm -rf "$DEB_ROOT"

# RPM package
RPM_TOP="$OUTDIR_ABS/.rpmbuild"
mkdir -p "$RPM_TOP"/{BUILD,BUILDROOT,RPMS,SOURCES,SPECS,SRPMS}
rpmbuild -bb packaging/arx.spec \
    --define "_topdir $RPM_TOP" \
    --define "arx_version $VERSION" \
    --define "arx_binary $BINARY_ABS" \
    --define "arx_readme $README_ABS" \
    --define "arx_license $LICENSE_ABS" \
    --define "arx_third_party $THIRD_PARTY_ABS" \
    >/dev/null
RPM_BUILT="$(find "$RPM_TOP/RPMS" -type f -name "$RPM_FILE" -print -quit)"
[ -n "$RPM_BUILT" ] || fail "expected RPM not produced: $RPM_FILE"
cp "$RPM_BUILT" "$OUTDIR_ABS/$RPM_FILE"
rm -rf "$RPM_TOP"

# checksums + structural verification
(
    cd "$OUTDIR_ABS"
    sha256sum "$ARCHIVE_FILE" "$DEB_FILE" "$RPM_FILE" > "$SUMS_FILE"
    sha256sum -c "$SUMS_FILE"
)

echo "=== tar contents ==="
tar tzf "$OUTDIR_ABS/$ARCHIVE_FILE"
echo "=== deb contents ==="
dpkg-deb --contents "$OUTDIR_ABS/$DEB_FILE"
echo "=== rpm contents ==="
rpm -qpl "$OUTDIR_ABS/$RPM_FILE"
echo "=== ok: Linux release artifacts in $OUTDIR_ABS ==="
