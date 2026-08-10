# ARX Commander: Final Status

**Автор:** Aleksej Voronin  
**Дата:** 10 августа 2026  
**Репозиторий:** `mrAibo/arx`

---

## PR #40 — Commander UX Hardening: MERGED ✓

**Merge commit:** `c035f1a` (squash)  
**Branch:** `feature/commander-ux-hardening` → `main`  
**Changed:** 16 files, +2215 / −254  
**CI:** SUCCESS  
**Tests:** 268 passed  
**Architecture review:** 0 blockers

### Что вошло

| Карточка | Тема | Статус |
|---|---:|---|
| ARX-UX-01 | Terminal lifecycle (`tcflush`) | ✓ |
| ARX-UX-02 | Selection safety (`Pane + Location`) | ✓ |
| ARX-UX-03 | Space → ToggleSelect + cursor advance | ✓ |
| ARX-UX-04 | Virtual parent row (`..`) | ✓ |
| ARX-UX-05 | F3 ViewFile through Action pipeline | ✓ |
| ARX-UX-06 | F4 EditFile through Action pipeline | ✓ |
| ARX-UX-07 | Responsive contextual footer | ✓ |

### Ключевые архитектурные решения

- Terminal: `tcflush(TCIFLUSH)` на Linux, симметричный enter/restore, Drop best-effort
- Selection: `selection_scope: Option<(Pane, Location)>`, mismatch → clear
- Editor: `config.ui.editor → VISUAL → EDITOR → unavailable`, без скрытого vi
- Footer: Action Catalog + runtime Keymap + availability, без жёсткого лимита

---

## ARX-FILEOPS-01A — Audit Legacy F5–F8: DONE

### Текущее состояние

| Клавиша | Действие | Action-driven? | Где | Замечания |
|---|---:|---|---:|---|
| **F5** | Copy | Нет | `tui.rs:1202` | Прямой handler, TransferPlanner |
| **F6** | Move | Нет | `tui.rs:1326` | Прямой handler, TransferPlanner |
| **F7** | Tmux / Mkdir | Нет | `tui.rs:818` / `tui.rs:1012` | Коллизия (см. ниже) |
| **F8** | Delete (trash) | Нет | `tui.rs:1450` | Локальный trash, SFTP disabled |
| **Shift+F6** | Rename | Нет | `tui.rs:1017` | Инлайн `cmd = "mv '…' "` |

### Коллизия F7

```text
Browser mode, !show_terminal:
  F7 → ListTmuxSessions (tui.rs:818) → continue
  │
  └─ late F7 (mkdir, tui.rs:1012) НЕДОСТИЖИМ

Browser mode, show_terminal:
  F7 → mkdir inline (tui.rs:1012)
```

**F7 делает две разные вещи в зависимости от `show_terminal`.**

### Что отсутствует

- F5–F8 **не зарегистрированы** в `keymap.rs` — не проходят через KeyRouter
- **Нет Action** для Copy, Move, Mkdir, Delete — только прямые handlers
- **Нет availability** — нельзя отключить в SFTP/Archive контексте
- **Footer** не показывает F5–F8
- **Command Center** не предлагает Copy/Move/Delete
- **README** не документирует F5–F8
- **Context menu** жёстко зашит: «Copy F5», «Move F6», «Delete F8», «View F3», «Edit F4»

---

## ARX-FILEOPS-01B — F7 Decision: RECOMMENDATION

**Рекомендация: Option A — F7 = Mkdir (MC-совместимо)**

| Опция | F7 | Tmux | Оценка |
|---|---:|---|---:|---|
| **A** (рекомендовано) | Mkdir | `Ctrl+T` или F9 | MC-совместимость, чистая семантика |
| B | Tmux | Mkdir на другую клавишу | Ломает MC-ожидания |
| C | Контекстный F7 | — | Ненадёжно, ambiguity |

**Обоснование A:**
- Commander familiarity: Midnight Commander, Double Commander, FAR — везде F7 = Mkdir
- Обнаружимость: Mkdir — файловая операция, должна быть на F-клавише
- Tmux — отдельный domain, логично на `Ctrl+T` (Terminal multiplexer) или F9
- Чистота Action architecture: один ActionId = одна семантика

---

## ARX-FILEOPS-02–06 — Implementation Plan

| Карточка | Действие | Что сделать |
|---|---:|---|
| **02** | F5 Copy | `Action::Copy` → Catalog → Availability → Keymap → dispatcher. Сохранить TransferPlanner. |
| **03** | F6 Move | `Action::Move` → Catalog → Availability → Keymap → dispatcher. |
| **04** | F7 Mkdir | `Action::Mkdir` → Catalog → Availability → Keymap → dispatcher. Tmux → F9 или Ctrl+T. |
| **05** | F8 Delete | `Action::Delete` → Catalog → Availability → Keymap → dispatcher. Локальный trash. |
| **06** | Footer | F5–F8 появятся в footer автоматически через Action + Keymap + Availability. |

### Принципы миграции

- Каждый F-key → один `ActionId`
- Availability: local = available, SFTP = disabled с причиной, archive = disabled/hidden
- Keymap регистрирует F5–F8 в Browser context
- Command Center и footer наследуют availability автоматически
- Context menu: динамический, через Action metadata
- README: документировать F3–F8 в таблице

---

## Next Action

Жду подтверждения **Option A (F7 = Mkdir)** — затем стартую ARX-FILEOPS-02.
