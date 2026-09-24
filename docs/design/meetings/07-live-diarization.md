# Meetings phase 7 — live speaker labels on the `them` side

Status: design + bench · 2026-09-24 · scopes what 53 decision 6 rejected
("live diarization: inaccurate and expensive") now that the offline stage runs
Nemotron 3 Diarization (`04-post-nemotron-as-built.md`), a streaming model
whose offline loop is already the streaming loop with big chunks. Owner's
steer: **optimise for accuracy; one switch in Meeting settings turns it off.**
No production code in this branch; the bench is
`crates/meeting/examples/live_diar_bench.rs`.

## 0. Decisions

1. **Feasible; do it.** On this M3 Max the accuracy-first geometry below runs
   at RTF 0.07 (half a core) beside the live STT (a third of a core), agrees
   with the offline pass on 97–99 % of frames, and finds the same speaker
   count on both real recordings (4 and 8). Per-step compute is ~constant
   (170–290 ms at 8 threads whatever the chunk), so **cost ∝ 1/chunk** and
   accuracy grows slowly with chunk + look-ahead: the trade is latency for CPU.
2. **Geometry: chunk 36 · right context 12 · FIFO 264 · update 222** (all 80 ms
   encoder frames; the model card's `low_latency` cache policy with a 4× chunk
   and 3× look-ahead). Input buffer 3.84 s, labels ≈ 4.1 s behind real time.
   Named `acc4` in the bench. §1 has the numbers; §8 Q1 has the alternative.
3. **Labels are late, never wrong-then-fixed for the model.** A closed system
   utterance is *held* until the diariser has scored its last frame (≤ ~4 s),
   then logged, batched and published with its speaker. The island shows the
   text at once as `them:` (partial and final) and re-paints the row when the
   label lands. Stop flushes the diariser (last chunk, no look-ahead) so every
   utterance is labelled before the last batch.
4. **Vocabulary `Speaker 1..8`**, numbered by first live turn, stable for the
   meeting (the model's arrival-order cache keeps channel identities; the
   labeller keeps `channel → label`). Ids `s1..s8` on the wire, as the post
   pass. Model lines read `Speaker 2: …`; `them:` only for an unlabelled line.
5. **Offline stays authoritative; it renumbers itself to agree with live.**
   `transcript.json` comes from `diarize::run` as today; a small step maps
   offline `sN` → live label by time overlap and renames so "Speaker 2" is the
   same voice in the session's memory and in the reviewed transcript. Never
   map by channel index: the bench shows a FIFO change swapping channels 1↔2
   (§1, `mid_fifo40`).
6. **Switch `meeting.live_diarization: true`** (config + Settings › Meetings).
   Off, or models missing, or the task dies → exactly today: `them:`, the
   post pass diarises. Read at meeting start; a running meeting keeps its mode.
7. **Not in v1:** live voiceprint names (§7 Q5 — the pieces exist, one later
   step), splitting a VAD utterance that contains two speakers (needs word
   times the streaming STT does not give; the post pass fixes it), mic
   diarisation (decision 6: mic is `me`).

## 1. Bench

`cargo run --release -p meeting --example live_diar_bench -- <system.opus>
--words transcript.json --configs …` — the production `Mel`, `turns`, model
path and `ort` setup, the streaming loop and `SpeakerCache` copied with FIFO /
update period as parameters. M3 Max (16 cores, ORT default 8 intra-op
threads), CPU EP, `model_quantized` (8-bit `MatMulNBits`), the two real
meetings under `~/.theuth` (system tracks decoded by `transcribe::pcm16k`).
Reference = the offline geometry on the same audio (which itself is 98–99.8 %
in agreement with HF fp32, 04 §6). "frames=" = share of 10 ms frames with the
same active-speaker set after matching channels by overlap; DER collar-less
over offline speech; "words=" = stored system-track words (TDT word times)
given the same speaker at their midpoint. `cpu%` = process CPU seconds per
audio second, i.e. share of one core while live.

