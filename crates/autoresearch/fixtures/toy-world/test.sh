#!/bin/bash
# checks.command: the copy is a landing page — a `# ` headline, at most 120 words, no banned
# claims. Runs locally at the launch fire, before any press.
set -euo pipefail
cd "${AUTORESEARCH_WORKTREE:-$PWD}"
f=landing/copy.md
[ -f "$f" ] || { echo "test.sh: $f missing" >&2; exit 1; }
head -n1 "$f" | grep -q '^# ' || { echo "test.sh: first line is not a '# ' headline" >&2; exit 1; }
words=$(tr -s '[:space:]' '\n' < "$f" | grep -c . || true)
[ "$words" -le 120 ] || { echo "test.sh: $words words, limit 120" >&2; exit 1; }
if grep -qiE 'miracle|cure|risk-free' "$f"; then echo "test.sh: banned claim" >&2; exit 1; fi
echo "test.sh: ok ($words words)"
