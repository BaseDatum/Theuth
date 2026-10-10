#!/bin/bash
# holdout.command: other seeds and sizes, verifies the tour itself.
set -euo pipefail
cd "${AUTORESEARCH_WORKTREE:-$PWD}"
exec python3 - <<'PY'
import math, random, sys
from router import solve

CASES = ((101, 50), (202, 80), (303, 60), (404, 70))
lengths = []
for seed, n in CASES:
    rng = random.Random(seed)
    cities = [(rng.uniform(0, 1000), rng.uniform(0, 1000)) for _ in range(n)]
    tour = list(solve(cities))
    if sorted(tour) != list(range(n)):
        print(f"holdout: seed {seed}: not a tour", file=sys.stderr)
        sys.exit(1)
    L = sum(math.dist(cities[tour[i]], cities[tour[(i + 1) % n]]) for i in range(n))
    lengths.append(L)
    print(f"CASE s{seed}n{n} length={L:.1f}")
print(f"METRIC length={sum(lengths) / len(lengths):.2f}")
PY
