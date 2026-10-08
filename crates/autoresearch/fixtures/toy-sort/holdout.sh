#!/bin/bash
# The holdout (140 §5.4): the same objective on sizes and seeds the search never
# scores, with a correctness check of its own (a cheat that returns the input
# unsorted is caught here even if the benchmark never looks).
set -euo pipefail
cd "${AUTORESEARCH_WORKTREE:-$PWD}"
exec python3 - <<'PY'
import random, sys, time
from sorter import sort

total = 0.0
for n, seed in ((2500, 101), (6000, 202), (10000, 303), (7000, 404)):
    rng = random.Random(seed)
    xs = [rng.randrange(-10**9, 10**9) for _ in range(n)]
    t0 = time.perf_counter()
    out = sort(xs)
    ms = (time.perf_counter() - t0) * 1000.0
    if out != sorted(xs):
        print(f"holdout: wrong output for n={n} seed={seed}", file=sys.stderr)
        sys.exit(1)
    total += ms
    print(f"CASE n{n}s{seed} time_ms={ms:.3f}")
print(f"METRIC time_ms={total:.3f}")
PY
