#!/usr/bin/env bash
set -euo pipefail

: "${RUNNER_TEMP:?RUNNER_TEMP must be set}"

FIXTURE_ROOT="${RUNNER_TEMP}/arx-sftp-sync-physical"
rm -rf -- "$FIXTURE_ROOT"
mkdir -p "$FIXTURE_ROOT" "$FIXTURE_ROOT/data-a" "$FIXTURE_ROOT/data-b"
chmod 700 "$FIXTURE_ROOT" "$FIXTURE_ROOT/data-a" "$FIXTURE_ROOT/data-b"

CLIENT_KEY="$FIXTURE_ROOT/client_ed25519"
AUTHORIZED_KEYS="$FIXTURE_ROOT/authorized_keys"
ssh-keygen -q -t ed25519 -N '' -f "$CLIENT_KEY"
cp "$CLIENT_KEY.pub" "$AUTHORIZED_KEYS"
chmod 600 "$AUTHORIZED_KEYS"

PORT_A="$(python3 - <<'PY'
import socket
s = socket.socket()
s.bind(('127.0.0.1', 0))
print(s.getsockname()[1])
s.close()
PY
)"
PORT_B="$(python3 - <<'PY'
import socket
s = socket.socket()
s.bind(('127.0.0.1', 0))
print(s.getsockname()[1])
s.close()
PY
)"
if [ "$PORT_A" = "$PORT_B" ]; then
  echo "failed to allocate distinct SSH ports" >&2
  exit 1
fi

USER_NAME="$(id -un)"
SSH_DIR="$HOME/.ssh"
SSH_CONFIG="$SSH_DIR/config"
SSH_CONFIG_BACKUP="$FIXTURE_ROOT/ssh_config.backup"
mkdir -p "$SSH_DIR"
chmod 700 "$SSH_DIR"
if [ -f "$SSH_CONFIG" ]; then
  cp "$SSH_CONFIG" "$SSH_CONFIG_BACKUP"
  HAD_SSH_CONFIG=1
else
  HAD_SSH_CONFIG=0
fi

cleanup_sftp_sync_fixture() {
  set +e
  for pidfile in "$FIXTURE_ROOT/sshd-a.pid" "$FIXTURE_ROOT/sshd-b.pid"; do
    if [ -s "$pidfile" ]; then
      sudo kill "$(cat "$pidfile")" >/dev/null 2>&1 || true
    fi
  done
  if [ "$HAD_SSH_CONFIG" = 1 ]; then
    cp "$SSH_CONFIG_BACKUP" "$SSH_CONFIG"
    chmod 600 "$SSH_CONFIG"
  else
    rm -f "$SSH_CONFIG"
  fi
}
trap cleanup_sftp_sync_fixture EXIT

make_sshd_config() {
  local name="$1"
  local port="$2"
  local host_key="$FIXTURE_ROOT/host_${name}_ed25519"
  local config="$FIXTURE_ROOT/sshd-${name}.conf"
  ssh-keygen -q -t ed25519 -N '' -f "$host_key"
  cat > "$config" <<EOF
Port $port
ListenAddress 127.0.0.1
HostKey $host_key
PidFile $FIXTURE_ROOT/sshd-${name}.pid
AuthorizedKeysFile $AUTHORIZED_KEYS
StrictModes no
PubkeyAuthentication yes
PasswordAuthentication no
KbdInteractiveAuthentication no
ChallengeResponseAuthentication no
UsePAM no
PermitRootLogin no
AllowUsers $USER_NAME
Subsystem sftp internal-sftp
LogLevel VERBOSE
EOF
  sudo /usr/sbin/sshd -t -f "$config"
  sudo /usr/sbin/sshd -f "$config" -E "$FIXTURE_ROOT/sshd-${name}.log"
}

sudo mkdir -p /run/sshd
make_sshd_config a "$PORT_A"
make_sshd_config b "$PORT_B"

cat >> "$SSH_CONFIG" <<EOF

Host arx-sftp-a
  HostName 127.0.0.1
  Port $PORT_A
  User $USER_NAME
  IdentityFile $CLIENT_KEY
  IdentitiesOnly yes
  BatchMode yes
  StrictHostKeyChecking no
  UserKnownHostsFile /dev/null
  LogLevel ERROR

Host arx-sftp-b
  HostName 127.0.0.1
  Port $PORT_B
  User $USER_NAME
  IdentityFile $CLIENT_KEY
  IdentitiesOnly yes
  BatchMode yes
  StrictHostKeyChecking no
  UserKnownHostsFile /dev/null
  LogLevel ERROR
EOF
chmod 600 "$SSH_CONFIG"

for alias in arx-sftp-a arx-sftp-b; do
  ready=0
  for _ in $(seq 1 40); do
    if ssh "$alias" true >/dev/null 2>&1; then
      ready=1
      break
    fi
    sleep 0.1
  done
  if [ "$ready" != 1 ]; then
    echo "SSH fixture $alias did not become ready" >&2
    cat "$FIXTURE_ROOT/sshd-a.log" >&2 2>/dev/null || true
    cat "$FIXTURE_ROOT/sshd-b.log" >&2 2>/dev/null || true
    exit 1
  fi
done

export ARX_SFTP_SYNC_PHYSICAL=1
export ARX_SFTP_SYNC_HOST_A=arx-sftp-a
export ARX_SFTP_SYNC_HOST_B=arx-sftp-b
export ARX_SFTP_SYNC_ROOT_A="$FIXTURE_ROOT/data-a"
export ARX_SFTP_SYNC_ROOT_B="$FIXTURE_ROOT/data-b"

echo "SFTP_SYNC_FIXTURE_ROOT=$FIXTURE_ROOT"
echo "SFTP_SYNC_ENDPOINT_A=arx-sftp-a:$PORT_A"
echo "SFTP_SYNC_ENDPOINT_B=arx-sftp-b:$PORT_B"
