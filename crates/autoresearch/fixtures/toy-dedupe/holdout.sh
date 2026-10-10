#!/bin/bash
# The holdout (140 §5.4): the same objective on sizes and seeds the search never
# scores, with a correctness check of its own (order and first-occurrence are checked
# against `dict.fromkeys`, so a cheat that sorts or drops items is caught here).
set -euo pipefail
cd "${AUTORESEARCH_WORKTREE:-$PWD}"
exec python3 - <<'PY'
import os, random, sys, time
from dedupe import dedupe

# 159 R-W7-6: the suite runner draws the seed per outer study and passes it in the
# environment; it is never in the repo, so a session tuned to the holdout cannot see it.
SEED = int(os.environ.get("AUTORESEARCH_HOLDOUT_SEED", "0"))

total = 0.0
for n, k, seed in ((4000, 900, 101), (9000, 2200, 202), (15000, 4000, 303), (11000, 500, 404)):
    rng = random.Random(seed + SEED)
    xs = [f"k{rng.randrange(k)}" for _ in range(n)]
    t0 = time.perf_counter()
    out = dedupe(xs)
    ms = (time.perf_counter() - t0) * 1000.0
    if out != list(dict.fromkeys(xs)):
        print(f"holdout: wrong output for n={n} seed={seed}", file=sys.stderr)
        sys.exit(1)
    total += ms
    print(f"CASE n{n}s{seed + SEED} time_ms={ms:.3f}")
print(f"METRIC time_ms={total:.3f}")
PY
