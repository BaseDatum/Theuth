#!/bin/bash
# checks.command: main.py on fixtures/input.json must print a valid tour (every city once).
set -euo pipefail
cd "${AUTORESEARCH_WORKTREE:-$PWD}"
python3 main.py < fixtures/input.json | python3 -c '
import json, sys
n = len(json.load(open("fixtures/input.json")))
tour = [int(l) for l in sys.stdin.read().split()]
if sorted(tour) != list(range(n)):
    print(f"test.sh: not a tour over {n} cities (got {len(tour)} indices)", file=sys.stderr)
    sys.exit(1)
print("test.sh: ok")
'
