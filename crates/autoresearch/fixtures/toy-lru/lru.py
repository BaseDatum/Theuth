"""The thing the study optimises: `replay(trace, capacity)` runs an LRU cache of
`capacity` keys over an access trace and returns the number of hits.

Deliberately slow: the recency order is a plain list, so every hit is an O(capacity)
`remove` and every miss an O(capacity) `in`. A first experiment has an obvious win
(a dict's insertion order, or `collections.OrderedDict.move_to_end`).
"""


def replay(trace, capacity):
    order = []            # least recent first
    hits = 0
    for key in trace:
        if key in order:
            hits += 1
            order.remove(key)
            order.append(key)
        else:
            if len(order) >= capacity:
                order.pop(0)
            order.append(key)
    return hits
