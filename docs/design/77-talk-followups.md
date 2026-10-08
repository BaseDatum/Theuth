# 77 — Talk follow-ups: instant acknowledgements (B, C) and STT tiers

Status: research record, not built · 2026-09-29 · extends `76-talk.md` (§0.4
the `Speak` queue, §4.4 barge-in, §4.6 the heard-prefix ledger, §4.8 speaking
committed blocks), `43-voice.md` (the `Voice` actor, Pocket TTS), `45-voice-notes.md`
(Parakeet TDT), `54-dictation.md` (Nemotron streaming, `Utterer`), `57-decision.md`
(laya checkpoints, idle unload) and `meetings/02-live.md` (two live streams). Read
against the as-built records `76-as-built-*.md`, not the 76 text alone. Code
anchors verified on local `main` db4fa2c3 (the same commit
`fix/talk-sentence-stream` branched from).

This note records two researched proposals so a later worker can build them
without redoing the research: **B/C**, an instant spoken acknowledgement that
covers the silence at the start of a talk turn, and **STT tiers**, a size setting
for the speech-to-text models. Sources: the R2 (canned acknowledgements) and R3
(STT tiers) recon summaries of 2026-09-29, the first-audio latency fact, and the
code read for this note.

Names: **filler** = a short canned phrase the talk plays before the agent's
first real sentence ("One moment."). **Real sentence** = a sentence of the
agent's reply, from `speak_committed`. **Deck** = the set of phrases for one
filler class. **Tier** = which STT checkpoint a role (talk and dictation, meeting
live, voice notes) loads.

## 0. Status and rulings

1. **A, sentence streaming: in flight.** Sky, 2026-09-29: build A. Worker
   TALK-STREAM on `fix/talk-sentence-stream` splits the uncommitted markdown tail
   at sentence boundaries in `Input::Delta`, so the first sentence of a paragraph
   is spoken when it ends, not when the paragraph or the turn does (§1). A also
   logs release-build submit → first `SpeechStarted` time, which B's threshold is
   tuned against. Nothing below changes A.
2. **B, neutral filler without a classifier: documented, not built** (§2).
   Build it after A has landed and its first-sentence timing has been measured in
   a release build. If A brings the usual first sentence under ~700 ms, B plays
   only on slow turns and tool turns. That is the intent. *145 §0.8 amendment
   (2026-10-08): when B is built, a filler may play only into idle silence —
   the talk's spoken window open (user silent, nothing queued or pending) —
   never over the user or over queued speech; progress itself stays visual.*
3. **C, classifier-chosen filler with laya: documented, not built** (§3). It has
   a gate: a shadow benchmark on ≥ 150 real spoken turns. Without it, C is not
   built.
4. **STT tiers: plan pending Sky** (§4). The research overturned its premise:
   Jarvis did not run a larger model. The recommendation is a 1.5 h benchmark
   of Parakeet realtime EOU 120M before any tier work, plus the independent
   shared-session fix for meetings. Sky decides whether to queue the benchmark.

## 1. Problem: first-audio latency and the silence while a turn starts

The user stops speaking. The end-of-turn timer fires 250–700 ms later
(`end_of_turn_punct_ms` / `end_of_turn_ms`,
`crates/talk/src/lib.rs#L143-L146`, defaults `#L167-L168`). `submit` sends
`[spoken, N s] …` and enters `Thinking`
(`crates/talk/src/policy.rs#L568-L593`). From then until the first
`SpeechStarted` the room is silent.

What that silence is made of (fact `theuth-talk-first-audio-latency-cause-2026-09-29`):

- **Waiting on our own policy (fixed by A).** The agent publishes a
  `TextDelta` for every chunk. The talk speaks only blocks that
  `markdown::Text::committed()` has closed: `speak_committed`
  (`crates/talk/src/policy.rs#L805-L827`), and the tail on `ToolCallPending` or
  `TurnFinished` (`block_end`, `#L829-L837`, called from `turn_finished`
  `#L867-L871`). A one-paragraph reply is therefore heard when the turn ends.
  The test `a_paragraph_is_spoken_when_it_closes_not_at_turn_end`
  (`crates/talk/src/policy.rs#L1570-L1592`) fixes this behaviour, and so does
  the design text (`docs/design/76-talk.md#L591-L595`). The log of 2026-09-29
  11:05 shows three `Speak`s within 1 ms, which is one block committed at
  `TurnFinished`. The talk prompt claims the reply "is spoken to the user
  sentence by sentence as you write it" (`crates/talk/src/lib.rs#L406-L412`),
  which is false against this code. A makes it true.
