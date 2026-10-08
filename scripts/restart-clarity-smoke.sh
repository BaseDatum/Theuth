#!/usr/bin/env bash
# 146 RESTART-CLARITY: the P1–P6 proof (docs/design/146-restart-clarity.md) against
# one real `t --headless` engine on a scratch data dir with the real `omp` ACP
# harness and the 139 fake OpenAI provider as the parent's model. Never touches
# ~/.theuth. Not CI; evidence goes to the design file's as-built section.
#
#   scripts/restart-clarity-smoke.sh [P1 P4 P5 P6]      # default: every proof
#
# Env: T_BIN (default target/debug/t; built when missing), SMOKE_DIR (default
# /tmp/146-smoke, wiped), SMOKE_PORT (7961; +1 mesh bind, +5 the P5 second
# engine), FAKE_PORT (7960), OMP_BIN (omp), CHILD_SLEEP_SECS (150), BACKUP_DB
# (P6: a `sqlite3 ~/.theuth/sessions.sqlite ".backup <file>"` copy).
set -euo pipefail
cd "$(dirname "$0")/.."
T_BIN="${T_BIN:-${CARGO_TARGET_DIR:-target}/debug/t}"
if [ ! -x "$T_BIN" ]; then
    echo "building $T_BIN"
    cargo build -p theuth --bin t
fi
command -v bun >/dev/null || { echo "bun is required (scripts/restart-clarity/smoke.ts)"; exit 1; }
command -v "${OMP_BIN:-omp}" >/dev/null || { echo "omp is required on PATH (or OMP_BIN)"; exit 1; }
port="${SMOKE_PORT:-7961}"
for p in "$port" $((port + 1)) $((port + 5)) "${FAKE_PORT:-7960}"; do
    if lsof -nP -iTCP:"$p" -sTCP:LISTEN >/dev/null 2>&1 || lsof -nP -iUDP:"$p" >/dev/null 2>&1; then
        echo "port $p is in use; another engine or smoke is running"; exit 1
    fi
done
export T_BIN
exec bun scripts/restart-clarity/smoke.ts "$@"
