"""The thing the study optimises: `top(words, k)` returns the k most frequent words as
`(word, count)` pairs, most frequent first, ties broken by the word.

Deliberately quadratic (`list.count` per distinct word) so a first experiment has an
obvious win.
"""


def top(words, k):
    seen = []
    for w in words:
        if w not in seen:
            seen.append(w)
    counts = [(w, words.count(w)) for w in seen]
    counts.sort(key=lambda wc: (-wc[1], wc[0]))
    return counts[:k]
