#!/bin/bash
# The study's benchmark (140 §5.1): fixed seeded workload, prints the contract lines.
# cwd = the worktree (the harness sets it); this file may run from custody, so it
# never uses its own location.
set -euo pipefail
cd "${AUTORESEARCH_WORKTREE:-$PWD}"
exec python3 - <<'PY'
import random, resource, sys, time
from lru import replay

# (accesses, key space, capacity) fixed by seed; holdout.sh uses other sizes and seeds
WORK = ((20000, 1500, 600), (40000, 3000, 1200), (60000, 4500, 1800))
total = 0.0
for n, keys, cap in WORK:
    rng = random.Random(n)
    trace = [int(rng.paretovariate(0.8)) % keys for _ in range(n)]
    t0 = time.perf_counter()
    hits = replay(trace, cap)
    ms = (time.perf_counter() - t0) * 1000.0
    if not 0 <= hits <= n:
        print(f"bench: replay returned {hits} hits for {n}", file=sys.stderr)
        sys.exit(1)
    total += ms
    print(f"CASE n{n} time_ms={ms:.3f}")
rss = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
peak_kb = rss // 1024 if sys.platform == "darwin" else rss   # macOS reports bytes
print(f"METRIC time_ms={total:.3f}")
print(f"METRIC peak_kb={peak_kb}")
print(f"ASI python={sys.version.split()[0]}")
PY
