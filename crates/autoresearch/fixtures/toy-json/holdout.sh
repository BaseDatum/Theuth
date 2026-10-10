#!/bin/bash
# The holdout (140 §5.4): the same objective on sizes and seeds the search never
# scores, with a correctness check of its own against `json.loads`.
set -euo pipefail
cd "${AUTORESEARCH_WORKTREE:-$PWD}"
exec python3 - <<'PY'
import json, os, random, sys, time
from jsonparse import parse

# 159 R-W7-6: the suite runner draws the seed per outer study and passes it in the
# environment; it is never in the repo, so a session tuned to the holdout cannot see it.
SEED = int(os.environ.get("AUTORESEARCH_HOLDOUT_SEED", "0"))


def doc(rng, n):
    """A seeded JSON document of about n records."""
    words = ["alpha", "beta", "gamma", "delta", "tab\there", "quote\"q", "uni\u00e9", "slash/x"]
    recs = []
    for i in range(n):
        recs.append({
            "id": i,
            "name": rng.choice(words),
            "score": round(rng.random() * 100, 3),
            "tags": [rng.choice(words) for _ in range(rng.randrange(4))],
            "ok": rng.random() < 0.5,
            "nested": {"x": rng.randrange(1000), "y": None, "z": [1, 2.5, -3]},
        })
    return json.dumps({"records": recs, "count": n}, indent=1)

total = 0.0
for n, seed in ((100, 101), (250, 202), (400, 303), (200, 404)):
    text = doc(random.Random(seed + SEED), n)
    t0 = time.perf_counter()
    out = parse(text)
    ms = (time.perf_counter() - t0) * 1000.0
    if out != json.loads(text):
        print(f"holdout: wrong output for n={n} seed={seed}", file=sys.stderr)
        sys.exit(1)
    total += ms
    print(f"CASE n{n}s{seed + SEED} time_ms={ms:.3f}")
print(f"METRIC time_ms={total:.3f}")
PY
