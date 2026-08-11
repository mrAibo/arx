#!/usr/bin/env bash
set -euo pipefail

# HERO-REAL-02 — Record real terminal capture for ARX v0.15.1
# Requires: asciinema (pip install asciinema), ffmpeg
# Output: docs/assets/remote-workspace-update.gif

BINARY="/tmp/arx-hero/arx-v0.15.1-x86_64-unknown-linux-gnu/arx"
RECORDING="/tmp/arx-hero/hero-0.15.1.cast"
OUTPUT="/home/aibo/arx/docs/assets/remote-workspace-update.gif"
FIXTURE_LOCAL="$HOME/arx-demo/app"
FIXTURE_REMOTE="/tmp/arx-demo/app"

# Verify binary
"$BINARY" --version | grep -q "0.15.1" || { echo "FAIL: wrong binary"; exit 1; }

# Prepare fixture
mkdir -p "$FIXTURE_LOCAL" "$FIXTURE_REMOTE"
echo '{"name":"ARX Demo","mode":"safe-update"}' > "$FIXTURE_LOCAL/manifest.json"
cp "$FIXTURE_LOCAL/manifest.json" "$FIXTURE_REMOTE/manifest.json"

# Clean any previous artifacts
rm -f "$FIXTURE_LOCAL"/.arx-bak-* "$FIXTURE_REMOTE"/.arx-bak-*

echo "=== READY ==="
echo "Binary: $BINARY"
echo "Fixture local: $FIXTURE_LOCAL"
echo "Fixture remote: $FIXTURE_REMOTE"
echo ""
echo "Recording will start in 3 seconds..."
sleep 3

asciinema rec "$RECORDING" --command="$BINARY" --cols=120 --rows=30

echo ""
echo "=== RECORDING SAVED ==="
echo "Convert to GIF:"
echo "  asciinema play $RECORDING  # preview"
echo "  ffmpeg -f gif -i <(asciinema cat $RECORDING) $OUTPUT  # convert"
