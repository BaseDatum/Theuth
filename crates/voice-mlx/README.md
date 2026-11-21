# voice-mlx — Parakeet TDT 0.6B v3 on MLX, in-process

## Route decision (2026-11-21): port onto `mlx-rs`, no existing crate to wrap

Searched for an existing Rust implementation of Parakeet TDT on MLX:

- crates.io `parakeet`: `parakeet-rs` 0.4.4 (ONNX Runtime, already used by `crates/voice`), `parakeet-rs-featsize`,
  `fusor-ai-parakeet` (own tensor runtime), `rightkit-asr`, `talk-rs` — none runs on MLX or loads the
  mlx-community safetensors.
- crates.io `mlx-rs` / `mlx-sys`: only the bindings (`oxiglade/mlx-rs`, MIT OR Apache-2.0, mlx-rs 0.34.0 /
  mlx-sys 0.7.0 published 2026-11, ~300k downloads) plus forks (`pmetal-mlx-rs`, `quill-mlx`, `mlx-rs-burn`).
  No ASR model crates on top of them.
- GitHub `parakeet mlx rust`, `parakeet-mlx-rs`, `parakeet tdt mlx-rs`: one hit, `Alexander-Ollman/voiceflow`
  (Swift), nothing in Rust.

So this crate ports the inference path of the Python package `parakeet-mlx` 0.7.0 (MIT; source read from the PyPI
wheel) onto `mlx-rs` 0.34 (MLX v0.32.2 via mlx-c). Toolchain present here: `cmake` 4.5.0, Metal toolchain
(`xcrun --find metal` → Metal.xctoolchain, `metal` 32023.830).

## What is ported (file → reference)

| here | parakeet-mlx 0.7.0 |
|---|---|
| `src/mel.rs` | `audio.py` `get_logmel` + librosa slaney mel filterbank |
| `src/model.rs` `encode` | `conformer.py` (dw_striding subsampling ×8, 24 FastConformer blocks), `attention.py` rel-pos MHA |
| `src/model.rs` `predict`, `decode_greedy` | `rnnt.py` (2-layer LSTM, joint), `parakeet.py` `ParakeetTDT.decode_greedy` |
| `src/text.rs` | `tokenizer.py` `decode` + `alignment.py` text assembly |

Faithfulness notes:

- Single pass over the whole clip (parakeet-mlx's default `chunk_duration=None`); no chunking.
- Weights stay float32, as shipped. parakeet-mlx's `from_pretrained` casts them to bfloat16 by default; on the three
  real clips tried both dtypes gave the identical transcript in Python.
- Vocabulary = `joint.vocabulary` in config.json (exactly what parakeet-mlx uses); the sentencepiece files are not
  read, so `FILES` is `config.json` + `model.safetensors`.
- The front end keeps parakeet-mlx's quirks (window padded at the end, `|re|+|im|` magnitude) — see `src/mel.rs`.
- Greedy rule: argmax over vocabulary+blank, argmax over the 5 durations, advance by the duration, at most
  `decoding.greedy.max_symbols` (10) zero-duration emissions per frame. The prediction-net output is cached between
  emissions (Python recomputes it with identical inputs).
- Words: a new word starts at every token whose text begins with a space; times = frame × 0.08 s.

## Packaging facts

- **Metal library at run time: yes.** MLX loads `mlx.metallib` (~136 MB for the full kernel set at this pin)
  when Metal initialises. mlx-sys's build script compiles it into `~/.mlx/lib/<mlx-c-key>/mlx.metallib` (or
  `$MLX_RS_METAL_PATH` if set at build time) and bakes that absolute path into the binary as CMake
  `MLX_METAL_PATH`. MLX also looks for a colocated `mlx.metallib` next to the executable (and in the
  `Resources/` dir of a bundle) before falling back to that compiled-in path. A shipped binary therefore needs the
  metallib beside it (or in the app bundle's Resources); the build-machine path will not exist on users' machines.
  `mlx_rs::metal::set_metallib_path` overrides it at run time. The `metal-jit` feature shrinks the metallib to
  ~1.5 MB but compiles kernels from source on first use in every process.
- **Linked:** static `libmlx` + `libmlxc` (+ gguflib), `libc++`, `objc`; frameworks `Foundation`, `Metal`,
  `Accelerate` (mlx-sys build.rs), plus `clang_rt.osx` from Xcode when found.
- **Minimum macOS:** MLX v0.32.2's CMake builds for the *build host's* macOS version unless
  `CMAKE_OSX_DEPLOYMENT_TARGET` is set, and refuses anything below 14.0. For a distributable binary set
  `MACOSX_DEPLOYMENT_TARGET=14.0` (or the app's floor, ≥ 14.0) for the build; not yet verified that mlx-sys's cmake
  invocation forwards it (check `otool -l` LC_BUILD_VERSION `minos` on objects in `libmlx.a`).
- **Build:** mlx-sys runs CMake, which fetches MLX v0.32.2 sources from GitHub (network at build time) and builds
  them; needs cmake and the Metal toolchain.

## Measured here

Python oracle (parakeet-mlx 0.7.0, `uv run --with parakeet-mlx`, run once in /tmp, not part of the build):
load 4.8 s cold (f32) / 0.14 s warm-cache (bf16); three private dictation clips of 6.9 s, 17.8 s and 36.8 s took
576 / 205 / 291 ms (f32, first includes kernel warm-up); f32 and bf16 transcripts identical. The synthetic-tone mel
values used by `mel::tests` come from the same run.

Rust port status (2026-11-21, first session): `cargo check -p voice-mlx` passes (cold build incl. mlx-sys's CMake
build of MLX: 2 min 10 s of cargo time at `CARGO_BUILD_JOBS=4`/`CMAKE_BUILD_PARALLEL_LEVEL=4`, after ~13 min
waiting for the build lock). Unit tests and the real-clip comparison had not reported back when the session ended;
binary size delta, Rust load/per-clip time and peak RSS are still open.

## Next steps for whoever continues

1. `cargo check -p voice-mlx` (under the build lock, `CMAKE_BUILD_PARALLEL_LEVEL=4`); fix API slips against
   mlx-rs 0.34 (written against its source, never compiled).
2. `cargo test -p voice-mlx` — `text::tests` (tokenizer joining oracle) and `mel::tests` (log-mel oracle, needs
   Metal).
3. Weights: `curl -L https://huggingface.co/mlx-community/parakeet-tdt-0.6b-v3/resolve/main/{config.json,model.safetensors}`
   into /tmp/parakeet-tdt-mlx, then `PARAKEET_MLX_DIR=... PARAKEET_MLX_CLIP=a.pcm16:b.pcm16 cargo test -p voice-mlx
   --test real_clip -- --ignored --nocapture`; compare with
   `uv run --with parakeet-mlx==0.7.0 python -c 'from parakeet_mlx import from_pretrained; from parakeet_mlx.audio import get_logmel; ...'`
   (`model.generate(get_logmel(pcm, model.preprocessor_config))[0].text`).
4. If transcripts diverge, compare encoder output mean/std first (oracle for the 6.9 s clip: shape [1, 87, 1024],
   mean −1.63e-5, std 0.018683), then the rel_shift / pos-emb path in `model.rs`.