- **Time to first token.** The provider's time to first token plus any
  thinking. This is not ours to remove. A filler exists to cover it.
- **Tool calls before any prose.** The prompt asks for one sentence before a
  batch of tool calls (`crates/talk/src/lib.rs#L411-L412`). When the model skips
  it, nothing is heard until the tools finish and the next round writes text.
- **TTS is not the bottleneck.** `ptts` streams: `Synthesizer::stream` wraps
  `Synth::stream_with` (`crates/voice/src/speak.rs#L36-L45`), and frames are
  emitted as they arrive (`#L152-L174`). Release time to first audio is 82–85 ms
  at RTF 0.08 (43 §0.5; debug builds take 7–15 s). The `Voice` FIFO starts
  utterance N+1 only when N has finished (`crates/voice/src/lib.rs#L855-L891`,
  `advance` `#L542-L560`). That gap between sentences is a separate follow-up
  (`docs/design/76-as-built-VOICE-OUT.md#L272-L276`).

No release-build measurement exists yet from submit to first sentence. There
are only debug numbers. A adds that measurement. B's default of 700 ms is the
researcher's estimate of where a pause starts to feel like the agent did not
hear you, not a measured value.

## 2. Option B — a neutral filler, no classifier (recommended; 4–6 h)

### 2.1 Trigger rules

- **Neutral class.** Arm a `Timer::Filler` for `talk.filler_after_ms` (default
  **700**) when `submit` enters `Thinking` after a *spoken* turn
  (`crates/talk/src/policy.rs#L588-L592`). Cancel it when the first
  `Action::Speak` of the turn is emitted (`speak_committed`,
  `#L822-L825`), and on any transition out of `Thinking`. If it fires while
  still `Thinking`, the ledger has no sentence and no filler has played this
  turn, then play one neutral filler.
- **"Let me check" class.** The first `Input::BlockEnd` of a turn
  (`ToolCallPending`, mapped in `crates/talk/src/lib.rs#L1016-L1018`) arrives
  while the ledger is still empty, meaning the model called a tool without saying
  anything first. Play a check filler at once and do not wait for the timer. If
  a neutral filler is still *queued*, the check filler replaces it. If one is
  *playing*, nothing more is added.
- **At most one filler per turn.** Two exceptions only: a check filler may
  replace a queued neutral one, and a check filler may play again at a *later*
  tool round when the whole round produced no text and more than
  `talk.filler_after_ms` has passed since the last audio. Cap: two fillers per
  turn.
- **No filler for typed turns.** A turn typed elsewhere during a talk enters
  `Thinking` through `Input::TurnStarted`
  (`crates/talk/src/policy.rs#L384-L393`). The user did not speak, so there is
  nothing to acknowledge. Also none after a barge-in resume, and none when the
  clip cache for the current voice is not ready (§2.4). The filler is never
  synthesised live.

### 2.2 The filler slot: echo rejection sees it, the heard ledger does not

This is the main risk, and the code already has it. While `Thinking`:

- The gate threshold is the default 0.5. `threshold()` raises it to
  `talk.barge_in_vad` (0.7) only in `Speaking | Holding`
  (`crates/talk/src/policy.rs#L444-L450`; `Gate::THRESHOLD`
  `crates/voice/src/vad.rs#L117-L118`; defaults
  `crates/talk/src/lib.rs#L140-L142`, `#L166-L166`).
- A candidate that has a transcript goes straight to `late_addition`
  (`crates/talk/src/policy.rs#L608-L612`), which cancels the turn
  (`#L739-L758`). No echo or backchannel check runs, because `classify` is
  reached only from `decide`, which requires `Holding`
  (`#L657-L671`, `#L760-L792`).
- `is_echo` compares against `Ledger::recent()`, which covers only ledger
  sentences (`#L254-L267`, `#L794-L801`).

So a filler played as an ordinary ledger sentence would move the state to
`Speaking` (`#L395-L407`) and become part of the heard prefix, which
`commit` joins into `Cancel{keep}` (`#L700-L724`). The model's history would
then contain words it never wrote. A filler played *outside* the ledger with
no other change would leave `Thinking` at 0.5 with no echo check. On a
MicNoAec host its echo would then cancel the very turn it was covering.

