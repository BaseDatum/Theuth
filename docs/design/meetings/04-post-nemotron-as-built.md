# Meetings phase 4 — diarization as built: Nemotron 3 Diarization

Status: as-built · 2026-09-24 · replaces the diarization stage of `04-post.md`
(§0 decision 1's file list, §1's `diarize.rs` line, §2 stage 2, §10 deviations
1 and 3). Everything else in 04 — alignment, profiles, review, summary —
stands; the stage still returns `diarize::Diarized { spans, embeddings }` and
`post/mod.rs` consumes it unchanged.

## 0. What changed and why

The pyannote `segmentation-3.0` + wespeaker-per-span + agglomerative-cosine
path over-split real meetings: on the two recordings under `~/.theuth` it
produced 21 and 25 "speakers" (the stored transcripts, from the pipeline's own
decode, show 18 and 20) for meetings of roughly four and eight people, and left
20 % of the system words outside any span. NVIDIA's **Nemotron 3 Diarization**
(released 2026-09-23; a Streaming Sortformer, 100 M parameters, `openmdw-1.1`,
commercial use allowed) is an end-to-end model: one pass over the recording
gives every 10 ms frame a probability per speaker, up to eight, with the
channels ordered by each speaker's first arrival, so there is nothing to
cluster. Its arrival-order speaker cache carries identities across chunks, so
a 1 h meeting is one continuous session, not a stack of independent windows.

wespeaker stays — one embedding per *speaker* now (from up to 30 s of their
turns) instead of one per span — because the profile match (`Profiles::
best_match`) and `Enroll` (`transcript::apply`) need a voiceprint per label.

## 1. Files

```
<data_dir>/models/nemotron-diarization/model_quantized.onnx        graph (356 KB)
<data_dir>/models/nemotron-diarization/model_quantized.onnx_data   weights (115 MB, ONNX external data — must sit beside the graph)
<data_dir>/models/pyannote/wespeaker_en_voxceleb_CAM++.onnx         unchanged
```

Origin: `hf:onnx-community/Nemotron-3-Diarization-ONNX/onnx/<file>`
(`scripts/fetch-models.sh` REQUIRED + `origin_of`; the pyannote
`segmentation-3.0.onnx` entry is gone). No config file is read at run time:
every value below is a named `const` in `crates/meeting/src/post/diarize.rs`.

`meeting.models_dir` now names the models **root** (default
`<data_dir>/models`), under which `diarize::models` looks for
`nemotron-diarization/` and `pyannote/`; `post::Paths::models` is that root.
Missing either file → the pipeline runs without diarization exactly as before
(04 §0.1), the note naming both paths.

## 2. Graph I/O (`onnx/model_quantized.onnx`, opset 21 + `com.microsoft`)

Inspected with `onnx` (Python) and confirmed by running ORT 1.30 with shapes;
the model card's "How to use" is empty.

| Tensor | Shape | dtype | Meaning |
|---|---|---|---|
| in `input_features` | `[1, M, 128]` | f32 | log-mel frames of the chunk **and** its look-ahead (10 ms each); `M` need not be a multiple of 8 — the graph zero-pads the last stack |
| in `cached_embeds` | `[1, C, 512]` | f32 | the speaker cache followed by the FIFO, pre-encoder embeddings; `C = 0` on the first chunk (an empty tensor is accepted) |
| in `attention_mask` | `[1, C + ⌈M/8⌉]` | i64 | one per **encoder** frame (cached + fresh), all ones for a batch of one |
| out `logits` | `[1, (C + ⌈M/8⌉)·8, 8]` | f32 | speaker logits at 10 ms for every encoder frame of the step, cached ones first; `sigmoid` → probability |
| out `chunk_embeds` | `[1, ⌈M/8⌉, 512]` | f32 | the fresh frames' pre-encoder embeddings (feature stacking ×8 + linear), what the FIFO/cache store |
| out `silence_embeds` | `[512]` | f32 | the learned silence embedding, one per speaker slot of a compressed cache |

