#!/usr/bin/env bash
# shellcheck disable=SC2029
set -euo pipefail

# HERO safety recorder for ARX v0.15.1
# Requires: ssh, asciinema, agg
# Output: docs/assets/remote-workspace-update.gif
#
# Order is fail-closed: validate host + remote path BEFORE any mutation,
# record BEFORE convert, and never destroy the existing GIF unless the new
# conversion succeeds.

REPO_ROOT="$(git rev-parse --show-toplevel)"

ARX_HERO_HOST="${ARX_HERO_HOST:-arx-demo}"
ARX_HERO_LOCAL="${ARX_HERO_LOCAL:-$HOME/arx-demo/app}"
ARX_HERO_REMOTE="${ARX_HERO_REMOTE:-/tmp/arx-demo/app}"
ARX_HERO_BINARY="${ARX_HERO_BINARY:-$REPO_ROOT/dist/arx-v0.15.1-x86_64-unknown-linux-gnu/arx}"
ARX_HERO_CAST="${ARX_HERO_CAST:-$REPO_ROOT/.hero/remote-workspace-v0.15.1.cast}"
ARX_HERO_OUTPUT="${ARX_HERO_OUTPUT:-$REPO_ROOT/docs/assets/remote-workspace-update.gif}"

# 1. Dependency preflight
command -v ssh >/dev/null || { echo "FAIL: ssh not found"; exit 1; }
command -v asciinema >/dev/null || { echo "FAIL: asciinema not found"; exit 1; }
command -v agg >/dev/null || { echo "FAIL: agg not found"; exit 1; }

# 2. Binary gate
[ -x "$ARX_HERO_BINARY" ] || { echo "FAIL: binary not executable: $ARX_HERO_BINARY"; exit 1; }
"$ARX_HERO_BINARY" --version | grep -q "0.15.1" || { echo "FAIL: wrong binary version"; exit 1; }

# 3. Host validation (fail-closed: only arx-demo may receive destructive fixture)
[ "$ARX_HERO_HOST" = "arx-demo" ] || {
    echo "FAIL: destructive fixture creation only allowed for arx-demo host"; exit 1; }

# 4. Remote path validation (under /tmp/arx-demo/, non-empty, no traversal)
case "$ARX_HERO_REMOTE" in
    /tmp/arx-demo/*) ;;
    *) echo "FAIL: remote path must be under /tmp/arx-demo/"; exit 1 ;;
esac
[ "$ARX_HERO_REMOTE" = "/tmp/arx-demo" ] && {
    echo "FAIL: remote path must not equal /tmp/arx-demo"; exit 1; }
case "$ARX_HERO_REMOTE" in
    *".."*) echo "FAIL: remote path must not contain traversal components"; exit 1 ;;
esac

# 5. SSH connectivity preflight
echo "=== SSH PREFLIGHT ==="
ssh -o BatchMode=yes "$ARX_HERO_HOST" 'printf "ARX_DEMO_SSH_OK\n"' || {
    echo "FAIL: SSH fixture not ready"; exit 1; }
echo "SSH OK"

# 6. Fixture mutation (local + remote) — only after all validation passed
echo "=== REMOTE FIXTURE ==="
ssh "$ARX_HERO_HOST" "mkdir -p '$ARX_HERO_REMOTE/src'"
ssh "$ARX_HERO_HOST" "echo '{\"name\":\"ARX Demo\",\"mode\":\"safe-update\"}' > '$ARX_HERO_REMOTE/manifest.json'"
ssh "$ARX_HERO_HOST" "echo 'fn main() {}' > '$ARX_HERO_REMOTE/src/main.rs'"
ssh "$ARX_HERO_HOST" "echo '[tool]' > '$ARX_HERO_REMOTE/config.toml'"

echo "=== LOCAL FIXTURE ==="
mkdir -p "$ARX_HERO_LOCAL/src"
echo '{"name":"ARX Demo","mode":"safe-update"}' > "$ARX_HERO_LOCAL/manifest.json"
echo 'fn main() {}' > "$ARX_HERO_LOCAL/src/main.rs"
echo '[tool]' > "$ARX_HERO_LOCAL/config.toml"

# Local-only differences (source-only, UPDATE mode)
echo "// local only" > "$ARX_HERO_LOCAL/local_change_1.rs"
echo "// local only" > "$ARX_HERO_LOCAL/local_change_2.rs"
echo "// local only" > "$ARX_HERO_LOCAL/local_change_3.rs"

# Clean stale artifacts
rm -f "$ARX_HERO_LOCAL"/.arx-bak-*
ssh "$ARX_HERO_HOST" "rm -f '$ARX_HERO_REMOTE'/.arx-bak-* 2>/dev/null || true"

# 7. Record
mkdir -p "$(dirname "$ARX_HERO_CAST")"
mkdir -p "$(dirname "$ARX_HERO_OUTPUT")"
rm -f "$ARX_HERO_CAST"

echo "=== READY ==="
echo "Binary: $ARX_HERO_BINARY"
echo "Host: $ARX_HERO_HOST"
echo "Local: $ARX_HERO_LOCAL"
echo "Remote: $ARX_HERO_REMOTE"
echo ""
echo "Starting asciinema in 3 seconds..."
sleep 3

asciinema rec "$ARX_HERO_CAST" --command="$ARX_HERO_BINARY" --cols=120 --rows=30

# 8. Convert — only if recording produced a non-empty cast
test -s "$ARX_HERO_CAST" || { echo "FAIL: cast missing or empty after recording"; exit 1; }

echo ""
echo "=== CONVERTING ==="
NEW_GIF="${ARX_HERO_OUTPUT}.new"
rm -f "$NEW_GIF"
agg "$ARX_HERO_CAST" "$NEW_GIF"
test -s "$NEW_GIF" || { echo "FAIL: conversion produced empty GIF"; exit 1; }
mv -f "$NEW_GIF" "$ARX_HERO_OUTPUT"

echo "GIF written to $ARX_HERO_OUTPUT"