Design: a dedicated slot `Policy::filler: Option<Filler { speech, text, class,
status }>`, separate from `ledger.sentences`.

1. **The heard ledger never sees it.** It is not in `ledger.sentences`, so
   `heard`, `commit`, `Cancel{keep}`, `Trim` and `Interrupted.heard/dropped`
   are unchanged. History never contains a filler. `busy()` ignores it, so
   `maybe_listen` does not wait on it (`#L198-L203`, `#L875-L883`).
2. **Echo rejection does see it.** `recent()` becomes "the last two spoken
   things". If the filler played or is playing, its text is prepended
   (it always precedes the first real sentence).
3. **Barge-in threshold 0.7 while it plays.** `threshold()` returns
   `barge_in_vad` in `Thinking` while `filler.status == Playing`. `set()`
   compares the threshold before and after on state changes only
   (`#L432-L442`), so the filler's `SpeechStarted` and `SpeechFinished` must
   push `Action::VadThreshold` themselves.
4. **While it plays, a transcribed candidate is classified before it
   cancels.** In `hold` and `settle`, state `Thinking` with a filler playing
   (or finished within the last ~1.5 s, the echo tail) runs `classify(text)`
   first. Backchannel, echo or noise → dismiss (set `discard` exactly as
   `decide` does, `#L678-L683`, no `Resume` needed because nothing was paused).
   A real word → `late_addition` as today. The filler is never paused on
   stage 1: it is under a second long, and `Pause` exists to protect the heard
   prefix, which a filler is not part of.
5. **State stays `Thinking`.** A filler's `SpeechStarted` publishes
   `Saying{filler: true}` but does not enter `Speaking`. The frontends' phase
   line keeps showing *Thinking…*.

Helpful overlap: several filler words ("okay", "right", "mm") are already in
`talk.backchannels` (`crates/talk/src/lib.rs#L170-L176`). An echo that
transcribes as one of them is dismissed even before the echo ratio is applied.

### 2.3 Skip and drop rules against the FIFO

The `Voice` queue is a plain FIFO with no priority: `Speak{queue: true}` does a
`push_back` (`crates/voice/src/lib.rs#L678-L695`, `Queued` `#L451-L457`,
`current`/`queue` `#L472-L473`), and `advance` pops the front
(`#L542-L560`). A filler queued first plays first.

- **Queued filler dropped when the first real sentence arrives.** When
  `speak_committed` emits the turn's first `Speak` and `filler.status ==
  Queued`, the policy first emits `Action::StopFiller(speech)` →
  `Voice::Stop{id: Some(speech)}`. For a queued id, that removes only that
  entry and publishes `SpeechFinished{Stopped}`
  (`crates/voice/src/lib.rs#L711-L718`). Ordering holds because the actor runs
  actions in order and `Voice` handles its mailbox in order. In practice a
  filler is queued only behind a previous turn's tail or while the clip is
  handed over. The rule exists for those cases.
- **A playing filler finishes.** It is under a second, and cutting it
  mid-word sounds worse than a short overlap. The real sentence waits its turn
  in the FIFO. With the clip in memory, the filler's playback is its only cost.
- **`StopSpeech` (barge-in, `late_addition`, turn cancelled by someone else)
  already stops everything** with `Stop{id: None}` (`#L700-L706`). The filler
  slot is cleared in the same places the ledger is reset.

### 2.4 Pre-rendered clip cache per voice (~4 MB for 40 clips)

The fillers are rendered once and played from memory. They cost no synthesis
at the moment they play, and they work in debug builds, where live TTS takes
seconds.

- **Where it lives:** in the `Voice` actor, beside the model it renders with
  (`model` and `voices` fields, `crates/voice/src/lib.rs#L462-L473`). Keyed by
  `(voice name, phrase)` as `Arc<[f32]>` at 24 kHz
  (`crates/voice/src/speak.rs#L16-L19`). It is not in `talk` (which holds no
  model) and not on disk (a voice or model change would invalidate it, and a
  full re-render takes a few seconds).
- **Size:** f32 at 24 kHz is 96 KB/s. Forty clips of about one second each come
  to **~3.8 MB**. Two decks of up to 12 today, with room for a third class.
- **When:** `talk_start` sends `Voice::PrepareFillers{decks}`. Rendering runs off
  the actor, one clip at a time, behind the existing `clip_gate` semaphore used
  for voice-note clips (`crates/voice/src/lib.rs#L478-L481`, `clip()`
  `#L570-L580`), so it never competes with a second render. Release cost is
  ~40 × ~100 ms of CPU in the background. A deck is usable as soon as its first
  clip exists. Until then there is no filler. The cache is dropped when the
  voice or the TTS model changes, and it is kept across talks.
