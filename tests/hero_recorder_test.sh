#!/usr/bin/env bash
# Focused semantic tests for .github/scripts/record_hero.sh
# Uses command stubs on a temp PATH + controlled env to assert fail-closed
# ordering and safe output replacement. No real SSH/asciinema/agg needed.
#
# NOTE: stubs must live off a noexec mount, so the work dir is under $HOME.
set -uo pipefail

SCRIPT_UNDER_TEST="$(cd "$(dirname "$0")/.." && pwd)/.github/scripts/record_hero.sh"
PASS=0
FAIL=0
BASE="${HERMES_TEST_TMP:-$HOME/.cache/arx-hero-test}"
rm -rf "$BASE"; mkdir -p "$BASE"

make_stubs() {
    local work="$1"; local bin="$work/bin"
    mkdir -p "$bin"
    cat > "$bin/ssh" <<'STUB'
#!/usr/bin/env bash
echo "ssh $*" >> "$ORDER_LOG"
exit 0
STUB
    cat > "$bin/asciinema" <<'STUB'
#!/usr/bin/env bash
if [ "$1" = "rec" ]; then
    echo asciinema >> "$ORDER_LOG"
    [ -z "${ASCIINEMA_FAIL:-}" ] || exit 1
    echo cast > "$CAST_FILE"
fi
exit 0
STUB
    cat > "$bin/agg" <<'STUB'
#!/usr/bin/env bash
echo agg >> "$ORDER_LOG"
[ -z "${AGG_FAIL:-}" ] || exit 1
echo gif > "$2"
exit 0
STUB
    cat > "$bin/arx" <<'STUB'
#!/usr/bin/env bash
[ "$1" = "--version" ] && echo "arx 0.15.1"
exit 0
STUB
    chmod +x "$bin"/ssh "$bin"/asciinema "$bin"/agg "$bin"/arx
}

run_script() {
    local work="$1"
    local bin="$work/bin"
    export PATH="$bin:$PATH"
    export ARX_HERO_HOST="${H_HOST:-arx-demo}"
    export ARX_HERO_LOCAL="$work/local"
    export ARX_HERO_REMOTE="${H_REMOTE:-/tmp/arx-demo/app}"
    export ARX_HERO_BINARY="$bin/arx"
    export ARX_HERO_CAST="$work/rec.cast"
    export ARX_HERO_OUTPUT="$work/out.gif"
    export ORDER_LOG="$work/order.log"
    export CAST_FILE="$work/rec.cast"
    unset ASCIINEMA_FAIL AGG_FAIL
    [ -n "${H_ASCFAIL:-}" ] && export ASCIINEMA_FAIL=1
    [ -n "${H_AGGFAIL:-}" ] && export AGG_FAIL=1
    : > "$ORDER_LOG"
    bash "$SCRIPT_UNDER_TEST" >/dev/null 2>&1
    RUN_RC=$?
    RUN_ORDER="$(cat "$ORDER_LOG")"
}

# ok <desc> <shell-cond> : pass when cond true
ok() { if eval "$2"; then PASS=$((PASS+1)); echo "PASS: $1"; else FAIL=$((FAIL+1)); echo "FAIL: $1"; fi; }
no() { if eval "$2"; then FAIL=$((FAIL+1)); echo "FAIL: $1"; else PASS=$((PASS+1)); echo "PASS: $1"; fi; }

order_ok() {
    local s a g
    s=$(echo "$RUN_ORDER" | grep -n ssh | head -1 | cut -d: -f1)
    a=$(echo "$RUN_ORDER" | grep -n asciinema | head -1 | cut -d: -f1)
    g=$(echo "$RUN_ORDER" | grep -n agg | head -1 | cut -d: -f1)
    [ -n "$s" ] && [ -n "$a" ] && [ -n "$g" ] && [ "$s" -lt "$a" ] && [ "$a" -lt "$g" ]
}

# T1: prod host -> fail before any ssh mutation
work="$BASE/t1"; make_stubs "$work"
H_HOST=prod run_script "$work"
ok   "T1 host rejection fails"   '[ "$RUN_RC" -ne 0 ]'
ok   "T1 no ssh before failure"  '[ ! -s "$work/order.log" ]'

# T2: bad remote path -> fail before mutation
work="$BASE/t2"; make_stubs "$work"
H_REMOTE=/srv/app run_script "$work"
ok   "T2 remote path rejection fails" '[ "$RUN_RC" -ne 0 ]'
ok   "T2 no ssh before failure"       '[ ! -s "$work/order.log" ]'

# T3: traversal remote path -> fail before mutation
work="$BASE/t3"; make_stubs "$work"
H_REMOTE=/tmp/arx-demo/../prod run_script "$work"
ok   "T3 traversal rejection fails" '[ "$RUN_RC" -ne 0 ]'
ok   "T3 no ssh before failure"    '[ ! -s "$work/order.log" ]'

# T4: asciinema fails -> agg never called
work="$BASE/t4"; make_stubs "$work"
H_ASCFAIL=1 run_script "$work"
ok   "T4 record failure exits non-zero" '[ "$RUN_RC" -ne 0 ]'
no   "T4 agg never called"             'echo "$RUN_ORDER" | grep -q agg'

# T5: full success -> order ssh ... asciinema ... agg + outputs exist
work="$BASE/t5"; make_stubs "$work"
run_script "$work"
ok   "T5 success rc=0"            '[ "$RUN_RC" -eq 0 ]'
ok   "T5 order ssh<asciinema<agg" 'order_ok'
ok   "T5 cast produced"          '[ -s "$work/rec.cast" ]'
ok   "T5 gif produced"           '[ -s "$work/out.gif" ]'

# T6: agg fails -> existing GIF untouched, no .new left
work="$BASE/t6"; make_stubs "$work"
echo "original" > "$work/out.gif"
H_AGGFAIL=1 run_script "$work"
ok   "T6 convert failure rc!=0"  '[ "$RUN_RC" -ne 0 ]'
ok   "T6 no .new leftover"       '[ ! -e "$work/out.gif.new" ]'
ok   "T6 existing gif preserved"  '[ "$(cat "$work/out.gif")" = "original" ]'

# T7: full success -> temp GIF atomically replaces output
work="$BASE/t7"; make_stubs "$work"
run_script "$work"
ok   "T7 gif present"            '[ -s "$work/out.gif" ]'
ok   "T7 no .new leftover"      '[ ! -e "$work/out.gif.new" ]'
ok   "T7 gif content from agg"  '[ "$(cat "$work/out.gif")" = "gif" ]'

rm -rf "$BASE"
echo ""
echo "RESULT: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