| config | chunk/right/fifo/update | buffer | standup 705 s, 4 spk: RTF · cpu% · step p50/p95 ms · frames= · DER · words= | migration 1348 s, 8 spk: same |
|---|---|---|---|---|
| offline | 340/40/40/300 | 30.4 s | 0.011 · 8 % · 311/316 · ref | 0.011 · 8 % · 311/324 · ref |
| ultra_low | 3/1/264/222 | 0.32 s | 0.749 · 598 % · 180/234 · 98.3 % · 2.6 % · 99.2 % | — |
| very_low | 6/2/264/222 | 0.64 s | 0.384 · 307 % · 184/240 · 98.8 % · 1.8 % · 99.3 % | — |
| low (card default) | 9/4/264/222 | 1.04 s | 0.241 · 193 % · 173/225 · 98.5 % · 2.3 % · 98.9 % | 0.249 · 199 % · 178/230 · 96.3 % · 5.5 % · 96.5 % |
| low_fifo124 | 9/4/124/100 | 1.04 s | 0.194 · 155 % · 141/160 · 98.5 % · 2.3 % · 99.0 % | — |
| low_fifo40 | 9/4/40/300 | 1.04 s | 0.167 · 133 % · 121/129 · 98.6 % · 2.2 % · 99.0 % | — |
| mid | 18/4/264/222 | 1.76 s | 0.113 · 90 % · 162/211 · 98.8 % · 1.9 % · 99.1 % | 0.145 · 116 % · 207/267 · 96.6 % · 5.2 % · 96.7 % |
| mid_fifo40 | 18/4/40/300 | 1.76 s | — | 0.089 · 71 % · 127/137 · 95.9 % · 6.2 % · 96.0 % · **channels 1↔2, 5↔6 swapped** |
| acc2 | 18/8/264/222 | 2.08 s | 0.120 · 96 % · 172/226 · 98.7 % · 2.0 % · 99.3 % | 0.135 · 107 % · 192/252 · 96.6 % · 5.1 % · 96.3 % |
| **acc4** | **36/12/264/222** | **3.84 s** | **0.066 · 53 % · 190/241 · 98.9 % · 1.7 % · 99.2 %** | **0.070 · 56 % · 198/250 · 97.0 % · 4.5 % · 97.0 %** |
| acc8 | 80/20/264/222 | 8.0 s | 0.035 · 27 % · 225/273 · 99.1 % · 1.4 % · 99.3 % | 0.036 · 28 % · 234/282 · 97.4 % · 4.0 % · 97.4 % |

Every configuration found 4 and 8 speakers; per-channel speech seconds match
offline within a few seconds per channel (the 8-speaker `s7`, 28 s offline,
is the one that moves: 16–23 s live). Words outside every span: offline
8.1 % / 10.0 %, live 8–12 % (`unassigned` column of the bench). Peak RSS of
the bench process 480–630 MB (weights 115 MB + arena + the whole track in
memory); the diariser session itself is ~300 MB resident.

**Threads** (standup, first 240 s, `low` / `mid`): 8 threads RTF 0.24 / 0.11;
4: 0.38 / 0.19 (cpu 150 % / 77 %); 2: 0.71 / 0.37; **1: 1.39 / 0.71** — the
card's `low` does not keep real time on one core; `acc4` on one core ≈ 0.35
[INFERENCE from the 4× fewer steps; not run]. CPU seconds are nearly
thread-independent, so the cap only trades wall latency per step.

**Beside the live STT** (standup, first 300 s; `voice::live::Utterer` with the
Nemotron streaming int8 model, 20 ms pushes as the ring delivers):

| | STT RTF | diariser RTF · step p50/p95 | pair cpu% |
|---|---|---|---|
| STT alone | 0.066 (31 % of a core) | — | 31 % |
| STT ∥ `low`, diariser 8 threads | 0.120 | 0.339 · 231/395 ms | 325 % |
| STT ∥ `low`, diariser 4 threads | 0.083 | 0.415 · 298/421 ms | 206 % |

