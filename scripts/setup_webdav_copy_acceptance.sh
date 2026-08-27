#!/usr/bin/env bash
# Two independently addressable Apache mod_dav endpoints for #275 physical
# WebDAV -> WebDAV recursive-copy acceptance. Test-only localhost Basic auth.
set -euo pipefail
set +x

IMAGE="httpd:2.4"
DAV_DIR="/usr/local/apache2/htdocs/dav"
RUN_TOKEN="${GITHUB_RUN_ID:-$$}-${GITHUB_RUN_ATTEMPT:-0}-$$"
CONTAINER_A="arx-webdav-copy-a-${RUN_TOKEN}"
CONTAINER_B="arx-webdav-copy-b-${RUN_TOKEN}"
CONF="$(mktemp /tmp/arx-webdav-copy-conf.XXXXXX)"

cleanup() {
  docker rm -f "$CONTAINER_A" "$CONTAINER_B" >/dev/null 2>&1 || true
  rm -f "$CONF"
}
trap cleanup EXIT

gen_user() {
  printf 'arxw%s' "$(head -c 8 /dev/urandom | base64 | tr -dc 'a-z0-9' | head -c 10)"
}

gen_pass() {
  head -c 24 /dev/urandom | base64 | tr -dc 'A-Za-z0-9' | head -c 28
}

cat >"$CONF" <<'EOF'
LoadModule dav_module modules/mod_dav.so
LoadModule dav_fs_module modules/mod_dav_fs.so
LoadModule dav_lock_module modules/mod_dav_lock.so
DavLockDB /usr/local/apache2/var/DavLock
<Directory "/usr/local/apache2/htdocs/dav">
    Dav On
    Options +Indexes
    AuthType Basic
    AuthName "arx-webdav-copy-test"
    AuthUserFile /usr/local/apache2/conf/dav.passwd
    Require valid-user
    AllowOverride None
</Directory>
EOF

start_endpoint() {
  local container="$1"
  local user="$2"
  local pass="$3"
  docker run -d --name "$container" -p 127.0.0.1:0:80 "$IMAGE" >/dev/null
  for _ in $(seq 1 30); do
    docker exec "$container" true >/dev/null 2>&1 && break
    sleep 0.2
  done
  docker cp "$CONF" "$container":/usr/local/apache2/conf/extra/arx-webdav-copy.conf
  docker exec "$container" sh -c "mkdir -p '$DAV_DIR' /usr/local/apache2/var && chown -R www-data:www-data '$DAV_DIR' /usr/local/apache2/var"
  docker exec "$container" sh -c "touch /usr/local/apache2/var/DavLock && chown www-data:www-data /usr/local/apache2/var/DavLock"
  printf '%s\n' "$pass" | docker exec -i "$container" sh -c "htpasswd -c -i -B /usr/local/apache2/conf/dav.passwd '$user' >/dev/null"
  docker exec "$container" sh -c "printf '\nInclude conf/extra/arx-webdav-copy.conf\n' >> /usr/local/apache2/conf/httpd.conf"
  docker exec "$container" apachectl configtest >/dev/null
  docker exec "$container" sh -c "apachectl graceful >/dev/null 2>&1 || httpd -k graceful >/dev/null 2>&1" || true
}

USER_A="$(gen_user)"
PASS_A="$(gen_pass)"
USER_B="$(gen_user)"
PASS_B="$(gen_pass)"
start_endpoint "$CONTAINER_A" "$USER_A" "$PASS_A"
start_endpoint "$CONTAINER_B" "$USER_B" "$PASS_B"
sleep 1

PORT_A="$(docker port "$CONTAINER_A" 80/tcp | awk -F: '/127\.0\.0\.1/ {print $2; exit}')"
PORT_B="$(docker port "$CONTAINER_B" 80/tcp | awk -F: '/127\.0\.0\.1/ {print $2; exit}')"
[ -n "$PORT_A" ] && [ -n "$PORT_B" ] && [ "$PORT_A" != "$PORT_B" ]

export ARX_WEBDAV_COPY_PHYSICAL=1
export ARX_WEBDAV_COPY_A_HOST="http://127.0.0.1:${PORT_A}/dav/"
export ARX_WEBDAV_COPY_A_USER="$USER_A"
export ARX_WEBDAV_COPY_A_PASS="$PASS_A"
export ARX_WEBDAV_COPY_B_HOST="http://127.0.0.1:${PORT_B}/dav/"
export ARX_WEBDAV_COPY_B_USER="$USER_B"
export ARX_WEBDAV_COPY_B_PASS="$PASS_B"
# Production ProviderRegistry secret-resolution path for target ids copya/copyb.
export ARX_WEBDAV_COPYA_PASSWORD="$PASS_A"
export ARX_WEBDAV_COPYB_PASSWORD="$PASS_B"
export ARX_WEBDAV_COPY_CONTAINER_A="$CONTAINER_A"
export ARX_WEBDAV_COPY_CONTAINER_B="$CONTAINER_B"

echo "# WebDAV copy fixture ready: two Apache mod_dav endpoints"
echo "# A: http://127.0.0.1:${PORT_A}/dav/"
echo "# B: http://127.0.0.1:${PORT_B}/dav/"
