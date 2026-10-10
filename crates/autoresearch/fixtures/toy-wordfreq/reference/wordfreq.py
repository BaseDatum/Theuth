"""Reference solution (159 §14): the suite runner measures it at the outer study's
Start to set `best_known`. Off limits to the inner study."""

from collections import Counter


def top(words, k):
    return sorted(Counter(words).items(), key=lambda wc: (-wc[1], wc[0]))[:k]
