#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy>=1.26"]
# ///
"""Turn Kyutai's Pocket TTS voice states into the `emb` voice files ptts reads (design 43 §5).

Run by a maintainer, never at build time; the output is committed under
crates/voice/data/pocket-tts-en-2026-10/voices/ and installed by scripts/fetch-models.sh.

    uv run crates/voice/build-voices/from_state.py                    # peter_yearsley alba
    uv run crates/voice/build-voices/from_state.py eve --cache /tmp/x # other voices / cache dir

Why: `languages/english_2026-10/embeddings/<name>.safetensors` in kyutai/pocket-tts-without-voice-cloning
is not an embedding but the flow LM's KV cache after the voice prompt (6 layers × `self_attn/cache`
[2, 1, T, 16, 64], K roped), and ptts 0.7.1 reads only `emb` [1, T, 1024] (or `speaker_wavs`
latents, which need the zeroed Mimi encoder to make). This checkpoint also prompts with
`[bos_before_voice, voice]` (`insert_bos_before_voice: true` in Kyutai's english_2026-10.yaml), which
ptts does not know about, so row 0 of the written `emb` is that learnt BOS and ptts's
`prompt_audio(emb)` rebuilds Kyutai's cache exactly.

How: the voice rows are `x_t = speaker_proj · z_t` (z_t a 32-dim Mimi latent). Layer 0's V gives
LayerNorm(x_t) linearly, so least squares over [speaker_proj, 1] gives x_t up to its scale s_t; with
the scale-free layer-0 attention output in hand, s_t is the 1-D minimiser of layer 1's V error. Then
the whole prompt is run through all six layers and every K and V is compared with Kyutai's cache:
the run fails unless each layer's relative error is below 1e-4.

Inputs come from the pinned revision below (cached under cache/); output metadata records their
sha256 and the reconstruction error.
"""

import argparse
import hashlib
import json
import sys
import urllib.request
from pathlib import Path

import numpy as np

REPO = "kyutai/pocket-tts-without-voice-cloning"
REV = "a7056f505a4a8ca72d8b5fb1440071d6f1a35739"  # 2026-10-21
FOLDER = "languages/english_2026-10"
HERE = Path(__file__).resolve().parent
OUT = HERE.parent / "data/pocket-tts-en-2026-10/voices"
LAYERS, HEADS, HEAD_DIM, MAX_PERIOD, EPS = 6, 16, 64, 10_000.0, 1e-5
TOLERANCE = 1e-4


def fetch(cache: Path, rel: str) -> Path:
    path = cache / rel
    if not path.is_file():
        path.parent.mkdir(parents=True, exist_ok=True)
        url = f"https://huggingface.co/{REPO}/resolve/{REV}/{FOLDER}/{rel}"
        print(f"fetching {url}", file=sys.stderr)
        urllib.request.urlretrieve(url, path.with_suffix(".part"))
        path.with_suffix(".part").rename(path)
    return path


def read_safetensors(path: Path, keep=lambda name: True) -> dict[str, np.ndarray]:
    raw = path.read_bytes()
    n = int.from_bytes(raw[:8], "little")
    header = json.loads(raw[8 : 8 + n])
    out = {}
    for name, info in header.items():
        if name == "__metadata__" or not keep(name):
            continue
        a, b = info["data_offsets"]
        buf = raw[8 + n + a : 8 + n + b]
        if info["dtype"] == "BF16":
            arr = (np.frombuffer(buf, "<u2").astype(np.uint32) << 16).view(np.float32)
        elif info["dtype"] == "F32":
            arr = np.frombuffer(buf, "<f4")
        elif info["dtype"] == "I64":
            arr = np.frombuffer(buf, "<i8")
        else:
            raise SystemExit(f"{path}: {name}: unexpected dtype {info['dtype']}")
        out[name] = arr.reshape(info["shape"]).astype(np.float64 if arr.dtype.kind == "f" else np.int64)
    return out


def write_emb(path: Path, emb: np.ndarray, metadata: dict[str, str]) -> None:
    data = emb.astype("<f4").tobytes()
    header = {"__metadata__": metadata, "emb": {"dtype": "F32", "shape": list(emb.shape), "data_offsets": [0, len(data)]}}
    blob = json.dumps(header, separators=(",", ":")).encode()
    blob += b" " * (-len(blob) % 8)
    path.write_bytes(len(blob).to_bytes(8, "little") + blob + data)


