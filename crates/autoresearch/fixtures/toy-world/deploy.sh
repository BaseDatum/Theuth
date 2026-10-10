#!/bin/bash
# world.deploy (159 §11): put one arm live in the simulator. The plugin runs this after the
# user's press with AUTORESEARCH_ARM, AUTORESEARCH_TREE, AUTORESEARCH_ARM_WORKTREE,
# AUTORESEARCH_EXPOSURE and AUTORESEARCH_N; cwd = the study worktree. Runs from custody, so it
# never uses its own location: `world.py` is resolved as a custody path too.
set -euo pipefail
cd "${AUTORESEARCH_WORKTREE:-$PWD}"
exec python3 "${AUTORESEARCH_EVALUATOR:-.}/world.py" deploy "${AUTORESEARCH_ARM:-control}"
