#!/bin/bash
# world.rollback (159 §11): take one arm down; idempotent (setup runs it once on the base).
set -euo pipefail
cd "${AUTORESEARCH_WORKTREE:-$PWD}"
exec python3 "${AUTORESEARCH_EVALUATOR:-.}/world.py" rollback "${AUTORESEARCH_ARM:-control}"
