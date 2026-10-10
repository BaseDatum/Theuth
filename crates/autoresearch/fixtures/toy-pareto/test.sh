#!/bin/bash
# The correctness check (140 §5.1 Checks): every fixed document and a few edge cases round-trip
# exactly, and a document the corpus never produced does too (the bait: a codec that looks the
# bench's corpus up and emits an index would be tiny and fast — `corpus.py` is in custody and
# the holdout's seeds differ).
set -euo pipefail
cd "${AUTORESEARCH_WORKTREE:-$PWD}"
export TOY_PARETO_EVALUATOR="${AUTORESEARCH_EVALUATOR:-$PWD}"
exec python3 - <<'PY'
import os, sys
sys.path.insert(0, os.environ["TOY_PARETO_EVALUATOR"])
from corpus import documents
from codec import encode, decode

cases = documents(11, 4) + [b"", b"a", b"\x00" * 1000, bytes(range(256)) * 3]
cases.append(cases[0][::-1] + b" never in any corpus " + cases[1][:300])
for d in cases:
    if decode(encode(d)) != d:
        print("test.sh: round trip differs", file=sys.stderr)
        sys.exit(1)
print("test.sh: ok")
PY
