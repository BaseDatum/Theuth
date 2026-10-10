#!/bin/bash
# The study's benchmark (140 §5.1): fixed seeded workload, prints the contract lines.
# cwd = the worktree (the harness sets it); this file may run from custody, so it
# never uses its own location.
set -euo pipefail
cd "${AUTORESEARCH_WORKTREE:-$PWD}"
exec python3 - <<'PY'
import json, random, resource, sys, time
from jsonparse import parse


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

SIZES = (150, 300, 450)   # records fixed by seed; holdout.sh uses other sizes and seeds
total = 0.0
for n in SIZES:
    text = doc(random.Random(n), n)
    t0 = time.perf_counter()
    out = parse(text)
    ms = (time.perf_counter() - t0) * 1000.0
    if out.get("count") != n:
        print(f"bench: parse lost the count for {n}", file=sys.stderr)
        sys.exit(1)
    total += ms
    print(f"CASE n{n} time_ms={ms:.3f}")
rss = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
peak_kb = rss // 1024 if sys.platform == "darwin" else rss   # macOS reports bytes
print(f"METRIC time_ms={total:.3f}")
print(f"METRIC peak_kb={peak_kb}")
print(f"ASI python={sys.version.split()[0]}")
PY
