#!/usr/bin/env bash
# toy-judged check: the artefact still runs and writes one non-empty blurb per case.
set -euo pipefail
cd "${AUTORESEARCH_WORKTREE:-$PWD}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
AUTORESEARCH_OUTPUT="$tmp" python3 blurb.py > /dev/null
python3 - "$tmp" <<'PY'
import json, os, sys
out = sys.argv[1]
ids = [json.loads(l)["id"] for l in open("products.jsonl", encoding="utf-8") if l.strip()]
missing = [i for i in ids if not os.path.exists(os.path.join(out, f"{i}.txt")) or os.path.getsize(os.path.join(out, f"{i}.txt")) == 0]
if missing:
    print(f"test.sh: no blurb for {missing}", file=sys.stderr)
    sys.exit(1)
print(f"test.sh: ok ({len(ids)} blurbs)")
PY
