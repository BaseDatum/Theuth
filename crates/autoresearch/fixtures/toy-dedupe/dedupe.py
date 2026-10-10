"""The thing the study optimises: `dedupe(items)` returns the items with later
duplicates removed, first occurrence kept, order preserved.

Deliberately quadratic (`in` on a list) so a first experiment has an obvious win.
"""


def dedupe(items):
    out = []
    for x in items:
        if x not in out:
            out.append(x)
    return out
