"""The scope: a codec over bytes. `encode(data) -> bytes`, `decode(blob) -> bytes`, round-trip exact.

The starting point is deliberately off the front: a pure-Python run-length pass (slow) feeding
zlib at level 6 (middling size). Faster codecs exist (no pre-pass, lower zlib levels, `store`),
smaller ones exist (lzma, bz2, zlib 9); none of them is both, so a study that counts both
`time_ms` and `size_kb` should keep several of them.
"""

import zlib

MAGIC = b"RL1"


def _rle(data: bytes) -> bytes:
    out = bytearray()
    i, n = 0, len(data)
    while i < n:
        b = data[i]
        j = i + 1
        while j < n and j - i < 255 and data[j] == b:
            j += 1
        out.append(j - i)
        out.append(b)
        i = j
    return bytes(out)


def _unrle(blob: bytes) -> bytes:
    out = bytearray()
    for k in range(0, len(blob), 2):
        out.extend(bytes([blob[k + 1]]) * blob[k])
    return bytes(out)


def encode(data: bytes) -> bytes:
    return MAGIC + zlib.compress(_rle(data), 6)


def decode(blob: bytes) -> bytes:
    if not blob.startswith(MAGIC):
        raise ValueError("bad magic")
    return _unrle(zlib.decompress(blob[len(MAGIC):]))
