# ARX Configuration

ARX keeps configuration minimal and secret-free. This document covers the
files ARX reads/writes and the safe boundaries between ARX-managed and
user-owned configuration.

## Locations

| Path | Owner | Purpose |
|------|-------|---------|
| `~/.config/arx/arx.toml` | ARX | main config + `[s3_targets]` metadata |
| `~/.config/arx/hosts.toml` | ARX | host bookmarks (id, alias, groups, tags, notes) |
| `~/.ssh/config` | **user** | OpenSSH client config |
| `~/.ssh/arx_hosts.conf` | ARX | ARX-managed SSH host entries |
| `~/.ssh/arx/` | ARX | ARX-generated SSH keys |
| `~/.aws/*` | AWS CLI | AWS profiles / credentials |

## S3 targets (`[s3_targets]` in arx.toml)

```toml
[s3_targets.mybucket]
name = "My Bucket"
bucket = "my-bucket"
region = "eu-central-1"
profile = "default"          # AWS CLI profile; credentials come from the chain
endpoint_url = "https://s3.example.com"
force_path_style = false
```

ARX stores **no secrets** here. AWS credentials are resolved by the standard
AWS credential chain (`~/.aws/credentials`, env, IMDS). See
[SECRET_STORAGE.md](SECRET_STORAGE.md).

## SSH hosts

ARX manages SSH host entries in `~/.ssh/arx_hosts.conf`, which is wired into
the user-owned `~/.ssh/config` via a single `Include` line installed by ARX:

```ssh
# installed once by ARX
Include ~/.ssh/arx_hosts.conf
```

ARX never rewrites unmanaged `Host` blocks. A managed entry looks like:

```ssh
Host prod
    HostName prod.example.com
    User deploy
    Port 22
    IdentityFile ~/.ssh/arx/prod_ed25519
    IdentitiesOnly yes
```

`IdentityFile` is a **path**. The private key bytes stay in the file; ARX never
stores or displays them. To generate a key, ARX shells out to `ssh-keygen`
(`ed25519`, empty passphrase — passphrase is not stored by ARX).

### Managed-config rules

- At most one `Include` line; never duplicated.
- First mutation of `~/.ssh/config` is backed up to `~/.ssh/config.arx-backup`.
- External manual edits to either file remain supported; ARX re-reads on reload.
- Collision with an unmanaged alias fails closed.
- Wildcard / control-character aliases are rejected.

## Editing

Use the in-app **SSH Hosts** manager (press **F12**, or Commander → SSH Hosts)
for add / edit / delete / test / identity / open-config / reload. Manual edits
to `~/.ssh/config` or `~/.ssh/arx_hosts.conf` are also respected (reload to
resync).

### SSH Hosts actions

| Key | Action |
|-----|--------|
| `A` | Add a host (form: Alias, HostName, User, Port, IdentityFile, ProxyJump, IdentitiesOnly) |
| `E` | Edit the selected host (same form, pre-filled) |
| `D` | Delete the selected ARX-managed host only |
| `T` | Test the alias (config truth via `ssh -G`, then a bounded real probe) — runs off the TUI loop |
| `K` | Identity: in the form, `Ctrl+K` generates a new Ed25519 key and attaches it; typing a path stores that path |
| `O` | Open `~/.ssh/config` in `$EDITOR`/`$VISUAL` (argument-safe, no shell interpolation) |
| `Shift+O` | Open `~/.ssh/arx_hosts.conf` |
| `R` | Reload from disk (sees external edits) |

### Generated keys — honest notes

- Keys are **Ed25519**, generated via `ssh-keygen`.
- Default path is `~/.ssh/arx/<alias>_ed25519`.
- ARX-generated keys are currently created **without a passphrase**; the form
  shows this fact and asks for confirmation before generating.
- The private key remains filesystem-only (ARX never stores or displays the
  key bytes or passphrase).
- When a generated key is attached, ARX sets `IdentitiesOnly yes` unless you
  disable it.
