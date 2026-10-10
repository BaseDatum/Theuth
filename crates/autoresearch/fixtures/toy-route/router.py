"""The scope: build a closed tour over `cities` (a list of (x, y)).

Returns a permutation of range(len(cities)). Nearest neighbour from city 0 is a
quick local optimum: 2-opt, or-opt and restarts beat it at a higher run time.
"""


def solve(cities):
    n = len(cities)
    if n == 0:
        return []
    left = set(range(1, n))
    tour = [0]
    while left:
        x, y = cities[tour[-1]]
        nxt = min(left, key=lambda j: (cities[j][0] - x) ** 2 + (cities[j][1] - y) ** 2)
        left.remove(nxt)
        tour.append(nxt)
    return tour
