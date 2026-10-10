#!/bin/bash
# The study's benchmark (159 §6, §14 toy-pareto): a fixed seeded corpus through the codec,
# `METRIC time_ms` (encode + decode, lower) and `METRIC size_kb` (Σ encoded bytes, lower).
# cwd = the worktree (the harness sets it); the corpus module ships in custody, so it is
# imported from $AUTORESEARCH_EVALUATOR when set.
set -euo pipefail
cd "${AUTORESEARCH_WORKTREE:-$PWD}"
export TOY_PARETO_EVALUATOR="${AUTORESEARCH_EVALUATOR:-$PWD}"
exec python3 - <<'PY'
import os, sys, time
sys.path.insert(0, os.environ["TOY_PARETO_EVALUATOR"])
from corpus import documents
from codec import encode, decode

SEEDS = (11, 12, 13)            # fixed; holdout.sh uses others
total_ms = 0.0
total_bytes = 0
for seed in SEEDS:
    docs = documents(seed, 12)
    t0 = time.perf_counter()
    blobs = [encode(d) for d in docs]
    back = [decode(b) for b in blobs]
    ms = (time.perf_counter() - t0) * 1000.0
    if back != docs:
        print(f"bench: round trip differs for seed {seed}", file=sys.stderr)
        sys.exit(1)
    size = sum(len(b) for b in blobs)
    total_ms += ms
    total_bytes += size
    print(f"CASE s{seed} time_ms={ms:.3f} size_kb={size / 1024:.3f}")
print(f"METRIC time_ms={total_ms:.3f}")
print(f"METRIC size_kb={total_bytes / 1024:.3f}")
print(f"ASI python={sys.version.split()[0]}")
PY
