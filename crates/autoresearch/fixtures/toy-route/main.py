"""CLI used by the check: cities as JSON on stdin -> the tour as indices, one per line."""
import json
import sys

from router import solve

cities = [tuple(c) for c in json.load(sys.stdin)]
for i in solve(cities):
    print(i)