Both stay real time even in the worst case measured; the pair with `acc4`
should sit near 0.9 of one core [INFERENCE: 31 % + 56 %, not run together].
Peak RSS 1.96 GB = one STT session (~0.9 GB) + diariser + audio; a meeting has
two STT sessions, so ~2.1 GB live instead of 1.8 GB.

**Windows/Linux, CPU only, no AEC** (58 §6.1): a 4-core laptop runs STT ~0.3
core + `acc4` ~0.6 core at a 4-thread cap — plausible; a 2-core machine is
tight, and the switch is the escape hatch. `MatMulNBits` dequantises on the
fly and fp32 is not faster on Apple BLAS (04 §6); on x86 the int8 path may be
relatively better. Not measured there.

## 2. Where it runs

```
capture::system thread ──► Feed::push ──► enc ring ──► encoder (Opus)
                                      ├──► stt ring ──► stt::run (Utterer)        [exists]
                                      └──► diar ring ──► diar::run (Diarizer)     [new]
                                                             │ tell Labels { at, spans }
                                                             ▼
                                                        Meeting actor ── hold-back ── live.jsonl · Batcher · MeetingEvent::Utterance{speaker}
```

- `capture/mod.rs`: a third `rtrb` producer on the **system** `Feed` only
  (`Feed::push` clones one more 20 ms chunk; `take_diar()`), same 5 s ring.
  Rejected: a tee inside `Utterer` (couples the diariser to the STT task's
  pacing and failure) and a shared 16 kHz buffer (two owners).
- `meeting/src/diar.rs` (new, the shape of `stt.rs`): one `spawn_blocking`
  task for the meeting owning `Diarizer { session, Mel, SpeakerCache,
  rubato 48→16 k, buf: Vec<f32> (16 kHz, the unscored tail), labeller }`.
  Loop: pop `(at, pcm48)`, resample, append; once `buf` holds `(36 + 12) × 8
  × 160` samples past the scored position, run one step (`input_features` =
  48 encoder frames of mel, `cached_embeds` = cache ‖ FIFO, exactly 04 §4
  with the new sizes), keep the 36 chunk frames' probabilities, feed the
  labeller, `tell` the actor. Same `stop`/`paused` flags as `stt::Flags`:
  pause → nothing arrives (the `Feed` does not push); resume → discard the
  stale ring, **keep** the `SpeakerCache` (speakers survive a pause; the
  splice is the same one the Opus file has). Stop → score the remaining tail
  as a last chunk with no look-ahead, `tell` the final `Labels`, return.
- Session: `diarize::session` as today (`ort_lib` first, `configure` for the
  EP) plus `with_intra_threads(4)` — a const; the post pass keeps ORT's
  default. Loaded in `start_task` beside the two `Utterer`s only when
  `cfg.live_diarization && diarize::models(..).is_some()`.
- Clock: as `stt::Piece`, the message carries the newest ring `Instant` and
  spans as durations *behind* it (`(at − 16 kHz samples / 16 000)`), and the
  actor converts with `RecordingClock::elapsed_at` — the recording clock
  excludes pauses exactly as the sample stream does. A dropped ring chunk
  (diariser slower than real time) shifts earlier frames by 20 ms per drop;
  logged, not corrected.
- `diarize.rs` refactor (behaviour-neutral): `SpeakerCache` takes `fifo` and
  `update` as fields (bench code, already written); a `Streamer` that owns
  session + `Mel` + cache and exposes `step(pcm16, is_last) -> probs`;
  `probabilities()` becomes `Streamer` with the offline consts. The two
  cache tests keep passing with the offline sizes.

## 3. Aligning STT utterances to diariser frames

