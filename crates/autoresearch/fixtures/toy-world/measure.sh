#!/bin/bash
# world.measure (159 §11): one simulated day for every live arm, printed as CASE v2 lines
# `CASE <arm> units=<n> mean=<x> var=<v> bounce=<b>` plus `CASE total units=<production>`.
set -euo pipefail
cd "${AUTORESEARCH_WORKTREE:-$PWD}"
exec python3 "${AUTORESEARCH_EVALUATOR:-.}/world.py" measure
