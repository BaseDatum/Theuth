#!/usr/bin/env bash
# toy-judged generation step (159 §8): the plugin sets AUTORESEARCH_OUTPUT and
# AUTORESEARCH_CASES; the artefact writes one blurb per case there. No metric is scored
# here — the blind pair judge scores the outputs.
set -euo pipefail
cd "${AUTORESEARCH_WORKTREE:-$PWD}"
exec python3 blurb.py
