#!/usr/bin/env bash
# setup_webdav_interop.sh — provisions a disposable real WebDAV server fixture
# for ARX interoperability certification (#241).
#
# Usage: setup_webdav_interop.sh <nextcloud|owncloud>
#
# Contract (#241):
#   - REAL pinned official images, never :latest:
#       nextcloud:34.0.2-apache
#       owncloud/server:11.0.0
#   - localhost-only binding on an ephemeral host port
#   - random ephemeral admin credentials (never admin/admin)
#   - idempotent: deterministic per-kind cleanup before provisioning
#   - readiness = real `occ status` inside the container AND an authenticated
#     PROPFIND against the exact user files root (207 expected)
#   - seeds one file/collection through authenticated WebDAV
#   - writes a chmod-0600 env file; never prints secret bytes
set -euo pipefail

KIND="${1:-}"
case "$KIND" in
  nextcloud) ;;
  owncloud) ;;
  *)
    echo "usage: $0 <nextcloud|owncloud>" >&2
    exit 2
    ;;
esac

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CLEANUP="$SCRIPT_DIR/cleanup_webdav_interop.sh"

CONTAINER="arx-webdav-interop-${KIND}"
ENV_FILE="/tmp/arx-webdav-interop-${KIND}.env"

USER="arx$(head -c 6 /dev/urandom | base64 | tr -dc 'a-z0-9')"
PASS="$(head -c 18 /dev/urandom | base64 | tr -dc 'A-Za-z0-9' | head -c 24)"

"$CLEANUP" >/dev/null 2>&1 || true

pick_port() {
  python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

wait_occ() {
  local tries="${1:-60}"
  for _ in $(seq 1 "$tries"); do
    if [ "$KIND" = "nextcloud" ]; then
      if docker exec -u www-data "$CONTAINER" php occ status >/dev/null 2>&1; then
        return 0
      fi
    else
      if docker exec -u www-data "$CONTAINER" bash -c \
        'cd /var/www/owncloud && php occ status' >/dev/null 2>&1; then
        return 0
      fi
    fi
    sleep 3
  done
  return 1
}

wait_propfind() {
  local root="$1" tries="${2:-40}"
  for _ in $(seq 1 "$tries"); do
    code="$(curl -s -o /dev/null -w '%{http_code}' \
      -u "${USER}:${PASS}" \
      -X PROPFIND -H 'Depth: 0' \
      "$root" || true)"
    if [ "$code" = "207" ]; then
      return 0
    fi
    sleep 3
  done
  return 1
}

if [ "$KIND" = "nextcloud" ]; then
  IMAGE="nextcloud:34.0.2-apache"
  HOST_PORT="$(pick_port)"
  docker run -d --name "$CONTAINER" \
    -p "127.0.0.1:${HOST_PORT}:80" \
    -e SQLITE_DATABASE="$USER"_db \
    -e NEXTCLOUD_ADMIN_USER="$USER" \
    -e NEXTCLOUD_ADMIN_PASSWORD="$PASS" \
    -e NEXTCLOUD_TRUSTED_DOMAINS="127.0.0.1 localhost" \
    "$IMAGE" >/dev/null
else
  IMAGE="owncloud/server:11.0.0"
  HOST_PORT="$(pick_port)"
  # ownCloud server image: the overwrite.config.php template reads dbtype from
  # OWNCLOUD_DB_TYPE; leaving it unset yields an empty dbtype → occ dies with
  # "Invalid database type". Set the full sqlite set explicitly.
  docker run -d --name "$CONTAINER" \
    -p "127.0.0.1:${HOST_PORT}:8080" \
    -e OWNCLOUD_ADMIN_USERNAME="$USER" \
    -e OWNCLOUD_ADMIN_PASSWORD="$PASS" \
    -e OWNCLOUD_TRUSTED_DOMAINS="127.0.0.1 localhost" \
    -e OWNCLOUD_DB_TYPE=sqlite \
    -e OWNCLOUD_DB_NAME="${USER}_db" \
    -e OWNCLOUD_DB_HOST="" \
    -e OWNCLOUD_DB_USERNAME="${USER}" \
    -e OWNCLOUD_DB_PASSWORD="" \
    -e OWNCLOUD_DB_PREFIX=oc_ \
    -e OWNCLOUD_VOLUME_FILES=/mnt/data/files \
    "$IMAGE" >/dev/null
fi

if ! wait_occ 80; then
  echo "# ERROR: ${IMAGE} did not become application-ready (occ status)" >&2
  docker logs --tail 80 "$CONTAINER" 2>&1 | grep -viE 'pass|secret|password' >&2 || true
  exit 1
fi

WEBDAV_ROOT="http://127.0.0.1:${HOST_PORT}/remote.php/dav/files/${USER}/"

if ! wait_propfind "$WEBDAV_ROOT" 40; then
  echo "# ERROR: authenticated PROPFIND against ${WEBDAV_ROOT} never reached 207" >&2
  exit 1
fi

# Seed data through REAL authenticated WebDAV.
SEED_DIR_URL="http://127.0.0.1:${HOST_PORT}/remote.php/dav/files/${USER}/interop-seed/"
code="$(curl -s -o /dev/null -w '%{http_code}' -u "${USER}:${PASS}" -X MKCOL "$SEED_DIR_URL")"
case "$code" in
  201|405) ;;
  *) echo "# ERROR: seed MKCOL failed with HTTP $code" >&2; exit 1 ;;
esac
code="$(curl -s -o /dev/null -w '%{http_code}' -u "${USER}:${PASS}" \
  -X PUT --data-binary 'interop-seed-bytes' "${SEED_DIR_URL}seed.txt")"
if [ "$code" != "201" ] && [ "$code" != "204" ]; then
  echo "# ERROR: seed PUT failed with HTTP $code" >&2
  exit 1
fi

DIGEST="$(docker image inspect "$IMAGE" --format '{{index .RepoDigests 0}}')"

cat > "$ENV_FILE" <<EOF
export ARX_WEBDAV_SMOKE_HOST=${WEBDAV_ROOT}
export ARX_WEBDAV_SMOKE_USER=${USER}
export ARX_WEBDAV_SMOKE_PASS=${PASS}
export ARX_WEBDAV_ACCEPT_PASSWORD=${PASS}
export ARX_WEBDAV_INTEROP_KIND=${KIND}
export ARX_WEBDAV_INTEROP_REQUIRED=1
export ARX_WEBDAV_CONTAINER=${CONTAINER}
EOF
chmod 0600 "$ENV_FILE"

echo "# WebDAV interop fixture ready (${KIND})."
echo "# Image: ${IMAGE}"
echo "# Digest: ${DIGEST}"
echo "# Fixture address: ${WEBDAV_ROOT}"
echo "source ${ENV_FILE}   # ephemeral local-only credentials, chmod 0600; never reuse"