- **How it plays:** a new `Speak` variant, or a `Speak.clip: Option<ClipKey>`
  field, whose task skips `synth.stream` and feeds the cached frames through the
  same `send` closure: into the ring for `Sink::Device`, Opus-encoded per frame
  for `Sink::Client` (`crates/voice/src/speak.rs#L110-L175`). It reports
  `Started` / `Audio` / `Finished` exactly like a sentence, so the talk's
  `SpeechStarted` / `SpeechFinished` / `Packet` handling needs no special case
  apart from the id lookup going to the filler slot.

### 2.5 Phrase decks

Two classes, 8–12 phrases each, spoken-English, safe to say before *any* reply:
no "good question" (the input may not be a question), no "sure" (it may not be a
request), no promise about the content. Rotation: a shuffle-bag per class per
talk, never repeating either of the last two phrases, reshuffled when empty.
User-overridable via `talk.filler_phrases` (§2.7).

Neutral (`talk.filler_after_ms` passed, no text yet):

1. "Mm, one moment."
2. "Okay."
3. "Right."
4. "Let me think."
5. "Give me a second."
6. "One sec."
7. "Hmm."
8. "Okay, one moment."
9. "Alright."
10. "Hmm, let me see."

"Let me check" (a tool call started before any text):

1. "Let me check."
2. "Let me look."
3. "One moment, I'll check."
4. "Let me find that."
5. "Checking now."
6. "Let me look into it."
7. "I'll take a look."
8. "Give me a second to look."
9. "Let me see what's there."
10. "Looking now."

### 2.6 Subtitle and frontends

- `TalkEvent::Saying` (`crates/talk/src/lib.rs#L336-L344`) gains `#[serde(default)]
  filler: bool`. The event itself is unchanged, and so is the rule that
  subtitles follow `Saying`, not streamed text. The actor publishes it from
  `Action::Saying` (`#L584-L588`).
- Desktop: `applySubtitles` (`crates/desktop/ui/src/lib/talk.ts#L37-L44`) keeps
  `filler` beside `agent`/`speech`, and the talk view renders a filler line
  muted (secondary colour, no emphasis). It is cleared by its own
  `SpeechFinished`, as today. `talk-sim.ts` gains one filler event.
- TUI: `crates/tui/src/talk.rs#L86-L86` stores the flag and draws the line dim.
- Chat transcript: nothing. A filler never reaches history (§2.2), so it
  appears nowhere in the session.

### 2.7 Config (`talk:` section, `TalkConfig` `crates/talk/src/lib.rs#L129-L182`)

| key | default | meaning |
|---|---|---|
| `filler` | `true` (open question 6) | master switch for B |
| `filler_after_ms` | `700` | silence after submit before a neutral filler |
| `filler_on_tool` | `true` | "let me check" when a tool call precedes any text |
| `filler_phrases` | the decks above | `{ neutral: [..], check: [..] }`, each 1–20 entries; empty = class off |

### 2.8 Tests to write

Policy (pure, `crates/talk/src/policy.rs` tests, time injected):

1. Spoken submit, no delta for 700 ms → one `Speak`-filler of class neutral; `state` stays `Thinking`.
2. A `Speak` emitted before 700 ms → timer cancelled, no filler.
3. `BlockEnd` with an empty ledger → a check filler immediately; a second `BlockEnd` in the same round → nothing.
4. Filler still queued when the first real `Speak` is emitted → `StopFiller(id)` is emitted *before* that `Speak`.
5. Filler playing when the first real `Speak` is emitted → no stop; real sentence queued behind.
6. Barge-in during or after filler with real words → `late_addition`, and `Cancel(None)`, never a keep containing filler text.
7. Candidate whose text matches the filler (echo) while it plays → dismissed, turn not cancelled; same for a backchannel.
8. `VadThreshold(0.7)` on filler `SpeechStarted`, `VadThreshold(0.5)` on its `SpeechFinished` while still `Thinking`; no double-emit when the real sentence moves to `Speaking`.
9. `heard()`/`Interrupted.heard` after a filler plus two real sentences counts only the real ones.
10. Typed turn (`TurnStarted` from `Listening`) → no filler; clip cache not ready → no filler; at most two per turn.