The streaming STT gives no word times (02 §2); an utterance is `[t0, t1]`
from the VAD (t0 includes a 320 ms pre-roll). The diariser's **horizon** is
the recording time of the last scored frame — ~3.84 s + step compute behind
real time.

Labelling rule (pure, unit-tested): over the 10 ms frames in `[t0 + 320 ms,
t1]`, count active frames (`p ≥ SPEECH` 0.5) per channel; the speaker is the
channel with the most, if it covers ≥ 30 % of the frames; else `None`
(`them:`). Overlapping speech gives the dominant voice; a two-speaker
utterance (speaker change without a 300 ms pause) is one label — known
limit, fixed by the post pass with word times.

Hold-back (actor, `Live.held: Vec<(Utterance, Instant)>`): a **System** final
whose `t1 > horizon` waits; each `Labels` message labels every held utterance
now inside the horizon, in order, and runs today's `closed()` for it
(`live.jsonl` line, alias scan, batcher feed, `MeetingEvent::Utterance {
final: true, speaker: Some }`). `MeetingEvent::Utterance` for partials and
the immediate final are published unchanged with `speaker: None` so the text
is on screen at once; the labelled re-publish has the same `track`/`t0_ms`
and the UI updates that row (§4). Mic finals and requests are never held.
Batch rule (c) (a `System` utterance ending in `?` flushes at once) fires
when that utterance is *labelled*, i.e. ≤ ~4 s later than today — inside the
25 s cadence's noise. At stop, the diariser's final `Labels` arrives before
the last batch is built (`StopMeeting` joins the task with the STT tasks);
anything still unlabelled goes out as `them:`. Diariser off/missing/dead →
`held` is bypassed entirely (today's path).

## 4. Vocabulary and where it flows

- Wire: `Utterance.speaker: Option<String>` (`#[serde(default)]`; README §3:
  add fields, never rename), same on `MeetingEvent::Utterance`. Ids `s1..s8`.
- Labeller: `channel → label` assigned on a channel's first turn ≥
  `MIN_TURN_MS` (200 ms, merged over `MERGE_GAP_MS` 500 as `turns()`), never
  reassigned; dense numbering as the post pass. Display name `Speaker N`
  (`transcript::SpeakerLabel` already renders `s2` → `Speaker 2`).
- Model batch (`session::batch_line_with`): `Speaker 2: text`; `me:`
  unchanged; `them:` when `speaker` is `None`. `tools.rs` prompt: "`me:` the
  user, `Speaker N:` other participants (`them:` when the speaker is not yet
  known)". `meeting_read` while recording accepts `s2` / `Speaker 2` as
  `speaker`, and `them` still means every system line.
- `live.jsonl`: the line gains `speaker`; readers (`meeting_read`,
  control-center replay) tolerate its absence.
- UI: `store/meeting.ts` `Line.speaker?: string`; the reducer replaces the
  open row of the track as today **and** a final row with the same
  `track`+`t0_ms` (the relabel). `IslandTranscript.who()` /
  `ControlCenter`: `speaker ? 'Speaker N:' : 'them:'`, unlabelled system rows
  keep the muted `them:`. `transcript.json` / review pane: unchanged (§5).

## 5. Post-meeting reconciliation

`diarize::run` stays the source of `transcript.json`. New pure step in
`post/align.rs`: `reconcile(live: &[Utterance], spans: &mut [Span])` — for
each offline label and each live label, the overlap in ms between offline
spans and the live utterances carrying that label; the permutation
maximising total overlap (8! exhaustive, as the bench) renames offline ids
to the live ids where the matched overlap is ≥ 50 % of that offline label's
speech; unmatched offline labels take the next free numbers. `post/mod.rs`
reads `live.jsonl` (`read_jsonl`, already there) and calls it before
`align::segments`. Effect: the session compacted from live batches and the
reviewed transcript name the same voice `Speaker 2`; the review pane's
`Speaker N` numbers are still dense. `embeddings.json` keys follow the
renamed ids. Live labels are never written into `transcript.json` themselves
— offline word-level attribution wins where they disagree (~3 % of frames).

## 6. Voiceprints live, mic, failure modes, knobs, accelerators

**Named speakers live** (later, §7 Q5): the diariser keeps the last 30 s of
16 kHz audio per label from its spans; once a label has ≥ 5 s, one
`Embedder::compute` (wespeaker, <1 s) → `Profiles::best_match` ≥ 0.6 →
`Labels.names[label] = Some("John")`; UI shows `John?:`, batches stay
`Speaker 2:` until the user confirms in review (04's rule: proposed, never
applied). Re-embed at 30 s. Feasible now — every piece exists; deferred so
v1 ships one thing.

**Mic**: not diarised, `me` (decision 6). The system loopback is the meeting
app's output and does not contain the local mic, so the diariser never sees
the user. Without headphones and AEC (`MicNoAec`) the far end leaks onto the
mic and is transcribed as `me` — existing behaviour, unchanged; a
`Speaker N` line and a `me` line with the same words a second apart is the
symptom, the post pass keeps both (04 §10 dev. 13).

**Failure modes**: models missing → `diarize::models` is `None`, no task,
today's behaviour, one `info` log. Session open fails or the task panics
(`catch_unwind` as `stt::spawn`) → `Degradation::DiarizationDown` (new
variant; the island already lists degradations), `held` drained as
`them:`, recording continues. Diariser slower than real time → ring drops
(`warn` per minute with the count); no automatic switch-off in v1. Ninth
speaker → absorbed into an existing channel by the model (04 §8); overlap →
dominant channel (§3). A meeting over an hour: state is constant-size (04
§4); the labeller holds only `channel → label` and the last 60 s of spans.

**Knobs**: `meeting.live_diarization: bool` (default `true`) — the only one.
Geometry, thread cap (4), coverage threshold (30 %), embed thresholds are
`const`s next to the offline ones in `diarize.rs`.

**Accelerators**: the session goes through `models::onnx::configure`, so
WebGPU (discrete GPU) / DirectML apply if `models.accelerator` says so;
whether `MatMulNBits` / `RotaryEmbedding` run there is untested (04 §8). A
GPU step would make the card's `low` (1 s) affordable — the geometry const is
the one line to revisit. CoreML: not tried (parakeet_rs reports it unstable
for the STT models).

## 7. Implementation plan

| # | step | files | tests | size |
|---|---|---|---|---|
| 1 | `SpeakerCache { fifo, update }` fields; `Streamer { session, mel, cache, geometry }` with `step(pcm16, is_last) -> probs`; `probabilities()` = `Streamer` with offline consts; `pub` `Geometry` consts `OFFLINE`, `LIVE` | `post/diarize.rs` | existing two cache tests (offline sizes) + `streamer_matches_probabilities` on the bench's synthetic path is model-bound → covered by the smoke in step 9 | ~150 lines, mostly moves |
| 2 | third ring on the system `Feed`, `take_diar()` | `capture/mod.rs`, `examples/capture_probe.rs` (destructure) | — | ~25 |
| 3 | `diar.rs`: `Labels`, `Flags` reuse, `run`/`spawn`, incremental labeller (`channel → label`, merge/min-turn, spans of the last 60 s) | `meeting/src/diar.rs` (new), `lib.rs` `pub mod` | labeller: stable labels across steps, dense numbering, gap merge; `speaker_of_utterance` coverage rule | ~250 |
| 4 | `Utterance.speaker`, `MeetingEvent::Utterance.speaker`, `Degradation::DiarizationDown`; `Live.held`, `Live.diar: Option<JoinHandle>`; `impl Message<Labels>` (label held, `closed()`, re-publish); `start_task` opens the session when enabled; `Begun` spawns; `StopMeeting` joins and drains; ts export | `lib.rs`, `state.rs`, `crates/rpc/src/export.rs` | hold-back ordering (pure fn over `held` + horizon) | ~200 |
| 5 | `Speaker N:` lines, prompt text, `meeting_read` speaker filter | `session.rs`, `tools.rs` | `batch_line_matches_the_design` + one labelled case | ~40 |
| 6 | config `live_diarization` (default true); RPC `meeting_live_diarization` / `set_meeting_live_diarization` (`runtime::write_config_value` + actor `SetLiveDiarization`, the `set_decision_mode` shape); Settings › Meetings row "Live speaker labels" (blurb: ~half a core, labels ~4 s late, off = labels after the meeting only); `Line.speaker`, reducer relabel, `who()` ×2 | `lib.rs`, `rpc/src/api.rs`, `desktop/ui/src/{components/settings/Meetings.tsx, store/meeting.ts, components/meeting/IslandTranscript.tsx, ControlCenter.tsx, rpc/events.ts}` | vitest: reducer relabels the matching final row; `preview()` with a speaker | ~130 Rust + TS |
| 7 | `align::reconcile` + call in `post/mod.rs` before `segments` | `post/align.rs`, `post/mod.rs` | two offline labels vs three live labels with a swap → renamed, unmatched gets next id | ~80 |
| 8 | docs: 02 §2 amendment (system finals may be held), README §3 (`speaker` field, `live_diarization` key), 53 decision 6 note, this doc's Status | docs | — | — |
| 9 | smoke: play back a recording through the live path (headless RPC as 02 §9), confirm `Speaker N:` in batches, relabel in the control center, reconciled ids in review; the `live_diar_bench` `--stt` line with `acc4` for the pair number | — | — | — |
| later | live voiceprint names (§6) | `diar.rs`, UI | — | ~80 |

Total ≈ 900 lines, 1–2 days. Steps 1–3 are independent of 4–6 and can go in
parallel; 7 needs 4's `speaker` field.

## 8. Open questions (recommended answer first)

1. **Label latency: `acc4` (≈4 s, 0.55 core) or `acc8` (≈8 s, 0.28 core, +0.4
   pt agreement)?** → **`acc4`.** The accuracy gap is one recording's noise;
   8 s of `them:` on the island before the name lands is long, and half a
   core is affordable on every target. Switch to `acc8` only if the Windows
   dogfood shows CPU pressure — one const.
2. **Hold system finals until labelled (text reaches the model ≤ ~4 s later)
   or send at once with `them:` and never relabel for the model?** → **hold.**
   The batcher already waits up to 25 s; the model sees one consistent label
   per line and no correction traffic. Requests and mic lines are never held.
3. **Line format for the model: `Speaker 2:` vs `them (2):`?** → **`Speaker
   2:`**, with `them:` as the unlabelled fallback; the prompt says so.
4. **Renumber the offline labels to match live (§5)?** → **yes.** Cheap, pure,
   one test; without it the follow-up chat's "Speaker 2" and the reviewed
   transcript's "Speaker 2" can be different people.
5. **Live voiceprint names in v1?** → **no**, next step after dogfood; ship
   `Speaker N` first. When added: proposals only (`John?:` on the island,
   `Speaker 2:` to the model) until confirmed in review.
6. **New `Degradation::DiarizationDown` or silent fallback?** → **new
   variant**: labels vanishing mid-meeting without a cue looks like a bug;
   the island already shows degradations.
7. **Switch semantics: applies at the next meeting start, not mid-meeting?**
   → **yes**; simplest, matches `live_tools`/`models_dir`. Persisted in
   `config.yaml` via `write_config_value`, shown live in Settings.
8. **Default on for Windows/Linux too?** → **yes**, with the switch as the
   escape hatch and no automatic off in v1; add "ring drops → auto-off +
   `DiarizationDown`" if the CPU-only dogfood shows sustained overload.
9. **Thread cap 4 for the live session?** → **yes** (const): CPU seconds are
   the same, step wall time stays under a second, and the STT tasks and the
   capture threads keep their cores.
