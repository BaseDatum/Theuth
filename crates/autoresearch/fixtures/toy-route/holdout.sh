#!/bin/bash
# holdout.command: other seeds and sizes, verifies the tour itself.
set -euo pipefail
cd "${AUTORESEARCH_WORKTREE:-$PWD}"
exec python3 - <<'PY'
import math, os, random, sys
from router import solve

# 159 R-W7-6: the suite runner draws the seed per outer study and passes it in the
# environment; it is never in the repo, so a session tuned to the holdout cannot see it.
SEED = int(os.environ.get("AUTORESEARCH_HOLDOUT_SEED", "0"))
CASES = ((101, 50), (202, 80), (303, 60), (404, 70))
lengths = []
for seed, n in CASES:
    rng = random.Random(seed + SEED)
    cities = [(rng.uniform(0, 1000), rng.uniform(0, 1000)) for _ in range(n)]
    tour = list(solve(cities))
    if sorted(tour) != list(range(n)):
        print(f"holdout: seed {seed}: not a tour", file=sys.stderr)
        sys.exit(1)
    L = sum(math.dist(cities[tour[i]], cities[tour[(i + 1) % n]]) for i in range(n))
    lengths.append(L)
    print(f"CASE s{seed + SEED}n{n} length={L:.1f}")
print(f"METRIC length={sum(lengths) / len(lengths):.2f}")
PY