So the graph is the whole encoder step of HF's
`Nemotron3DiarizationForAudioFrameClassification.forward` (transformers 5.18
dev, `models/nemotron3_diarization`): feature stacking → 31-layer RoPE
transformer over `concat(cached, fresh)` with positions restarting at 0 each
step → 192-d projection → sub-pixel ×8 upsampler → classifier. The speaker
cache and FIFO are **not** graph state; the caller keeps them (§4). The three
variants (`model`, `model_quantized`, `model_q4`) share this signature. The
quantized ones use `MatMulNBits` (8-bit and 4-bit block weights), not QDQ int8.

## 3. Feature extraction (`Mel`)

HF `NemotronAsrStreamingFeatureExtractor` with `preprocessor_config.json`:

- input f32 PCM in [−1, 1] at 16 kHz (the pipeline's `system_pcm` as is; the
  old stage took `i16` — the log-mel is unnormalised, so scale matters);
- pre-emphasis `y[n] = x[n] − 0.97·x[n−1]`, `y[0] = x[0]`;
- STFT: `n_fft` 512, hop 160, a **symmetric** 400-sample Hann window
  (`torch.hann_window(periodic=False)`) zero-padded to 512 *centred* (56 each
  side), `center=True` with constant (zero) padding of 256 samples at both
  ends — frame `t` is centred on sample `160·t`; power spectrum `|X|²`;
- 128 mel filters, librosa `filters.mel(sr=16000, n_fft=512, n_mels=128,
  fmin=0, fmax=8000, norm="slaney")`: Slaney mel scale (linear below 1 kHz,
  log above), triangles scaled to unit area;
- `ln(mel + 2⁻²⁴)`; no mean/variance normalisation;
- a recording has `⌊len/160⌋` frames (HF returns one more, masked).

`Mel::frames(pcm, t0, t1, out)` computes any frame range straight from the
PCM (pre-emphasis applied on the fly, no whole-recording feature buffer), so a
1 h meeting never holds 360 000 × 128 floats. `realfft` 3.5 (already in the
graph under `parakeet-rs`) does the FFT.

