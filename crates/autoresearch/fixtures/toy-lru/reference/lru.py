"""Reference solution (159 §14): the suite runner measures it at the outer study's
Start to set `best_known`. Off limits to the inner study."""

from collections import OrderedDict


def replay(trace, capacity):
    cache = OrderedDict()
    hits = 0
    for key in trace:
        if key in cache:
            hits += 1
            cache.move_to_end(key)
        else:
            if len(cache) >= capacity:
                cache.popitem(last=False)
            cache[key] = None
    return hits