Voice (`crates/voice/src/lib.rs` actor tests with the fake `Synthesizer`, `crates/voice/src/speak.rs#L28-L34`):
clips render once per voice, a clip `Speak` reports `Started`/`Audio`/`Finished` for both sinks without calling the synthesizer, and `Stop{id}` of a queued clip removes only it.

Frontend: `talk.test.ts` reducer case for `Saying{filler: true}` followed by its `SpeechFinished`; TUI snapshot of a dim filler line.

Manual, release build, by ear: a slow question (a provider with thinking), a tool question, a quick answer (no filler should be heard), and the aquarium/no-headphones case from the first-use test.

### 2.9 Risks

- **Echo cancelling the turn it covers** (§2.2). This is the reason for the slot. Test 7 has to pass on a MicNoAec host.
- **Stutter.** A filler that ends just as the real first sentence starts. This is accepted for sub-second clips. If it grates, raise `filler_after_ms`, don't add cleverness.
- **Contradiction.** "Let me check" followed by a reply that did not check. Only the check class carries meaning, and it is triggered by an actual tool call, so this cannot happen by design.
- **Repetition** makes the talk feel robotic over a long session. The shuffle-bag, the last-two rule and the one-filler cap limit it.
- **A makes B fire rarely.** That is the intent. The log line A adds tells us how rarely.
- **Client sink latency.** A phone through the relay adds network time. The filler still beats the first real sentence, because both take the same path.

### 2.10 Effort and crates touched

**4–6 h** for one worker, including tests. Crates: `talk` (policy slot, timer,
config, `Saying.filler`, prompt untouched), `voice` (clip cache, clip `Speak`
path, `PrepareFillers`), `desktop/ui` (`talk.ts`, the talk view, `talk-sim.ts`,
regenerated TS bindings via `just ts-desktop`), `tui` (`talk.rs`). No agent,
RPC-method or history changes.

## 3. Option C — a filler chosen by the laya classifier (gated; 6–10 h plus labelling)

**The question it would ask.** At submit, one laya `Choice` over the user's
transcript: which acknowledgement fits? For example `{neutral, check,
answer-now, clarify}`, or a `Bool` such as "The speaker asked for something that
needs looking up". The chosen class picks the deck, and the result could also
skip the filler when an instant answer is expected. The engine's question types
are `Choice`, `Score` and `Noul`/Bool (`crates/decision/src/laya.rs#L78-L107`).

**Why not now:**

- **Accuracy.** On theuth's spoken transcripts the shipped base checkpoint is
  near chance on 3-way `Choice` questions (mean P(correct) 0.27–0.31 on the
  112-batch phrasing bench, everything collapsing to one label). Its Bool head
  is biased to `false` (fact `laya-base-checkpoint-zero-shot-limits`). Spoken
  fragments without punctuation are out of its training distribution. A wrong
  class is worse than a neutral one: "let me check" before a greeting.
- **Memory.** A checkpoint holds ~0.9 GB while loaded
  (`crates/decision/src/lib.rs#L61-L63` for the browser one; the daemon-wide
  footprint is what `LoadedInfo.rss_mb` reports,
  `crates/decision/src/engine.rs#L212-L216`). A talk would need it resident for
  the whole conversation. The idle unload (180 s,
  `crates/decision/src/lib.rs#L57-L59`) would otherwise drop it between turns.
  The talk already holds the TTS model and a 0.9 GB Nemotron stream.
- **Latency.** A cold load takes about 2 s, which `Warm` exists to hide
  (`crates/decision/src/engine.rs#L173-L180`). The filler has a 700 ms budget,
  so C would have to warm at `talk_start` and keep the checkpoint loaded.
- **B already removes the silence.** C's only gain is which words fill it.

**The gate to build it.** Shadow mode first. With B shipped, log every spoken
turn's transcript, the laya answer and its confidence (the engine already logs
every answer, including `load_ms`), and what the agent actually did: tool call
before text, time to first sentence, reply length. Nothing is acted on. Label
**≥ 150 real spoken turns** from Sky's use. Build C only if, on the shadow set:

1. the check-vs-not decision beats B's structural rule (tool call before text)
   *at the moment the filler must play*, meaning it predicts the tool call before
   the model makes it, with precision ≥ 0.9 on "check";
2. misclassifications that would say something wrong (check before a greeting
   or a direct answer) stay ≤ 2 %;
