"""The thing the study optimises: `sort(xs)` returns a new sorted list of ints.

Deliberately quadratic (insertion sort) so a first experiment has an obvious win.
"""


def sort(xs):
    out = list(xs)
    for i in range(1, len(out)):
        key = out[i]
        j = i - 1
        while j >= 0 and out[j] > key:
            out[j + 1] = out[j]
            j -= 1
        out[j + 1] = key
    return out
