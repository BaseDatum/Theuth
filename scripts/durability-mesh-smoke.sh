#!/usr/bin/env bash
# 139 Z: the two-engine durability matrix (design 139 §11 Z, T1–T8) against
# two real `t --headless` engines on this Mac, paired over the loopback mesh,
# with a scripted OpenAI-compatible provider as their model. Not CI; evidence
# goes to docs/design/139-smoke.md. Shape: 68-two-engine-smoke.md §1,
# 109-as-built-W6.md §8.
#
#   scripts/durability-mesh-smoke.sh [T1 T2 …]      # default: every row
#
# Env: T_BIN (default target/debug/t; built when missing), SMOKE_DIR
# (default /tmp/139-z, wiped), FAKE_PORT (7950). Ports 7931/7932 (rpc) and
# 7941/7942 (mesh bind) must be free.
set -euo pipefail
cd "$(dirname "$0")/.."
T_BIN="${T_BIN:-${CARGO_TARGET_DIR:-target}/debug/t}"
if [ ! -x "$T_BIN" ]; then
    echo "building $T_BIN"
    cargo build -p theuth --bin t
fi
command -v bun >/dev/null || { echo "bun is required (scripts/durability-mesh/*.ts)"; exit 1; }
for p in 7931 7932 7941 7942 "${FAKE_PORT:-7950}"; do
    if lsof -nP -iTCP:"$p" -sTCP:LISTEN >/dev/null 2>&1 || lsof -nP -iUDP:"$p" >/dev/null 2>&1; then
        echo "port $p is in use; another engine or smoke is running"; exit 1
    fi
done
export T_BIN
exec bun scripts/durability-mesh/smoke.ts "$@"