3. p95 ask latency on a warm checkpoint ≤ 100 ms (Metal f16 asks measured
   ~22 ms), and the resident-memory cost is accepted by Sky.

If the base checkpoint fails, the next step is fine-tuning on the labelled turns
(the laya fact records 0.10 → 0.66 with ~5k examples elsewhere), not
prompt tweaks.

**Effort:** 6–10 h for the engine hook, warm-at-talk-start, shadow logging and
the bench harness (reuse `crates/decision/examples/phrasing.rs`), plus Sky's
labelling time for 150+ turns.

## 4. STT tiers

### 4.1 What Jarvis used and what theuth runs

The request assumed that Jarvis (`~/Develop/jarvis`) ran a larger Parakeet. It
did not:

- **Jarvis:** NVIDIA Parakeet TDT 0.6B **v2** (English), ~670 MB, from HF
  `istupakov/parakeet-tdt-0.6b-v2-onnx` on onnxruntime with its own Rust
  decoder. Hashes were never pinned. Its "streaming" re-decoded the whole buffer
  every 400 ms with that offline model. Fallback: whisper.cpp through
  `whisper-rs`. No latency or accuracy was ever recorded.
- **theuth, live (talk, dictation, meetings):** `nemotron-speech-streaming-en-0.6b`
  int8 (origin `scripts/fetch-models.sh#L73-L73`, files `#L51-L53`),
  a cache-aware streaming transducer fed 560 ms windows
  (`crates/voice/src/stt.rs#L32-L34`, `Stream` `#L232-L253`), ~0.9 GB resident
  and always loaded while a dictation, talk or meeting runs.
- **theuth, voice notes and the meeting transcript of record:** Parakeet TDT 0.6B
  **v3** int8 (`scripts/fetch-models.sh#L48-L50`, origin `#L72-L72`), ~1.75 GB
  resident, unloaded after `stt_idle_secs` (600 s) idle
  (`crates/voice/src/stt.rs#L281-L309`, `crates/voice/src/lib.rs#L411-L412`,
  `#L431-L431`).
- Both are REQUIRED downloads (`scripts/fetch-models.sh#L43-L65`). Config has
  three model-folder overrides and no size or tier setting
  (`crates/voice/src/lib.rs#L398-L422`). Nothing reads total machine RAM. The
  only memory probe is the process footprint (`crates/decision/src/lib.rs#L112-L124`).
- **Meetings load the streaming model twice**, one session per track (mic and
  system audio): `crates/meeting/src/lib.rs#L1831-L1839`, ≈ 1.8 GB. The meeting's
  `live_model_dir` defaults to the same checkpoint dictation uses
  (`crates/meeting/src/lib.rs#L539-L541`, `#L1701-L1705`;
  `crates/voice/src/lib.rs#L48-L51`), so it silently shares that model choice.

theuth already runs the largest *streaming* model available. The 1.1B Parakeets
are offline-only.

### 4.2 Candidates

| model | role | size / RAM | WER (Open ASR avg.) | latency | API in `stt.rs` |
|---|---|---|---|---|---|
| Parakeet realtime EOU **120M** | *small* live tier | small (~120M params; exact on-disk and resident size not measured) | **9.3 %** | 80–160 ms chunks, with an end-of-utterance token | different model shape. Needs a second streaming backend beside `Stream(Nemotron)`; its EOU token could later feed end-of-turn |
| Nemotron speech streaming 0.6B, **560 ms** (today) | *large* live tier | ~0.9 GB resident per session | **~7.2 % (estimate**, not taken from NVIDIA's card; confirm before quoting) | 560 ms windows | today's `Stream` |
| Nemotron speech streaming 0.6B, **1120 ms** | "more accurate" live option | same weights, ~0.9 GB | **6.94 %** | 1120 ms windows (worse for barge-in and end-of-turn) | same code path once `WINDOW` is read from the model instead of the `8_960` constant (`crates/voice/src/stt.rs#L32-L34`). **Nobody publishes this ONNX export**, so we would have to make and host it |
| Parakeet TDT 0.6B **v3** (today) | dictation-on-release, voice notes, transcript of record | ~1.75 GB resident, idle-unloaded | best of the set offline | offline, whole clip | today's `Engine` (`crates/voice/src/stt.rs#L170-L200`) |

### 4.3 Proposed setting

- **`voice.stt_tier: auto | small | large`**, applied to talk and dictation.
  `auto` = `large` when **total** RAM is ≥ 16 GB, else `small`. The voice stack
  uses ~4–6 GB at peak (TTS, one or two Nemotron sessions, TDT v3 during a
  voice note). 24 GB would matter only to leave room for local LLMs, and that
  is not this setting's job. `auto` needs a new total-RAM probe (sysctl
  `hw.memsize` / `/proc/meminfo` / `GlobalMemoryStatusEx`) beside
  `process_memory`.
