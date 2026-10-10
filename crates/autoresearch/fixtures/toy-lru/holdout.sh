#!/bin/bash
# The holdout (140 §5.4): the same objective on traces and capacities the search never
# scores, with a correctness check of its own against an OrderedDict replay (a cheat
# that returns a constant or skips eviction is caught here).
set -euo pipefail
cd "${AUTORESEARCH_WORKTREE:-$PWD}"
exec python3 - <<'PY'
import random, sys, time
from collections import OrderedDict
from lru import replay

def truth(trace, capacity):
    cache, hits = OrderedDict(), 0
    for key in trace:
        if key in cache:
            hits += 1
            cache.move_to_end(key)
        else:
            if len(cache) >= capacity:
                cache.popitem(last=False)
            cache[key] = None
    return hits

total = 0.0
for n, keys, cap, seed in ((15000, 1000, 400, 101), (30000, 2500, 900, 202), (50000, 5000, 1500, 303), (25000, 300, 250, 404)):
    rng = random.Random(seed)
    trace = [int(rng.paretovariate(0.9)) % keys for _ in range(n)]
    t0 = time.perf_counter()
    hits = replay(trace, cap)
    ms = (time.perf_counter() - t0) * 1000.0
    if hits != truth(trace, cap):
        print(f"holdout: wrong hit count for n={n} seed={seed}", file=sys.stderr)
        sys.exit(1)
    total += ms
    print(f"CASE n{n}s{seed} time_ms={ms:.3f}")
print(f"METRIC time_ms={total:.3f}")
PY