def layer_norm(x, w, b):
    mu = x.mean(-1, keepdims=True)
    var = ((x - mu) ** 2).mean(-1, keepdims=True)
    return (x - mu) / np.sqrt(var + EPS) * w + b


def rope(x, pos):  # x [T, H, D]; interleaved (real, imaginary) pairs, as pocket_tts/modules/rope.py
    freqs = np.exp(np.arange(HEAD_DIM // 2) * (-np.log(MAX_PERIOD) * 2 / HEAD_DIM))
    ang = pos[:, None, None] * freqs
    c, s = np.cos(ang), np.sin(ang)
    re, im = x[..., 0::2], x[..., 1::2]
    out = np.empty_like(x)
    out[..., 0::2] = re * c - im * s
    out[..., 1::2] = re * s + im * c
    return out


def gelu_tanh(x):
    return 0.5 * x * (1 + np.tanh(np.sqrt(2 / np.pi) * (x + 0.044715 * x**3)))


def attend(q, k, v):  # causal, q/k/v [T, H, D] at positions 0..T-1 → [T, H*D]
    t = q.shape[0]
    scores = np.einsum("thd,shd->hts", q, k) / np.sqrt(HEAD_DIM)
    scores[:, np.triu(np.ones((t, t), dtype=bool), 1)] = -np.inf
    scores = np.exp(scores - scores.max(-1, keepdims=True))
    scores /= scores.sum(-1, keepdims=True)
    return np.einsum("hts,shd->thd", scores, v).reshape(t, HEADS * HEAD_DIM)


def qkv(w, n, pos):
    proj = n @ w["in_proj"].T
    q, k, v = (proj[:, i * 1024 : (i + 1) * 1024].reshape(-1, HEADS, HEAD_DIM) for i in range(3))
    return rope(q, pos), rope(k, pos), v


def feed_forward(w, h):
    return h + gelu_tanh(layer_norm(h, w["norm2.weight"], w["norm2.bias"]) @ w["linear1"].T) @ w["linear2"].T


def forward(ws, x):
    """Run the prompt through every layer; return each layer's (K, V) as the cache holds them."""
    pos = np.arange(x.shape[0], dtype=np.float64)
    caches = []
    for w in ws:
        q, k, v = qkv(w, layer_norm(x, w["norm1.weight"], w["norm1.bias"]), pos)
        caches.append((k, v))
        x = feed_forward(w, x + attend(q, k, v) @ w["out_proj"].T)
    return caches


def rel(a, b):
    return float(np.linalg.norm(a - b) / np.linalg.norm(b))


def convert(weights: dict, state_path: Path) -> tuple[np.ndarray, list[float]]:
    st = read_safetensors(state_path)
    cache = [st[f"transformer.layers.{i}.self_attn/cache"][:, 0] for i in range(LAYERS)]  # [2, T, H, D]
    t = cache[0].shape[1]
    for i in range(LAYERS):
        offset, pad = int(st[f"transformer.layers.{i}.self_attn/offset"][0]), int(st[f"transformer.layers.{i}.self_attn/pad"][0])
        if offset != t or pad != 0:
            raise SystemExit(f"{state_path}: layer {i}: offset {offset} pad {pad}, expected {t} and 0")
    ws = weights["layers"]
    proj, bos = weights["speaker_proj"], weights["bos"]
    w0, w1 = ws[0], ws[1]
    pos = np.arange(t, dtype=np.float64)

    # Row 0 must be the learnt BOS, or this is not an insert_bos_before_voice state.
    k_bos, v_bos = forward(ws[:1], bos[None])[0]
    if rel(v_bos[0], cache[0][1, 0]) > TOLERANCE or rel(k_bos[0], cache[0][0, 0]) > TOLERANCE:
        raise SystemExit(f"{state_path}: position 0 is not flow_lm.bos_before_voice")

    # LayerNorm(x_t) = g ⊙ (proj·a_t + c_t) + b with x_t = s_t · proj·a_t; V0 = Wv · LayerNorm(x).
    wv = w0["in_proj"][2048:]
    design = wv @ (w0["norm1.weight"][:, None] * np.c_[proj, np.ones(1024)])
    rhs = cache[0][1, 1:].reshape(t - 1, -1) - wv @ w0["norm1.bias"]
    sol = np.linalg.lstsq(design, rhs.T, rcond=None)[0]
    y = sol[:32].T @ proj.T  # proj·a_t, [T-1, 1024]
    normed = np.vstack([layer_norm(bos[None], w0["norm1.weight"], w0["norm1.bias"]),
                        w0["norm1.weight"] * (y + sol[32][:, None]) + w0["norm1.bias"]])
    q0, _, _ = qkv(w0, normed, pos)
    attn0 = (attend(q0, cache[0][0], cache[0][1]) @ w0["out_proj"].T)[1:]
    v1 = cache[1][1, 1:].reshape(t - 1, -1)

    def v1_error(scale):  # [T-1] scales → [T-1] squared errors of layer 1's V
        h = feed_forward(w0, scale[:, None] * y + attn0)
        pred = layer_norm(h, w1["norm1.weight"], w1["norm1.bias"]) @ w1["in_proj"][2048:].T
        return ((pred - v1) ** 2).sum(-1)

    # Coarse grid in log scale, then golden-section search around each token's best point.
    grid = np.linspace(np.log(1e-3), np.log(1e3), 121)
    errs = np.stack([v1_error(np.full(t - 1, np.exp(g))) for g in grid])
    best = errs.argmin(0)
    lo, hi = grid[np.maximum(best - 1, 0)], grid[np.minimum(best + 1, len(grid) - 1)]
    ratio = (np.sqrt(5) - 1) / 2
    for _ in range(60):
        m1, m2 = hi - ratio * (hi - lo), lo + ratio * (hi - lo)
        left = v1_error(np.exp(m1)) < v1_error(np.exp(m2))
        hi, lo = np.where(left, m2, hi), np.where(left, lo, m1)
    scale = np.exp((lo + hi) / 2)

    emb = np.vstack([bos[None], scale[:, None] * y])
    errors = [max(rel(k, c[0]), rel(v, c[1])) for (k, v), c in zip(forward(ws, emb), cache)]
    return emb, errors


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("names", nargs="*", default=["peter_yearsley", "alba"])
    ap.add_argument("--cache", type=Path, default=HERE / "cache", help="mirror of the folder at REV (default: cache/)")
    ap.add_argument("--out", type=Path, default=OUT)
    args = ap.parse_args()

    model = fetch(args.cache, "model.safetensors")
    keep = lambda n: n in ("flow_lm.bos_before_voice", "flow_lm.speaker_proj_weight") or n.startswith("flow_lm.transformer.layers.")
    raw = read_safetensors(model, keep)
    names = {"in_proj": "self_attn.in_proj.weight", "out_proj": "self_attn.out_proj.weight",
             "linear1": "linear1.weight", "linear2": "linear2.weight"}
    layers = []
    for i in range(LAYERS):
        p = f"flow_lm.transformer.layers.{i}."
        w = {k: raw[p + v] for k, v in names.items()}
        w |= {k: raw[p + k] for k in ("norm1.weight", "norm1.bias", "norm2.weight", "norm2.bias")}
        layers.append(w)
    weights = {"layers": layers, "speaker_proj": raw["flow_lm.speaker_proj_weight"], "bos": raw["flow_lm.bos_before_voice"][0, 0]}
    model_sha = hashlib.sha256(model.read_bytes()).hexdigest()

    args.out.mkdir(parents=True, exist_ok=True)
    for name in args.names:
        state = fetch(args.cache, f"embeddings/{name}.safetensors")
        emb, errors = convert(weights, state)
        worst = max(errors)
        print(f"{name}: {emb.shape[0]} rows (BOS + {emb.shape[0] - 1}), K/V relative error per layer "
              + " ".join(f"{e:.1e}" for e in errors))
        if worst > TOLERANCE:
            raise SystemExit(f"{name}: reconstruction error {worst:.2e} > {TOLERANCE}; not written")
        write_emb(args.out / f"{name}.safetensors", emb[None], {
            "source": f"hf://{REPO}@{REV}/{FOLDER}/embeddings/{name}.safetensors",
            "source_sha256": hashlib.sha256(state.read_bytes()).hexdigest(),
            "model_sha256": model_sha,
            "row0": "flow_lm.bos_before_voice",
            "max_kv_relative_error": f"{worst:.2e}",
            "made_by": "crates/voice/build-voices/from_state.py",
        })


if __name__ == "__main__":
    main()
