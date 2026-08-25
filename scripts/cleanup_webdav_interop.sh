#!/usr/bin/env bash
# cleanup_webdav_interop.sh — tears down #241 interop fixtures (both kinds).
set -euo pipefail
for c in $(docker ps -a --format '{{.Names}}' 2>/dev/null | grep -E '^arx-webdav-interop-' || true); do
  docker rm -f "$c" >/dev/null 2>&1 || true
done
rm -f /tmp/arx-webdav-interop-nextcloud.env /tmp/arx-webdav-interop-owncloud.env 2>/dev/null || true
echo "webdav interop fixture cleaned"