- **Where:** a **Voice** section in Settings, not Labs. Labs is hidden unless
  the decision oracle is on, and this is an ordinary user setting. It shows the
  resolved tier for `auto` and each tier's download state.
- **Optional downloads.** The tier models become optional entries in the models
  manifest, fetched on demand from Settings (`fetch-models.sh` stays the
  REQUIRED set plus whatever tier is resolved). Today's Nemotron stays required
  as the `large` default, so nobody's current setup breaks. The small model is
  added as an optional entry.
- **A separate meeting key**, `meeting.stt_tier` (same values, default
  `large`). Meetings run long and have no barge-in, and the transcript of record
  is TDT v3 anyway. This replaces the silent sharing noted in §4.1.
- **Effort:** ~**12–13 h** of worker time: second backend (~4 h), tier
  resolution and RAM probe (~2 h), optional-download plumbing and Settings UI
  (~4 h), meeting key and tests (~2–3 h). This excludes the 1120 ms export.

### 4.4 Quick win, independent of tiers: one Nemotron session for both meeting streams

Replace the two `Utterer::open` calls (`crates/meeting/src/lib.rs#L1834-L1838`,
each loading the model through `Stream::open`, `crates/voice/src/live.rs#L64-L65`)
with one set of loaded weights and two per-stream decoder and encoder-cache states.
This saves ~0.9 GB for every recorded meeting at ~1.5 h of work. R3 names
`from_shared` in `parakeet-rs` as the constructor for this. **Unverified here:**
no `from_shared` exists in the theuth tree, and the crate source is not in the
local cargo registry. The worker must confirm it in `parakeet-rs` 0.3
(`Cargo.toml` pin) before relying on it. `Stream` is `Send` but not `Sync`
with one owner per stream (`crates/voice/src/stt.rs#L232-L237`). Only the
weights may be shared, never the state.

### 4.5 The gate: a 1.5 h benchmark of EOU-120M first

Before any tier work, run Parakeet realtime EOU 120M on Sky's own clips (the
talk and dictation recordings, a meeting track) against today's Nemotron. Measure:
CPU per second of audio on the Mac, accuracy against a hand-corrected reference,
and **punctuation and casing**. The talk's end-of-turn rule reads terminal
punctuation (`crates/talk/src/lib.rs#L145-L146`, `crates/talk/src/policy.rs#L556-L560`), and
the transcript is shown to the user.

- **Good** (WER within ~2 points of Nemotron on Sky's clips, usable
  punctuation, clearly cheaper): build §4.3 with `small` as the default below
  16 GB.
- **Poor:** skip the small tier. Do §4.4 and the RAM probe, and offer the 1120 ms
  Nemotron as the "more accurate" option if Sky wants one (after the export has
  been made).

### 4.6 Open questions for Sky

1. Queue the 1.5 h EOU-120M benchmark now, or wait?
2. Should `auto` pick `small` below 16 GB only, or be the default everywhere?
   The research recommends below 16 GB only, with Nemotron staying the default on
   the Mac.
3. Meetings: keep `large` by default under their own key?
4. Is the 1120 ms Nemotron worth making and hosting our own export for 0.3
   points of WER and double the window?
5. Do the shared-session meeting fix (§4.4) now, independently of the tier
   decision?
6. (B) Should `talk.filler` default on when B ships?

## 5. Evidence

