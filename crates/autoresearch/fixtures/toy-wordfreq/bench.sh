#!/bin/bash
# The study's benchmark (140 §5.1): fixed seeded workload, prints the contract lines.
# cwd = the worktree (the harness sets it); this file may run from custody, so it
# never uses its own location.
set -euo pipefail
cd "${AUTORESEARCH_WORKTREE:-$PWD}"
exec python3 - <<'PY'
import random, resource, sys, time
from wordfreq import top

# (words, vocabulary) fixed by seed; holdout.sh uses other sizes and seeds
WORK = ((20000, 400), (40000, 800), (60000, 1200))
total = 0.0
for n, v in WORK:
    rng = random.Random(n)
    words = [f"t{int(rng.paretovariate(1.2)) % v}" for _ in range(n)]
    t0 = time.perf_counter()
    out = top(words, 50)
    ms = (time.perf_counter() - t0) * 1000.0
    if len(out) != 50:
        print(f"bench: top returned {len(out)} pairs for {n}", file=sys.stderr)
        sys.exit(1)
    total += ms
    print(f"CASE n{n} time_ms={ms:.3f}")
rss = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
peak_kb = rss // 1024 if sys.platform == "darwin" else rss   # macOS reports bytes
print(f"METRIC time_ms={total:.3f}")
print(f"METRIC peak_kb={peak_kb}")
print(f"ASI python={sys.version.split()[0]}")
PY
