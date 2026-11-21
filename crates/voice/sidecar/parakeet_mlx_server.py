"""Parakeet TDT v3 on MLX for theuth's voice crate (docs/design/202-stt-mlx-engine.md).

Two modes, both speaking newline-delimited JSON on stdout (logs on stderr only):

  server.py serve <model_dir>
      Loads the model from <model_dir> (config.json + model.safetensors, no network:
      the caller sets HF_HUB_OFFLINE=1), prints {"ready": true, "model": ..., "backend": ...}
      once, then serves stdin requests serially on this one thread (MLX state is
      thread-local):
        {"id": 1, "op": "warm"}                                   -> {"id": 1, "text": "", "ms": 12}
        {"id": 2, "op": "transcribe", "pcm16_b64": "...", "sample_rate": 16000, "words": false}
                                                                  -> {"id": 2, "text": "...", "ms": 85, "words": [...]}
      any failure -> {"id": N, "error": "..."}. EOF on stdin ends the process.

  server.py download <model_dir> <repo>
      Fetches <repo> into <model_dir>, printing {"done_bytes": n, "total_bytes": m} lines,
      verifies model.safetensors against the hub's LFS sha256, writes <model_dir>/.complete,
      and ends with {"complete": true} (or {"error": "..."} and exit 1).
"""

import base64
import hashlib
import json
import os
import sys
import threading
import time
from pathlib import Path


def emit(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()


def log(msg):
    sys.stderr.write(f"parakeet-mlx: {msg}\n")
    sys.stderr.flush()


def serve(model_dir):
    import mlx.core as mx
    import numpy as np
    from parakeet_mlx import from_pretrained
    from parakeet_mlx.audio import get_logmel

    t = time.monotonic()
    model = from_pretrained(model_dir)
    cfg = model.preprocessor_config
    log(f"loaded {model_dir} in {time.monotonic() - t:.1f}s")

    def run(pcm, words):
        # float32 in, as `parakeet_mlx.audio.load_audio` returns: get_logmel views the
        # complex STFT as the input dtype, so bfloat16 here breaks its shapes.
        audio = mx.array(pcm, dtype=mx.float32)
        if audio.shape[0] < cfg.hop_length:
            return "", []
        result = model.generate(get_logmel(audio, cfg))[0]
        out = []
        if words:
            out = [
                {"text": tok.text, "start": tok.start, "end": tok.end}
                for tok in result.tokens
            ]
        return result.text.strip(), out

    # One short clip so the first real request does not pay for kernel compilation.
    run(np.zeros(cfg.sample_rate // 2, dtype=np.float32), False)
    emit({"ready": True, "model": str(model_dir), "backend": f"mlx {mx.default_device()}"})

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        rid = None
        try:
            req = json.loads(line)
            rid = req.get("id")
            t = time.monotonic()
            op = req.get("op")
            if op == "warm":
                run(np.zeros(cfg.sample_rate // 2, dtype=np.float32), False)
                text, words = "", []
            elif op == "transcribe":
                rate = int(req.get("sample_rate", cfg.sample_rate))
                if rate != cfg.sample_rate:
                    raise ValueError(f"sample_rate {rate}, model wants {cfg.sample_rate}")
                raw = base64.b64decode(req["pcm16_b64"])
                pcm = np.frombuffer(raw, dtype="<i2").astype(np.float32) / 32768.0
                text, words = run(pcm, bool(req.get("words")))
            else:
                raise ValueError(f"unknown op {op!r}")
            reply = {"id": rid, "text": text, "ms": int((time.monotonic() - t) * 1000)}
            if words:
                reply["words"] = words
            emit(reply)
        except Exception as e:  # one bad request must not take the server down
            emit({"id": rid, "error": f"{type(e).__name__}: {e}"})


def dir_bytes(path):
    total = 0
    for root, _, files in os.walk(path):
        for f in files:
            try:
                total += os.path.getsize(os.path.join(root, f))
            except OSError:
                pass
    return total


def download(model_dir, repo):
    from huggingface_hub import HfApi, snapshot_download

    dest = Path(model_dir)
    dest.mkdir(parents=True, exist_ok=True)
    (dest / ".complete").unlink(missing_ok=True)
    info = HfApi().model_info(repo, files_metadata=True)
    files = {s.rfilename: s for s in info.siblings}
    total = sum(s.size or 0 for s in files.values())
    stop = threading.Event()

    def report():
        while not stop.wait(1.0):
            emit({"done_bytes": min(dir_bytes(dest), total), "total_bytes": total})

    reporter = threading.Thread(target=report, daemon=True)
    reporter.start()
    try:
        snapshot_download(repo, local_dir=str(dest))
    finally:
        stop.set()
        reporter.join()
    weights = files.get("model.safetensors")
    want = weights.lfs.sha256 if weights is not None and weights.lfs else None
    if want:
        h = hashlib.sha256()
        with open(dest / "model.safetensors", "rb") as f:
            for block in iter(lambda: f.read(1 << 22), b""):
                h.update(block)
        if h.hexdigest() != want:
            raise RuntimeError(f"model.safetensors sha256 {h.hexdigest()} != hub {want}")
    (dest / ".complete").write_text(json.dumps({"repo": repo, "sha": info.sha, "sha256": want}))
    emit({"done_bytes": total, "total_bytes": total})
    emit({"complete": True})


def main():
    mode = sys.argv[1] if len(sys.argv) > 1 else ""
    try:
        if mode == "serve":
            serve(sys.argv[2])
        elif mode == "download":
            download(sys.argv[2], sys.argv[3])
        else:
            log(f"usage: {sys.argv[0]} serve <model_dir> | download <model_dir> <repo>")
            sys.exit(2)
    except Exception as e:
        emit({"error": f"{type(e).__name__}: {e}"})
        sys.exit(1)


if __name__ == "__main__":
    main()
