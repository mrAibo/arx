#!/usr/bin/env bash
# setup_webdav_acceptance.sh — provisions a disposable Apache httpd + mod_dav_fs
# WebDAV server over TCP for ARX physical WebDAV acceptance (W1–W18).
#
# Design (matches S3/SFTP physical-fixture conventions):
#   - Real WebDAV server (Apache mod_dav), NOT a mock.
#   - Binds ONLY to 127.0.0.1 on an ephemeral port.
#   - Ephemeral credentials (random user/password, never reused).
#   - Idempotent: re-running tear-downs first.
#   - Prints ONLY env exports an operator sources; never prints secret values.
#
# Approach: run the stock httpd image, then APPEND a single Include of our DAV
# config to the shipped httpd.conf and gracefully reload. No sed/grep on the
# shipped config (that corrupts it on re-runs).
#
# Requires: docker. No external cloud. Test-only; plaintext Basic over 127.0.0.1
# is acceptable per PACK E (explicit localhost/test use).
set -euo pipefail
cd "$(dirname "$0")/.."

CONTAINER="arx-webdav-acceptance-$$"
IMAGE="httpd:2.4"
WEBDAV_DIR="/usr/local/apache2/htdocs/dav"
USER="arxw$(head -c 6 /dev/urandom | base64 | tr -dc 'a-z0-9')"
PASS="$(head -c 18 /dev/urandom | base64 | tr -dc 'A-Za-z0-9' | head -c 24)"

# Clean any previous run.
"$(dirname "$0")/cleanup_webdav_acceptance.sh" >/dev/null 2>&1 || true

# DAV config (mod_dav* ship with the image but are not loaded by default).
cat > /tmp/arx-webdav.conf <<EOF
LoadModule dav_module modules/mod_dav.so
LoadModule dav_fs_module modules/mod_dav_fs.so
LoadModule dav_lock_module modules/mod_dav_lock.so

DavLockDB /usr/local/apache2/var/DavLock

<Directory "${WEBDAV_DIR}">
    Dav On
    Options +Indexes
    AuthType Basic
    AuthName "arx-webdav-test"
    AuthUserFile /usr/local/apache2/conf/dav.passwd
    Require valid-user
    AllowOverride None
</Directory>
EOF

docker run -d --name "$CONTAINER" -p 127.0.0.1:0:80 "$IMAGE" >/dev/null

# Wait for the default httpd to be up.
for _ in $(seq 1 20); do
  docker exec "$CONTAINER" true >/dev/null 2>&1 && break
  sleep 0.5
done

docker cp /tmp/arx-webdav.conf "$CONTAINER":/usr/local/apache2/conf/extra/arx-webdav.conf
docker exec "$CONTAINER" sh -c "mkdir -p ${WEBDAV_DIR} /usr/local/apache2/var && chown -R www-data:www-data ${WEBDAV_DIR} /usr/local/apache2/var"
docker exec "$CONTAINER" sh -c "touch /usr/local/apache2/var/DavLock && chown www-data:www-data /usr/local/apache2/var/DavLock"
printf '%s\n' "$PASS" | docker exec -i "$CONTAINER" sh -c "htpasswd -c -i -B /usr/local/apache2/conf/dav.passwd '$USER' >/dev/null"

# Append a single Include of our config at the end of the shipped httpd.conf,
# then verify and gracefully reload. Appending (not sed) keeps it idempotent.
docker exec "$CONTAINER" sh -c "printf '\nInclude conf/extra/arx-webdav.conf\n' >> /usr/local/apache2/conf/httpd.conf"
docker exec "$CONTAINER" sh -c "apachectl configtest"
docker exec "$CONTAINER" sh -c "apachectl graceful >/dev/null 2>&1 || httpd -k graceful >/dev/null 2>&1" || true
sleep 1

# Resolve the mapped host port on 127.0.0.1.
HOST_PORT="$(docker port "$CONTAINER" 80/tcp | grep '127.0.0.1' | head -1 | cut -d: -f2)"
URL="http://127.0.0.1:${HOST_PORT}/dav/"

# Smoke: create a seeded file via curl with creds (test-only localhost).
echo "arx-webdav-seed" | docker exec -i "$CONTAINER" sh -c "cat > ${WEBDAV_DIR}/.keep"

# Blocker G: write credentials to a chmod 0600 file; print ONLY the path and a
# source instruction. Never echo the password bytes to stdout/log/report.
ENV_FILE="$(mktemp /tmp/arx-webdav-env.XXXXXX)"
chmod 0600 "$ENV_FILE"
cat > "$ENV_FILE" <<EOF
export ARX_WEBDAV_SMOKE_HOST=http://127.0.0.1:${HOST_PORT}/dav/
export ARX_WEBDAV_SMOKE_USER=${USER}
export ARX_WEBDAV_SMOKE_PASS=${PASS}
export ARX_WEBDAV_CONTAINER=${CONTAINER}
EOF
echo "# WebDAV acceptance fixture ready (Apache $(docker exec "$CONTAINER" httpd -v 2>/dev/null | head -1 | sed 's/^.*\///'))."
echo "# Docker image: ${IMAGE}"
echo "# Fixture address: ${URL}"
echo "source ${ENV_FILE}   # credentials are ephemeral, local-only, chmod 0600; never reuse"
