# ARX Commander UX Hardening: итоговый отчёт

**Автор и владелец:** Aleksej Voronin  
**Дата проверки:** 10 августа 2026  
**Репозиторий:** `mrAibo/arx`  
**Ветка:** `feature/commander-ux-hardening`  
**Pull request:** [#40 — Commander UX hardening: terminal lifecycle](https://github.com/mrAibo/arx/pull/40)  
**Базовый SHA:** `681089a17372dd797d9c45c35989a830fb36c69f`  
**Проверенный implementation HEAD:** `90ee248119c6b7518917db05c915bbb090346eb5`

## Итог

Карточки ARX-UX-01A–07 выполнены. Локальные Rust gates прошли, обязательные сценарии в настоящем Konsole проверены, local и remote ref совпали, CI для implementation HEAD завершился успешно. PR остаётся открытым и не был слит.

`Cargo.toml` не менялся. Новые зависимости не добавлялись.

## Статус по карточкам

| Карточка | Статус | Результат | Commit SHA |
|---|---|---|---|
| ARX-UX-01A | PASS | Положительный контроль подтвердил, что Konsole действительно генерирует SGR mouse reports при включённом reporting и перестаёт после отключения. | Проверка; код не менялся |
| ARX-UX-01B | PASS | 10 обычных циклов и отдельный Ctrl+O/subshell цикл прошли без post-quit mouse leakage. Проверены keyboard editing, Ctrl+C, prompt и selection marker. | Верифицирует `9b68097a3909654aa7e2647aea7eb9d7dff61526` |
| ARX-UX-01C | PASS | Read-only audit подтвердил Linux gating, `TCIFLUSH` и сохранение terminal lifecycle invariants. | Верифицирует `9b68097a3909654aa7e2647aea7eb9d7dff61526` |
| ARX-UX-01D | PASS | Перед возвратом stdin внешней программе queued input очищается атомарно через `tcflush(TCIFLUSH)`. | `9b68097a3909654aa7e2647aea7eb9d7dff61526` |
| ARX-UX-02A | PASS | Audit обнаружил коллизию одинаковых имён в разных panes/locations. | Audit; код не менялся |
| ARX-UX-02B | PASS | Selection identity привязана к `(Pane, Location)`; rendering и mutation paths используют тот же scope. | `0a8bda0600878f83704ecab9fced248fda733916` |
| ARX-UX-03 | PASS | Plain Space вызывает `ToggleSelect` и двигает cursor, сохраняя split-pane, empty-list и virtual-parent semantics. | `597b630e10031c7694b707b4d5981dfbde09b168` |
| ARX-UX-04 | PASS | Добавлены provider-aware `Location::parent()` и virtual parent row; строка `..` исключена из selection и file operations. | `610c1174dbd41d61d00e9e135d4bcefacf64a536` |
| ARX-UX-05 | PASS | F3 идёт через `Action::ViewFile → Effect::PreviewFile → ViewerLines`. Preview разрешён только для local regular file. | `60d1a3464c1302ee3f4d6e0659107b297a322dbe`, corrections: `901f6f4fc851083690950b6cf68e4efba065a18a` |
| ARX-UX-06 | PASS | F4 идёт через `Action::EditFile`; editor запускается после suspend и active pane перечитывается после выхода. | `03e6154044364adf079db6912889826a63d9e8e6`, corrections: `901f6f4fc851083690950b6cf68e4efba065a18a` |
| ARX-UX-07 | PASS | Footer строится из Action Catalog, runtime Keymap и общей availability model, сортируется по priority и заполняется по реальной ширине. | `901f6f4fc851083690950b6cf68e4efba065a18a`, review test: `90ee248119c6b7518917db05c915bbb090346eb5` |

Все перечисленные commits являются предками remote ref `origin/feature/commander-ux-hardening`.

## Что изменилось

### Terminal lifecycle

- `src/tui_terminal.rs`: Linux-specific `tcflush(TCIFLUSH)` очищает непрочитанный stdin после отключения mouse reporting и до передачи terminal внешней программе.
- Изменение узкое: output queue не затрагивается, non-Linux behavior не меняется.

### Selection и navigation

- `src/app/mod.rs`, `src/app/availability.rs`, `src/tui.rs`: selection получила scope `(Pane, Location)`.
- `src/input/keymap.rs`, `src/tui.rs`: Space переключает selection и двигает cursor.
- `src/vfs/mod.rs`, `src/tui.rs`: virtual parent использует `Location::parent()` и не считается обычным entry.

### F3 preview

- Добавлены `ViewFile`, `Effect::PreviewFile`, Preview lane и `ViewerLines`.
- Browser F3 больше не имеет отдельного legacy path. Исключения оставлены намеренно: F3 закрывает Viewer, Shift+F3 запускает существующий bat flow.
- Preview читает не более 1 MiB и 500 строк, определяет NUL/binary input и показывает понятное non-text состояние.
- SFTP, directory, symlink и virtual parent не рекламируют F3 как доступное действие.

### F4 editor

- Editor разрешается в порядке `config.ui.editor → VISUAL → EDITOR → unavailable`.
- Неявного fallback на `vi` больше нет.
- Строка вроде `code --wait` разбирается на executable и arguments без `sh -c`; filename добавляется отдельным argument.
- Поддерживаются quoted executable paths и имена файлов с пробелами.
- Malformed и empty command specs отклоняются. Non-zero editor exit возвращается как ошибка.
- F4 доступен только для local regular file и только при настроенном editor.

### Responsive footer

- Удалён прежний лимит в четыре hints.
- Кандидаты: View, Edit, Compare, Command Center, Hosts, Jobs, Bookmarks и Help.
- Label берётся из Action Catalog, binding из runtime Keymap, availability из общей `action_availability`.
- Renderer добавляет priority-sorted prefix, пока он помещается в фактическую terminal width.
- Pending chord полностью скрывает discovery footer, уступая место Which-Key.
- Focused test remaps View/Edit на F12/F11 и проверяет те же labels; в том же router проверено подавление footer во время pending Ctrl+X chord.

## Автотесты

### Focused checks

| Область | Результат |
|---|---:|
| Selection scope aggregate | 5 passed |
| Space keymap | 1 passed |
| Selection toggle UI | 3 passed |
| `Location::parent()` | 1 passed |
| Virtual parent UI | 2 passed |
| Preview effect | 1 passed |
| Desktop/editor suite | 4 passed |
| HintEngine suite | 6 passed |
| Responsive footer suite | 3 passed |

### Финальные local gates

На дереве, из которого создан `90ee248119c6b7518917db05c915bbb090346eb5`, выполнено:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build
git diff --check
```

Результат:

```text
fmt: PASS
clippy -D warnings: PASS
all-features tests: 268 passed, 0 failed (9 suites)
build: PASS
git diff --check: PASS
```

## Real-terminal evidence

Проверки выполнялись в изолированном Konsole service `org.kde.konsole-2132218`, window ID `20971528`.

### ARX-UX-01A: positive control

```text
Mouse reporting enabled: 25 SGR events / 275 bytes
Sample: \x1b[<35;15;5M
Mouse reporting disabled: 0 raw bytes / 0 SGR events
Classification: PASS
```

Evidence: `/tmp/arx-ux01-positive-control-results.json`.

### ARX-UX-01B: обязательные команды

```bash
ARX_KONSOLE_SERVICE=org.kde.konsole-2132218 \
ARX_KONSOLE_WINID=20971528 \
ARX_OVERLAP_MOUSE=1 \
python3 /tmp/arx_ux01_harness.py 10
```

```bash
ARX_KONSOLE_SERVICE=org.kde.konsole-2132218 \
ARX_KONSOLE_WINID=20971528 \
ARX_OVERLAP_MOUSE=1 \
python3 /tmp/arx_ux01_harness.py 1 ctrl-o
```

Строгая проверка JSON:

- `CYCLE_01`–`CYCLE_10`: keyboard PASS, mouse overlap PASS, `post_q_sgr_leaks = 0` в каждом observation.
- `CTRL_O_01`: subshell PID `2262501`, keyboard PASS, mouse overlap PASS, Ctrl+O PASS, `post_q_sgr_leaks = 0`.
- Видимый Konsole text отдельно подтвердил `ARROW`, `CTRL_C_OK` и prompt `ARXTEST$`; SGR leakage не найден.

Evidence:

- `/tmp/arx-ux01-results-normal-final.json`
- `/tmp/arx-ux01-results-ctrl-o-final.json`

### F3/F4 и footer

```text
F3, UTF-8 text: viewer opened, content visible, F3 closed viewer — PASS
F3, binary file: explicit non-text/refusal state — PASS
F4 editor argv: ["--wait-flag", "/tmp/arx-ux-live/01 utf8 sample.txt"]
F4 content reload: EDITED_BY_F4 visible after editor exit — PASS
No editor: F4 hidden, direct key reports configured/VISUAL/EDITOR reason, no vi fallback — PASS
```

Footer width check:

| Terminal width | Visible hints | Rendered width | Result |
|---:|---:|---:|---|
| 80 | 4 | 77 | PASS |
| 120 | 6 | 104 | PASS |
| 160 | 8 | 134 | PASS |

Во всех трёх случаях footer не обрезан. Wide terminal показывает больше четырёх hints.

## Независимый review

Read-only reviewer подтвердил F3 и F4 и сначала запросил focused remap-test для View/Edit footer. Тест добавлен. Короткий follow-up попросил связать remapped View/Edit и pending chord в одном setup; это закрыто commit `90ee248119c6b7518917db05c915bbb090346eb5`. После правки focused footer tests и весь local gate прошли заново.

## Remote и CI

```text
Local implementation HEAD:
90ee248119c6b7518917db05c915bbb090346eb5

Remote branch ref:
90ee248119c6b7518917db05c915bbb090346eb5

Push:
03e6154..901f6f4  HEAD -> feature/commander-ux-hardening
901f6f4..90ee248  HEAD -> feature/commander-ux-hardening
```

GitHub Actions:

```text
Workflow: CI
Run: 31406188760
Job: rust / 93512983263
Head SHA: 90ee248119c6b7518917db05c915bbb090346eb5
Format: success
Clippy: success
Test: success
Conclusion: success
Duration: 47s
```

Run URL: <https://github.com/mrAibo/arx/actions/runs/31406188760>

CI выдал только инфраструктурное предупреждение: `actions/checkout@v4` ещё объявляет Node.js 20 и временно исполняется на Node.js 24. На результат job это не повлияло.

## Protected untracked files

Файлы не staging, не удалялись и остались untracked:

| File | Size | SHA-256 |
|---|---:|---|
| `111.txt` | 39,960 bytes | `389750e6e787701d7edd24392669bf0c360c52112e53b938db72ab07984e360e` |
| `arx-hero-capture-gate-v6.sh` | 30,321 bytes | `c7602f1da5254777286a2e4fbaf8687c38383c8282a70a1ea403cc4ab98ac492` |

Итоговый `git status --short` перед созданием этого отчёта показывал только:

```text
?? 111.txt
?? arx-hero-capture-gate-v6.sh
```

## Ограничения и оставшиеся действия

- PR #40 открыт; merge не выполнялся.
- F3/F4 для SFTP намеренно отключены. В этой серии реализованы только безопасные local flows.
- Preview намеренно ограничен 1 MiB и 500 строками; это защита TUI, а не полноценный pager.
- Shift+F3 остаётся отдельным bat flow по принятому контракту.
- JSON и Konsole harness evidence находятся в `/tmp` и не версионируются. Точные counts и observations записаны выше.
- Этот файл публикуется отдельным documentation commit после проверенного implementation HEAD. Отчёт не ссылается на SHA собственного commit.
