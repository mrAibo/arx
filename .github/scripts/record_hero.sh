#!/usr/bin/env bash
set -euo pipefail

# HERO-REAL-02 — Record real terminal capture for ARX v0.15.1
# Requires: asciinema, agg
# Output: docs/assets/remote-workspace-update.gif

REPO_ROOT="$(git rev-parse --show-toplevel)"

ARX_HERO_HOST="${ARX_HERO_HOST:-arx-demo}"
ARX_HERO_LOCAL="${ARX_HERO_LOCAL:-$HOME/arx-demo/app}"
ARX_HERO_REMOTE="${ARX_HERO_REMOTE:-/tmp/arx-demo/app}"
ARX_HERO_BINARY="${ARX_HERO_BINARY:-$REPO_ROOT/dist/arx-v0.15.1-x86_64-unknown-linux-gnu/arx}"
ARX_HERO_CAST="${ARX_HERO_CAST:-$REPO_ROOT/.hero/remote-workspace-v0.15.1.cast}"
ARX_HERO_OUTPUT="${ARX_HERO_OUTPUT:-$REPO_ROOT/docs/assets/remote-workspace-update.gif}"

# Dependency preflight
command -v ssh >/dev/null || { echo "FAIL: ssh not found"; exit 1; }
command -v asciinema >/dev/null || { echo "FAIL: asciinema not found"; exit 1; }
command -v agg >/dev/null || { echo "FAIL: agg not found"; exit 1; }

# Binary gate
[ -x "$ARX_HERO_BINARY" ] || { echo "FAIL: binary not executable: $ARX_HERO_BINARY"; exit 1; }
"$ARX_HERO_BINARY" --version | grep -q "0.15.1" || { echo "FAIL: wrong binary version"; exit 1; }

# SSH preflight
echo "=== SSH PREFLIGHT ==="
ssh -o BatchMode=yes "$ARX_HERO_HOST" 'printf "ARX_DEMO_SSH_OK\n"' || { echo "FAIL: SSH fixture not ready"; exit 1; }
echo "SSH OK"

# Remote fixture creation (through SSH, not local mkdir)
echo "=== REMOTE FIXTURE ==="
ssh "$ARX_HERO_HOST" "mkdir -p '$ARX_HERO_REMOTE/src'"
ssh "$ARX_HERO_HOST" "echo '{\\\"name\\\":\\\"ARX Demo\\\",\\\"mode\\\":\\\"safe-update\\\"}' > '$ARX_HERO_REMOTE/manifest.json'"
ssh "$ARX_HERO_HOST" "echo 'fn main() {}' > '$ARX_HERO_REMOTE/src/main.rs'"
ssh "$ARX_HERO_HOST" "echo '[tool]' > '$ARX_HERO_REMOTE/config.toml'"

# Safety: reject cleanup unless host == arx-demo and remote path is correct
if [ "$ARX_HERO_HOST" != "arx-demo" ]; then
    echo "FAIL: cleanup only allowed for arx-demo host"
    exit 1
fi
if [[ ! "$ARX_HERO_REMOTE" =~ ^/tmp/arx-demo/ ]] || [ "$ARX_HERO_REMOTE" = "/tmp/arx-demo" ] || [[ "$ARX_HERO_REMOTE" == *".."* ]]; then
    echo "FAIL: remote path must be under /tmp/arx-demo/ and not be /tmp/arx-demo or contain .."
    exit 1
fi

# Local fixture
mkdir -p "$ARX_HERO_LOCAL/src"
echo '{"name":"ARX Demo","mode":"safe-update"}' > "$ARX_HERO_LOCAL/manifest.json"
echo 'fn main() {}' > "$ARX_HERO_LOCAL/src/main.rs"
echo '[tool]' > "$ARX_HERO_LOCAL/config.toml"

# Local-only differences (source-only, UPDATE mode)
echo "// local only" > "$ARX_HERO_LOCAL/local_change_1.rs"
echo "// local only" > "$ARX_HERO_LOCAL/local_change_2.rs"
echo "// local only" > "$ARX_HERO_LOCAL/local_change_3.rs"

# Clean artifacts
rm -f "$ARX_HERO_LOCAL"/.arx-bak-*
ssh "$ARX_HERO_HOST" "rm -f '$ARX_HERO_REMOTE'/.arx-bak-* 2>/dev/null || true"

# Convert cast → gif with agg
echo ""
echo "=== CONVERTING ==="
agg "$ARX_HERO_CAST" "$ARX_HERO_OUTPUT"
echo "GIF written to $ARX_HERO_OUTPUT"

echo "=== READY ==="
echo "Binary: $ARX_HERO_BINARY"
echo "Host: $ARX_HERO_HOST"
echo "Local: $ARX_HERO_LOCAL"
echo "Remote: $ARX_HERO_REMOTE"
echo ""
echo "Starting asciinema in 3 seconds..."
sleep 3

mkdir -p "$(dirname "$ARX_HERO_CAST")"
mkdir -p "$(dirname "$ARX_HERO_OUTPUT")"

# Remove existing cast file
rm -f "$ARX_HERO_CAST"

asciinema rec "$ARX_HERO_CAST" --command="$ARX_HERO_BINARY" --cols=120 --rows=30

echo ""
echo "=== RECORDING SAVED ==="
echo "Preview:   asciinema play $ARX_HERO_CAST"
echo "Convert:   agg $ARX_HERO_CAST $ARX_HERO_OUTPUT"