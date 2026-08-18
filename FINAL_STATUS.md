> ⚠️ **ARCHIVE / HISTORICAL SNAPSHOT — NOT current status.**
> Authored **10 August 2026** (PR #40 merged, PR #42 open). It does **NOT** reflect
> current product truth. For current status see `README.md`, `ROADMAP.md`, `docs/`, and
> [GitHub Releases](https://github.com/mrAibo/arx/releases) — **ARX v0.17.0 was released
> 2026-08-16**.

# ARX Commander: Final Status

**Автор:** Aleksej Voronin  
**Дата:** 10 августа 2026  
**Репозиторий:** `mrAibo/arx`

---

## PR #40 — Commander UX Hardening: MERGED

**Merge:** `c035f1a` (squash) · **Tests:** 268 · **Files:** 16, +2215/−254

---

## PR #42 — Action-driven F5-F8 file operations: OPEN

**Branch:** `feature/fileops` · **Head:** `721f1dc`  
**Base:** `main` (`c035f1a`) · **Tests:** 276 · **Files:** 7, +703/−403

| Key | Action | Truth |
|---:|---|---|
| F5 | Copy | TransferPlanner |
| F6 | Move | TransferPlanner |
| F7 | Mkdir | State |
| F8 | Delete | MutationService |
| F9 | Remote Hosts | Toggle |

### Corrections applied:
- `4d5b1d3` — F8→Delete binding (was missing)
- `9ee23f0` — Copy/Move availability aligned with TransferPlanner
- `a1f5ccd` — Delete cleanup, phantom guard removed, parent safety tests
- `721f1dc` — F9=Hosts (product decision), Tmux in Command Center

### Parity review: PASS (0 BLOCKER, 0 MAJOR)
Selection clearing, pane refresh, messages, parent exclusion, job creation, locations, planner inputs, error handling — all match legacy.

---

## Что дальше
- **CI на PR #42** — ожидание
- **ARX-FILEOPS-06** — Automated acceptance matrix (после CI)
- **ARX-FILEOPS-07** — Real interactive acceptance (после 06)
- **ARX-KEYS-01** — Emulator-safe shortcut architecture
