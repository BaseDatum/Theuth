#!/bin/bash
# The holdout (140 §5.4): the same objective on sizes and seeds the search never
# scores, with a correctness check of its own against `collections.Counter`.
set -euo pipefail
cd "${AUTORESEARCH_WORKTREE:-$PWD}"
exec python3 - <<'PY'
import os, random, sys, time
from collections import Counter
from wordfreq import top

# 159 R-W7-6: the suite runner draws the seed per outer study and passes it in the
# environment; it is never in the repo, so a session tuned to the holdout cannot see it.
SEED = int(os.environ.get("AUTORESEARCH_HOLDOUT_SEED", "0"))

total = 0.0
for n, v, k, seed in ((15000, 300, 20, 101), (30000, 900, 50, 202), (50000, 1500, 100, 303), (25000, 100, 10, 404)):
    rng = random.Random(seed + SEED)
    words = [f"u{int(rng.paretovariate(1.1)) % v}" for _ in range(n)]
    t0 = time.perf_counter()
    out = top(words, k)
    ms = (time.perf_counter() - t0) * 1000.0
    want = sorted(Counter(words).items(), key=lambda wc: (-wc[1], wc[0]))[:k]
    if list(map(tuple, out)) != want:
        print(f"holdout: wrong output for n={n} seed={seed}", file=sys.stderr)
        sys.exit(1)
    total += ms
    print(f"CASE n{n}s{seed + SEED} time_ms={ms:.3f}")
print(f"METRIC time_ms={total:.3f}")
PY
