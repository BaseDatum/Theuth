#!/bin/bash
# The study's benchmark (140 §5.1): three seeded 60-city instances, prints the contract lines.
# cwd = the worktree (the harness sets it); this file may run from custody, so it
# never uses its own location.
set -euo pipefail
cd "${AUTORESEARCH_WORKTREE:-$PWD}"
exec python3 - <<'PY'
import math, random, sys, time
from router import solve

SEEDS = (11, 23, 37)   # workload fixed by seed; holdout.sh uses other seeds
N = 60

def instance(seed):
    rng = random.Random(seed)
    return [(rng.uniform(0, 1000), rng.uniform(0, 1000)) for _ in range(N)]

def length(cities, tour):
    return sum(math.dist(cities[tour[i]], cities[tour[(i + 1) % len(tour)]]) for i in range(len(tour)))

total_ms = 0.0
lengths = []
for seed in SEEDS:
    cities = instance(seed)
    t0 = time.perf_counter()
    tour = list(solve(cities))
    ms = (time.perf_counter() - t0) * 1000.0
    if sorted(tour) != list(range(N)):
        print(f"bench: seed {seed}: not a tour over {N} cities", file=sys.stderr)
        sys.exit(1)
    L = length(cities, tour)
    lengths.append(L)
    total_ms += ms
    print(f"CASE s{seed} length={L:.1f}")
print(f"METRIC length={sum(lengths) / len(lengths):.2f}")
print(f"METRIC time_ms={total_ms:.3f}")
print(f"ASI python={sys.version.split()[0]}")
PY
