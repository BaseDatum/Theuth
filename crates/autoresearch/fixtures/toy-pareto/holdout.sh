#!/bin/bash
# The holdout (140 §5.4): both objectives on seeds and lengths the search never scores, with
# its own round-trip check — a codec that memorised the bench's corpus fails here.
set -euo pipefail
cd "${AUTORESEARCH_WORKTREE:-$PWD}"
export TOY_PARETO_EVALUATOR="${AUTORESEARCH_EVALUATOR:-$PWD}"
exec python3 - <<'PY'
import os, sys, time
sys.path.insert(0, os.environ["TOY_PARETO_EVALUATOR"])
from corpus import documents
from codec import encode, decode

total_ms = 0.0
total_bytes = 0
for seed, count, words in ((101, 8, 900), (202, 10, 2200), (303, 6, 1500)):
    docs = documents(seed, count, words)
    t0 = time.perf_counter()
    blobs = [encode(d) for d in docs]
    back = [decode(b) for b in blobs]
    ms = (time.perf_counter() - t0) * 1000.0
    if back != docs:
        print(f"holdout: round trip differs for seed {seed}", file=sys.stderr)
        sys.exit(1)
    total_ms += ms
    total_bytes += sum(len(b) for b in blobs)
    print(f"CASE s{seed} time_ms={ms:.3f} size_kb={total_bytes / 1024:.3f}")
print(f"METRIC time_ms={total_ms:.3f}")
print(f"METRIC size_kb={total_bytes / 1024:.3f}")
PY
