#!/usr/bin/env bash
set -euo pipefail

: "${RUNNER_TEMP:?RUNNER_TEMP must be set}"

FIXTURE_ROOT="${RUNNER_TEMP}/arx-sftp-sync-physical"
REMOTE_USER="arxsftp"
REMOTE_HOME="/home/${REMOTE_USER}"
REMOTE_ROOT_A="/tmp/arx-sftp-sync-physical-a-${GITHUB_RUN_ID:-$$}"
REMOTE_ROOT_B="/tmp/arx-sftp-sync-physical-b-${GITHUB_RUN_ID:-$$}"

rm -rf -- "$FIXTURE_ROOT"
mkdir -p "$FIXTURE_ROOT"
chmod 700 "$FIXTURE_ROOT"

CLIENT_KEY="$FIXTURE_ROOT/client_ed25519"
ssh-keygen -q -t ed25519 -N '' -f "$CLIENT_KEY"
chmod 600 "$CLIENT_KEY"

# Use a dedicated disposable account rather than the hosted-runner account.
# GitHub runner accounts may be locked in /etc/shadow even when pubkey auth is
# configured, which causes sshd to reject them before authorized_keys is read.
if id "$REMOTE_USER" >/dev/null 2>&1; then
  sudo userdel -r "$REMOTE_USER" >/dev/null 2>&1 || true
fi
sudo useradd --create-home --shell /bin/bash "$REMOTE_USER"
# An empty password field unlocks the account for public-key authentication;
# password and keyboard-interactive authentication stay disabled in sshd.
sudo passwd -d "$REMOTE_USER" >/dev/null
sudo install -d -m 700 -o "$REMOTE_USER" -g "$REMOTE_USER" "$REMOTE_HOME/.ssh"
sudo install -m 600 -o "$REMOTE_USER" -g "$REMOTE_USER" \
  "$CLIENT_KEY.pub" "$REMOTE_HOME/.ssh/authorized_keys"
sudo install -d -m 755 -o "$REMOTE_USER" -g "$REMOTE_USER" "$REMOTE_ROOT_A"
sudo install -d -m 755 -o "$REMOTE_USER" -g "$REMOTE_USER" "$REMOTE_ROOT_B"

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
  sudo rm -rf -- "$REMOTE_ROOT_A" "$REMOTE_ROOT_B" >/dev/null 2>&1 || true
  sudo userdel -r "$REMOTE_USER" >/dev/null 2>&1 || true
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
AuthorizedKeysFile .ssh/authorized_keys
StrictModes yes
PubkeyAuthentication yes
PasswordAuthentication no
KbdInteractiveAuthentication no
ChallengeResponseAuthentication no
UsePAM no
PermitRootLogin no
AllowUsers $REMOTE_USER
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
  User $REMOTE_USER
  IdentityFile $CLIENT_KEY
  IdentitiesOnly yes
  BatchMode yes
  StrictHostKeyChecking no
  UserKnownHostsFile /dev/null
  LogLevel ERROR

Host arx-sftp-b
  HostName 127.0.0.1
  Port $PORT_B
  User $REMOTE_USER
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
    echo "--- ssh client diagnostics ($alias) ---" >&2
    ssh -vvv "$alias" true >&2 || true
    echo "--- sshd-a log ---" >&2
    sudo cat "$FIXTURE_ROOT/sshd-a.log" >&2 2>/dev/null || true
    echo "--- sshd-b log ---" >&2
    sudo cat "$FIXTURE_ROOT/sshd-b.log" >&2 2>/dev/null || true
    exit 1
  fi
done

export ARX_SFTP_SYNC_PHYSICAL=1
export ARX_SFTP_SYNC_HOST_A=arx-sftp-a
export ARX_SFTP_SYNC_HOST_B=arx-sftp-b
export ARX_SFTP_SYNC_ROOT_A="$REMOTE_ROOT_A"
export ARX_SFTP_SYNC_ROOT_B="$REMOTE_ROOT_B"

echo "SFTP_SYNC_FIXTURE_ROOT=$FIXTURE_ROOT"
echo "SFTP_SYNC_ENDPOINT_A=arx-sftp-a:$PORT_A"
echo "SFTP_SYNC_ENDPOINT_B=arx-sftp-b:$PORT_B"
echo "SFTP_SYNC_REMOTE_ROOT_A=$REMOTE_ROOT_A"
echo "SFTP_SYNC_REMOTE_ROOT_B=$REMOTE_ROOT_B"
