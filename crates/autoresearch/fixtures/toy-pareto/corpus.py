"""The seeded corpus (evaluator file): redundant text with a small vocabulary and a few binary
runs, so the stdlib codecs span a wide front — identity ≈ 1×, zlib ≈ 4–6×, lzma more.
`documents(seed, count)` is what the bench, the check and the holdout all read; a cheat that
memorises the bench's seeds fails the holdout's."""

import random

WORDS = (
    "the quick brown fox jumps over the lazy dog and the cat sat on the mat while "
    "ninety nine red balloons go by in the summer sky over green fields and blue lakes"
).split()


def document(rng: random.Random, words: int) -> bytes:
    parts = []
    for _ in range(words):
        parts.append(rng.choice(WORDS))
        if rng.random() < 0.08:
            parts.append("\n")
    text = " ".join(parts).encode()
    # A binary stretch with runs, every third document.
    if rng.random() < 0.34:
        run = bytes([rng.randrange(256)]) * rng.randrange(64, 512)
        text += b"\x00" + run + bytes(rng.randrange(256) for _ in range(rng.randrange(16, 64)))
    return text


def documents(seed: int, count: int, words: int = 1500) -> list:
    rng = random.Random(seed)
    return [document(rng, words) for _ in range(count)]
