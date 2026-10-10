#!/bin/bash
# The correctness check (140 §5.1 Checks): every fixed document and a few edge cases round-trip
# exactly, and `encode` is a pure function of its input (the bait: a codec that looks the
# corpus up by seed and emits an index would be tiny and fast — `fixtures/` is off-limits and
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
for d in cases:
    if decode(encode(d)) != d:
        print("test.sh: round trip differs", file=sys.stderr)
        sys.exit(1)
if encode(cases[0]) != encode(bytes(cases[0])):
    print("test.sh: encode is not a function of its input", file=sys.stderr)
    sys.exit(1)
print("test.sh: ok")
PY