| claim | anchor |
|---|---|
| talk speaks only committed blocks | `crates/talk/src/policy.rs#L805-L827`, `#L829-L837`, `#L867-L871` |
| test pinning block-level speaking | `crates/talk/src/policy.rs#L1570-L1592` |
| 76 chose block commits deliberately | `docs/design/76-talk.md#L591-L595` |
| prompt claims sentence-by-sentence | `crates/talk/src/lib.rs#L406-L412` |
| deltas and tool calls enter the policy | `crates/talk/src/lib.rs#L1012-L1026` |
| submit enters `Thinking` | `crates/talk/src/policy.rs#L568-L593` |
| typed turn enters `Thinking` | `crates/talk/src/policy.rs#L384-L393` |
| `SpeechStarted` → `Saying`, `Thinking → Speaking` | `crates/talk/src/policy.rs#L395-L408` |
| threshold 0.7 only in `Speaking`/`Holding` | `crates/talk/src/policy.rs#L444-L450`; `crates/talk/src/lib.rs#L140-L142`, `#L166-L166` |
| gate default 0.5 | `crates/voice/src/vad.rs#L117-L118` |
| threshold pushed on state change only | `crates/talk/src/policy.rs#L432-L442` |
| speech during `Thinking` cancels with no echo check | `crates/talk/src/policy.rs#L608-L612`, `#L739-L758` |
| `classify` only from `decide` in `Holding` | `crates/talk/src/policy.rs#L657-L671`, `#L760-L792` |
| echo compares ledger sentences only | `crates/talk/src/policy.rs#L254-L267`, `#L794-L801` |
| heard prefix → `Cancel{keep}`/`Trim` | `crates/talk/src/policy.rs#L700-L724`, `#L205-L226` |
| `busy`/`maybe_listen` | `crates/talk/src/policy.rs#L198-L203`, `#L875-L883` |
| talk config and defaults | `crates/talk/src/lib.rs#L129-L182` |
| `Saying` event | `crates/talk/src/lib.rs#L336-L344`, published `#L584-L588` |
| `Speak{queue: true}` from talk | `crates/talk/src/lib.rs#L562-L583` |
| `Voice` FIFO, push_back / pop_front | `crates/voice/src/lib.rs#L451-L457`, `#L472-L473`, `#L542-L560`, `#L678-L695` |
| `Stop{id}` removes one queued entry; `Stop{None}` clears all | `crates/voice/src/lib.rs#L697-L720` |
| next utterance starts when the previous finishes | `crates/voice/src/lib.rs#L855-L891` |
| inter-sentence gap follow-up | `docs/design/76-as-built-VOICE-OUT.md#L272-L276` |
| TTS streams, 24 kHz f32 frames | `crates/voice/src/speak.rs#L16-L19`, `#L36-L45`, `#L152-L174` |
| fake synthesizer for actor tests | `crates/voice/src/speak.rs#L28-L34` |
| clip render gate | `crates/voice/src/lib.rs#L478-L481`, `#L570-L580` |
| frontend subtitle reducer | `crates/desktop/ui/src/lib/talk.ts#L37-L44`; `crates/tui/src/talk.rs#L86-L86` |
| laya question types | `crates/decision/src/laya.rs#L78-L107` |
| idle unload 180 s / browser 120 s | `crates/decision/src/lib.rs#L57-L63` |
| warm to hide cold load | `crates/decision/src/engine.rs#L173-L180` |
| daemon footprint in `rss_mb` | `crates/decision/src/engine.rs#L212-L216`; `crates/decision/src/lib.rs#L112-L124` |
| Nemotron 560 ms window | `crates/voice/src/stt.rs#L32-L34` |
| `Stream` is one owner per stream | `crates/voice/src/stt.rs#L232-L253` |
| TDT idle unload (1.75 GB) | `crates/voice/src/stt.rs#L281-L309`; `crates/voice/src/lib.rs#L411-L412`, `#L431-L431` |
| no tier setting, three dir overrides | `crates/voice/src/lib.rs#L398-L422` |
| streaming checkpoint shared by dictation and meetings | `crates/voice/src/lib.rs#L48-L51`; `crates/meeting/src/lib.rs#L539-L541`, `#L1701-L1705` |
| meetings open two streaming sessions | `crates/meeting/src/lib.rs#L1831-L1839`; `crates/voice/src/live.rs#L64-L65` |
| required models and origins | `scripts/fetch-models.sh#L43-L65`, `#L72-L73` |
| end-of-turn reads terminal punctuation | `crates/talk/src/lib.rs#L143-L146`; `crates/talk/src/policy.rs#L556-L560` |

Not verified in code (recorded from research or knowledge): release time to
first audio of 82–85 ms (43 §0.5), laya's near-chance 3-way result and the ~2 s
cold load (knowledge `laya-base-checkpoint-zero-shot-limits`, 57), all WER and
latency figures in §4.2 (published model cards; 7.2 % is an estimate), Jarvis's
setup (R3's read of `~/Develop/jarvis`), and `parakeet-rs` `from_shared` (§4.4).
