#!/usr/bin/env bash
# cleanup_webdav_acceptance.sh — tears down the disposable Apache mod_dav fixture.
set -euo pipefail
cd "$(dirname "$0")/.."
# Tear down any container we created for WebDAV acceptance.
for c in $(docker ps -a --format '{{.Names}}' 2>/dev/null | grep -E '^arx-webdav-acceptance'); do
  docker rm -f "$c" >/dev/null 2>&1 || true
done
# Remove stale temp config if present.
rm -f /tmp/arx-webdav.conf 2>/dev/null || true
echo "webdav acceptance fixture cleaned"