**Validation:** `tests::mel_matches_the_reference_extractor` feeds 0.5 s of a
deterministic signal (two tones, a chirp, a DC step) and checks 14 scattered
(frame, bin) values and the sum against the HF extractor's output (generated
with transformers 5.18 dev + torch 2.14 + librosa 1.0), tolerance 2·10⁻³ in
log-mel units. End to end, the fp32 graph driven by this front end reproduces
the HF torch model's probabilities on both real recordings to `max |Δp| <
0.0005` over every frame (§6), which also covers the streaming loop.

## 4. Streaming loop (`probabilities`, `SpeakerCache`)

The offline configuration of `config.json` (`chunk_length` 340,
`chunk_right_context` 40, `fifo_length` 40, `speaker_cache_update_period`
300; the policy from `streaming_config`: `speaker_cache_length` 264,
`speaker_cache_silence_frames_per_speaker` 1, `prediction_score_threshold`
0.25, `latest_frames_score_boost` 0.05, `min_positive_scores_rate` 0.5,
`strong_boost_rate` 0.75, `weak_boost_rate` 1.5). Units are encoder frames
(80 ms = 8 mel frames). A 27.2 s chunk with 3.2 s look-ahead is what the card
calls the "30.4 s input buffer" — its best-scoring setting.

Per chunk `k` (`start = 340k`, `end = min(start + 340, E)`, `stop = min(end +
40, E)`, `E = ⌈frames/8⌉`):

1. `input_features` = mel frames `[8·start, min(8·stop, frames))`;
   `cached_embeds` = cache ‖ FIFO (`C` frames); `attention_mask` = ones.
2. Run. The chunk's own probabilities are `sigmoid(logits)` rows
   `[8C, 8(C + end − start))` — the look-ahead rows are discarded and scored
   again as the head of the next chunk.
3. Pool `sigmoid(logits)` to encoder frames (mean of 8) → `probs[C + stop −
   start, 8]`, then `SpeakerCache::update(probs, chunk_embeds, n_chunk =
   end − start, silence_embeds)`, a port of HF's
   `Nemotron3DiarizationSpeakerCache.update` / `_compress`:
   - FIFO ← FIFO ‖ `chunk_embeds[..n_chunk]` (look-ahead frames do not join);
   - if the FIFO exceeds 40 frames, `popped = min(max(300, len − 40), len)` of
     its oldest move to the cache with their pooled probabilities (an
     uncompressed cache's stored probabilities are replaced by this step's
     re-estimate; a compressed cache's are kept — it is out of time order);
   - a cache over 264 frames is **compressed**: each (frame, speaker) is
     scored `ln max(p, .25) − ln max(1−p, .25) + Σₛ ln max(1−pₛ, .25) + ln 2`
     (how clearly this speaker *alone* is active), `−∞` unless `p > 0.5`, and
     `−∞` for non-positive scores once a speaker has ≥ 16 positive frames
     (`⌊32·0.5⌋`, budget 32 = 264/8 − 1); frames just popped from the FIFO get
     +0.05; each speaker's top 24 frames get +2 ln 2 and top 48 get +ln 2; one
     `+∞` silence slot per speaker; the top 264 (speaker, frame) pairs are
     kept, sorted speaker-major then by time, `−∞` picks and the silence slots
     taking `silence_embeds` with zero probability.
   In steady state every step attends to 264 + 40 cached frames plus 380
   fresh ones (684 encoder frames), whatever the meeting length: memory and
   per-chunk time are constant, and a speaker first heard in minute 2 keeps
   their channel in minute 50.
4. `progress(k + 1, chunks)`.

The result is `⌊len/160⌋ × 8` probabilities at 10 ms, ~11.5 MB per hour.

Tests: `fifo_overflow_moves_frames_to_the_cache_and_compresses_it` (sizes and
the compressed flag over three chunks including a short tail),
`compress_keeps_cache_size_groups_by_speaker_and_seats_silence` (two clean
speakers plus an overlap frame: 264 rows, speaker-major time order, eight
silence slots with zero probability, the overlap frame loses).

## 5. Frames → turns (`turns`) and embeddings (`run`)

Per channel: active where `p ≥ SPEECH` (**0.5**, the reference processor's
default); a gap shorter than **`MERGE_GAP_MS` = 500** inside one channel's
activity is bridged; runs shorter than **`MIN_TURN_MS` = 200** are dropped.
Channels are relabelled `s1..sn` in order of their *first turn*, so labels
are dense even when the model skipped a channel (channel 5 firing without
channel 4 does not produce `s6` without `s5`). Overlapping speech yields
overlapping spans; `align::speaker_of` takes the first span containing the
word's midpoint, as before.

Embeddings: for each label, its spans' PCM is concatenated in time order up
to 30 s (`EMBED_SAMPLES`, as `ME_EMBED_SAMPLES`) and run once through
`Embedder::compute` (unchanged: kaldi fbank via `knf-rs`, tiled to a multiple
of 200 frames — 04 §10 deviation 2). A label with under 1 s of speech gets no
embedding: no profile proposal, and `Enroll` on it reports "no voice
embedding" as it already did for a missing file.

Progress: chunks 0–80 % of the stage, embeddings 80–100 %.

Smoke (`diarize::run`, `model_quantized`, both recordings): every label got
an embedding (512-d), including the 3 s speaker; the whole stage took 9.5 s /
19.1 s (the wespeaker passes add under a second per speaker); every pairwise
cosine between the speakers of one meeting was below `speakers::MATCH` (0.6)
— highest 0.55 (standup `s1`/`s4`) and 0.51 (migration `s4`/`s5`), the rest
−0.07–0.44 — so no two labels would be proposed as the same enrolled person.

## 6. Variant choice and bench

M3 Max, ORT 1.27.0 (the shipped runtime, `models::onnx::ort_lib`), CPU EP
(`auto` picks WebGPU only for a discrete GPU), `--release`, the two meetings
under `~/.theuth` (system tracks decoded by `transcribe::pcm16k`), Rust
`diarize::probabilities` + `turns` end to end, HF torch (fp32) as the
reference (`transformers` 5.18 dev on the same WAVs):

| Variant | Weights | 11.75 min standup: wall, s/min, spk/turns | agreement vs torch fp32 | 22.5 min migration: wall, s/min, spk/turns | agreement |
|---|---|---|---|---|---|
| `model` (fp32) | 385 MB | 7.8 s · 0.66 · 4 / 138 | max Δp 0.000, active set 100 % | 15.8 s · 0.70 · 8 / 213 | max Δp 0.000, 100 % |
| **`model_quantized` (8-bit MatMulNBits)** | **115 MB** | 8.6 s · 0.73 · 4 / 138 | mean Δp 0.0006, max 0.28, 99.82 % | 18.1 s · 0.81 · 8 / 221 | mean Δp 0.0056, max 0.79, 98.10 % |
| `model_q4` (4-bit) | 79 MB | 9.1 s · 0.77 · 4 / 134 | mean 0.0011, max 0.43, 99.72 % | 17.0 s · 0.76 · 8 / 221 | mean 0.0051, max 0.57, 98.55 % |

"Agreement" = share of 10 ms frames whose set of active channels (`p ≥ 0.5`)
equals the fp32 reference's; per-channel active-frame counts differ by < 1 %
for the 8-bit variant on every channel with more than a few seconds of speech.
`model_fp16` and `model_q4f16` were not tried: the CPU EP would upcast.

**Shipped: `model_quantized`.** Same speaker counts, 98–99.8 % frame
agreement, coherent per-speaker text on both recordings, at 30 % of the fp32
download. It is *not* faster than fp32 on this CPU — `MatMulNBits` dequantises
on the fly and fp32 GEMM is what Apple's BLAS is tuned for — so the choice is
purely distribution size; a machine short on RAM also benefits. Revisit if a
GPU EP lands: fp32 or fp16 may then win outright. q4 buys another 36 MB for a
further, small, accuracy loss and was slower still; not worth it.

Old vs new on the same audio (`--release`, same machine):

| Recording | pyannote + wespeaker + clustering (main) | Nemotron `model_quantized` |
|---|---|---|
| standup, 705 s, ~4 people | 3.4 s wall (0.29 s/min) · **21 speakers** · 188 spans · 364 / 1844 system words outside every span | 8.6 s wall (0.73 s/min) · **4 speakers** · 138 turns · 110 / 1844 outside |
| migration, 1348 s, ~8 people | 6.6 s wall (0.30 s/min) · **25 speakers** · 311 spans · 721 / 3458 outside | 18.1 s wall (0.81 s/min) · **8 speakers** · 221 turns · 231 / 3458 outside |

Speech per new speaker: standup `s1` 288 s, `s2` 3 s, `s3` 58 s, `s4` 123 s;
migration 320 / 40 / 241 / 110 / 78 / 30 / 14 / 113 s. Reading the stored
transcript's words under the new labels, each label is one voice ("I'm
Steffi…" is one label throughout). pyannote's extra "speakers" were 1–6 s
fragments. The new stage is 2.5× slower but still under 1 s per minute of
audio; the transcription stage before it (Parakeet TDT on both tracks) is
what dominates the pipeline.

## 7. Removed

- `diarize::{SEGMENTATION, CLUSTER, MIN_EMBED_MS, WINDOW, FRAME, FRAME_START,
  speech, cluster}` and their test; `Models::segmentation`.
- `pyannote/segmentation-3.0.onnx` from `fetch-models.sh` REQUIRED (the
  bucket copy is now unreferenced; `release-models.sh` publishes the list, so
  the next release drops it and adds the two Nemotron files — not run here).
- `diarize::run` takes `&[f32]`; `to_i16` stays for wespeaker
  (`me_embedding`, per-speaker embeddings).

## 8. Not verified

- The CoreML / DirectML / WebGPU EPs on this graph (the session goes through
  `models::onnx::configure` like every other, so the knob applies; whether
  `MatMulNBits` and `RotaryEmbedding` (`com.microsoft`) are taken by those EPs
  or fall back per node is untested).
- A meeting over one hour end to end (the loop's state is constant-size by
  construction; the longest real recording here is 22.5 min = 50 chunks with
  the cache compressed from chunk 1 on).
- More than eight simultaneous speakers: the ninth voice is absorbed into an
  existing channel by the model.
- `release-models.sh` / the bucket manifest: not run, per instructions.
