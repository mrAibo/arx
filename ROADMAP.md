# ARX Roadmap

## Phase 0 — Architecture contract

- define Location model
- define VFS contracts
- define Job model and events
- define TransferPlan and capability model
- define Host and HostGroup model
- define safety and error taxonomy

## Phase 1 — Project foundation

- Ratatui + Crossterm TUI shell
- Tokio runtime
- tracing/logging
- Serde/TOML config foundation
- CI: fmt, clippy, tests
- module boundaries

## Phase 2 — Local VFS

- list/stat/read/write
- mkdir/rename/remove
- symlink handling
- permission metadata
- filesystem error mapping

## Phase 3 — Job engine

- job queue
- progress events
- cancellation
- concurrency limits
- logs/details
- retry model

## Phase 4 — Commander parity

- dual panels
- keyboard + mouse navigation
- marks/selections
- F5 copy
- F6 move/rename
- F7 mkdir
- Trash + undo
- viewer
- themes
- archive operations

## Phase 5 — rsync adapter

- capability detection
- local/local transfer mode
- local/remote and remote/local over SSH
- `--info=progress2` parser
- partial/resume support
- cancellation and exit-code mapping
- dry-run support

## Phase 6 — Host Manager

- import/discover OpenSSH host aliases
- saved ARX host metadata
- favorites
- tags
- many-to-many host groups
- nested group presentation
- default remote directories
- transfer preferences
- saved locations

Example grouping:

```text
Infrastructure
├── Database
│   ├── Oracle
│   └── PostgreSQL
└── Application

Projects
├── Project A
└── Project B

Environment
├── Production
├── Integration
└── Test
```

One host may appear in several groups simultaneously.

## Phase 7 — SSH/SFTP

- OpenSSH config resolution
- ssh-agent integration
- known_hosts verification
- connection manager
- keepalive/reconnect
- SFTP VFS backend
- remote capability cache
- rsync fallback to SFTP

## Phase 8 — Transfer planner

- Native/Rsync/SFTP strategy selection
- remote-to-remote rules
- free-space checks
- overwrite/conflict policy
- preservation policy for permissions, symlinks, xattrs/ACL where available

## Phase 9 — Remote shell

- suspend TUI and launch `ssh <host>`
- launch local shell
- environment-safe terminal restore
- host actions menu

## Phase 10 — Directory synchronization

- compare
- rsync dry-run
- preview changes
- left → right
- right → left
- update-newer mode
- mirror mode with explicit confirmation
- conflict handling

## Phase 11 — Sessions and UX

- tabs
- saved locations
- navigation history
- bookmarks
- persisted layouts/session state
- command palette
- quick view
- tree navigation

## Phase 12 — Embedded terminal

- local PTY
- remote SSH PTY
- terminal emulation component
- resize/session lifecycle

## Later adapters

Architect for these, but do not block initial releases on them:

- rclone
- restic
- borg
- SMB
- WebDAV
- object/cloud storage

## Explicit non-goals for early phases

Do not implement custom versions of:

- SSH protocol
- SFTP protocol
- rsync protocol
- compressors
- text editor
- pager
- full terminal emulator

Use existing system tools or mature libraries until a concrete limitation justifies otherwise.
