# Meetings phase 2 — live transcription, batching, topics and briefs

Status: phase design v0 · 2026-09-19 · implements 53 §0 decisions 2, 3, 5,
10 (the tagging half), 11 (the actor half) and §3.3/§3.4/§4.3–§4.7. Depends
on `01-capture.md` (rings, actor, session, store). Contract: `README.md` §3.

Goal of the phase: with the recorder from phase 1 running, the two live
streams are transcribed, batched into the meeting's session, and a real model
produces `meeting_topic`/`meeting_brief` calls that the actor validates,
persists and publishes — all observable headless over RPC. No UI.

## 0. Decisions local to this phase

1. **`voice::stt::Engine` is extracted from `Transcriber`, not duplicated.**
   `crates/voice/src/stt.rs` today holds `ParakeetTDT` privately in
   `Transcriber.model` (`Arc<Mutex<Option<ParakeetTDT>>>`). New `pub struct
   Engine(ParakeetTDT)` with `pub fn load(dir: &Path) -> anyhow::Result<Engine>`
   (moves the `ort_lib` + `from_pretrained` lines at stt.rs:213-217) and
   `pub fn transcribe(&self, pcm16k: &[f32], words: bool) -> anyhow::Result<Piece>`
   (`Piece { text, words: Vec<Word { text, t0_ms, t1_ms }> }`; `words: true`
   → `TimestampMode::Words`, used by phase 4). `Transcriber` keeps its idle
   drop and file decode; its `model` field becomes `Option<Arc<Engine>>`.
   `meeting` uses `Engine` only in phase 4 (the offline record); live text
   comes from `Stream` (1b). This deviates from 53 decision 5 ("one TDT
   engine instance per track"): the guidance is the streaming transducer.
   Rejected: asking `TRANSCRIBER_ACTOR` per utterance (file-based message,
   one queue for two streams).
1b. **Live STT is a stateful cache-aware streaming transducer: NVIDIA
   `nemotron-speech-streaming-en-0.6b` through `parakeet_rs::Nemotron`, one
   session per track; TDT stays the offline engine (voice notes, phase 4's
   transcript of record).** Same crate already pinned at 0.3.7 (`Cargo.lock`),
   same `libonnxruntime`, no new dependency. Contract: 16 kHz mono `f32`,
   fixed 560 ms windows (8 960 samples), `transcribe_chunk(&window)` runs one
   cache-aware encoder step + greedy RNNT decode and returns only the *new*
   text (punctuated, cased); encoder cache and decoder LSTM state live in the
   session across calls; `reset()` only at a real turn boundary. Never
   re-decode a buffer. Wrapped in `voice` as `stt::Stream` (§1) so
   `parakeet_rs` types still do not leave that crate (stt.rs header rule) and
   `ort_lib` stays in one place. Files: `encoder.onnx`, `decoder_joint.onnx`,
   `tokenizer.model` (+ `encoder.onnx.data` for the fp32 export). Use the
   int8 export (`lokkju/nemotron-speech-streaming-en-0.6b-int8`, 880 MB
   encoder) — two fp32 sessions would be ~5 GB resident. CPU execution
   provider (the crate's README: CoreML is unstable for these models).
   Rejected: TDT with VAD endpointing (v0 — text only after the speaker
   stops, 8 s chop); TDT with rolling whole-buffer re-decodes (`../jarvis`
   `ParakeetStream` — O(n²) per utterance, no punctuation, it exists there
   only because jarvis predates cache-aware exports in the crate);
   `ParakeetEOU` 120m (English-only, no punctuation/casing, the crate's own
   example notes its EOU flag "does not work very well in real-world
   tests"; the seam is `Stream` if it improves — one `open` match arm);
   `nemotron-3.5-asr-streaming-0.6b` multilingual (same API and size; drops
   disfluencies; take it only when a non-English meeting is the need —
   `Nemotron::from_pretrained` auto-detects the variant, so it is a model
   directory swap, not code).
1c. **VAD is `wavekat-vad` with the TEN-VAD backend; Silero is a feature-flag
   swap.** One trait (`VoiceActivityDetector::process(&[i16], rate) -> f32`
   probability), backends behind features. On its TEN-VAD test set (README,
   v0.1.17): Silero F1 0.938 / 63 µs per 32 ms frame; TEN-VAD 0.928 / 30 µs
   per 16 ms frame; Earshot 0.928 / 5 µs pure Rust; WebRTC 0.895 / 2 µs,
   binary. TEN-VAD's 256-sample frame divides the Nemotron window exactly
   (8 960 = 35 × 256), so window edges are frame edges. It depends on `ort
   = "2.0.0-rc.12"` which unifies with the workspace pin `=2.0.0-rc.13`
   (`load-dynamic` is additive) — the VAD session must be created **after**
   `voice::stt::ort_lib` has run `ort::init_from`, else `ort` looks for a
   dylib it cannot find; `Stream::open` runs first, so order it that way.
   Its model is fetched at **build** time (`ureq`) unless
   `TEN_VAD_MODEL_PATH` points at a local `ten-vad.onnx` — set it in the
   release build script; the dev build may download. Rejected:
   `flexaudio-vad` (53 §2.2: Silero embedded via `include_bytes!` — no
   build-time fetch, but one backend, a second VAD API in the tree if we
   ever want TEN-VAD/Earshot, and the same `ort` coupling); `earshot`
   (take it only if the build-time download or `ort` ordering bites — it is
   the one-line fallback `features = ["earshot"]`, F1 0.928, no native
   deps); WebRTC (binary output, weakest F1).
2. **Agent gains three small, general messages; nothing meeting-shaped.**
   (a) `agent::SetCompaction { session, at_tokens: Option<u32>, hint:
   Option<String> }` (`ask` → `Result<(), String>`), in-memory on the session
   actor: `at_tokens` replaces `compact_at × window` as the trigger limit at
   `loop_.rs:3092-3109` (and the two `limit =` sites at 3494, 3717); `hint`
   is appended as one paragraph to `COMPACT_INSTRUCTION` (`compact.rs:13`)
   for that session. `None` clears. Sent at meeting start, cleared at stop
   — after the meeting the session compacts like any other, so nothing is
   persisted and no migration exists. (b) `CreateSessionOptions.model:
   Option<String>` — sets that session's model only. `SetModel` (lib.rs:603-612)
   also rewrites the *default* in `config.yaml`, which a meeting must not do.
   (c) `tools::SetApprovals` gains `read_only: bool` (default `false`;
   `enabled: true, read_only: true` = ReadOnly tools skip the ask, others
   ask). 53 §4.5 called this `PolicyOverride::ReadOnlyAllow`; there is no
   `PolicyOverride` in the tree (`crates/tools/src/lib.rs:363-462` has
   `PolicyAction`/`Policy`/`Gate`), so the flag lands on the message that
   already carries per-session approval state. Inherited by nothing.
3. **The batcher lives in the actor, not a thread.** It is a few fields
   (`pending: Vec<Utterance>`, `last_send: Instant`, `turn_running: bool`) and
   a 1 s tokio interval; the actor already owns topics and the session id.
   Rejected: a separate `Batcher` actor (a mailbox for three counters).
4. **Turn state comes from `AgentEvent`.** The actor subscribes
   (`rt.subscribe::<Meeting, AgentEvent>`) and tracks `TurnStarted/
   TurnFinished` for its session → `turn_running`, `ApprovalRequested` /
   resolved → `ApprovalWaiting`, a turn error → `Degradation::ModelDown`
   until the next successful turn. Steer-vs-send is chosen on
   `turn_running` (53 §4.4).
5. **Topic re-touch is a case-insensitive substring scan of each utterance
   over ≤ 6 live labels.** Trivial cost; no index.
6. **Utterance ids are not a contract.** `live.jsonl` lines carry
   `{track, t0_ms, t1_ms, text, request}`; phase 4 replaces the live text by
   time, never by id.

## 1. Files

```
crates/voice/src/stt.rs        Engine (TDT, new pub; Transcriber uses it) + Stream (Nemotron, new pub)
crates/meeting/src/stt.rs      Vad + two streaming tasks, Utterance
crates/meeting/src/batch.rs    Batcher (pure: feed(utterance, now) -> Option<Batch>; unit-tested)
crates/meeting/src/session.rs  line formats (`[meeting hh:mm:ss] …`), SendInput/Steer helpers
crates/meeting/src/topics.rs   Topics { live, dismissed, briefs }: validate/insert/expire/touch (pure; unit-tested)
crates/meeting/src/tools.rs    MeetingTopicTool, MeetingBriefTool, PROMPT, register/unregister
crates/agent/src/{lib.rs,loop_.rs,compact.rs,supervisor.rs,lifecycle.rs}   §0.2 (a), (b)
crates/tools/src/{lib.rs,registry.rs}                                        §0.2 (c)
crates/rpc/src/api.rs, export.rs                                            §6
```

`voice::stt::Stream`: `pub fn open(dir: &Path) -> anyhow::Result<Stream>`
(`Nemotron::from_pretrained(dir, None)` after `ort_lib(dir)`), `pub fn feed
(&mut self, window: &[f32; 8960]) -> anyhow::Result<String>` (new text),
`pub fn reset(&mut self)`, `pub const WINDOW: usize = 8_960`. `!Sync`
by nature — one owner task per stream. `meeting/Cargo.toml` adds `voice`
(for `stt::{Engine, Stream}`), `tools`, `wavekat-vad = { version = "0.1",
default-features = false, features = ["ten-vad"] }` (1c), `rubato` already
present. Feature `ts` forwards `tools/ts`, `voice/ts`.
Config: `meeting.live_model_dir` (default `<data_dir>/models/nemotron-streaming-en`).

Since 54 (dictation, 2026-09-20) `voice::stt::Stream` (with the `ort`
init-once order — `Stream::open` before any `Vad::new`), `voice::vad::{Vad,
Gate, Edge}` and `voice::mic::Capture` already exist in `voice` with the
`wavekat-vad` dependency; phase 2 consumes them and does not build them —
the `Vad` in `meeting/src/stt.rs` reduces to `voice::vad::Gate` over
`voice::vad::Vad` probabilities, and `live_model_dir` defaults to
`voice.dictation_model_dir`.

## 2. Live STT (`meeting/src/stt.rs`)

Two tokio tasks started by `StartMeeting` step 4 (after capture), one per
track, each owning one `voice::stt::Stream` (opened in `spawn_blocking`
from `cfg.live_model_dir`) and one `wavekat_vad::backends::ten_vad::TenVad`
behind a `FrameAdapter`. Both sessions load at start (≈ 0.9 GB each, int8)
and are dropped at stop. Loop:

1. drain `ring_stt` (from phase 1) at 48 kHz; resample to 16 kHz with one
   stateful `rubato` instance held for the whole meeting (FIR state must
   survive frame boundaries); convert to `i16` for the VAD and score every
   256-sample (16 ms) frame; keep the `f32` in a rolling buffer.
2. **onset**: probability > 0.5 on ≥ 10 consecutive frames (160 ms) → the
   utterance opens at the first of those frames minus a 320 ms pre-roll
   (20 frames), so the first syllable is not clipped; `t0_ms` = that point
   on the recording clock. From here the track is *feeding*.
3. **feed** while feeding: every full 560 ms window (35 frames) from the
   utterance start → `spawn_blocking(stream.feed(&window))` (the session
   moves into the blocking task and back; one call in flight per track,
   windows queue behind it — at RTF < 0.5 the queue stays empty). Returned
   text is appended to the open utterance's `text` and published as
   `Utterance { …, final: false }` when non-empty. Nemotron never sees raw
   silence except the one flush window below.
4. **utterance close** (the batcher's unit): probability < 0.5 for ≥ 300 ms
   (19 frames), or `meeting.stt_max_utterance_ms` (default 15 000) of
   continuous speech, or (mic only) the PTT release flag (`request_pending:
   AtomicBool` set by `PushToTalk { held: false }`) → pad the partial
   window with zeros and feed it, then feed one window of zeros to pull the
   tail text out of the decoder's lookahead; close: `Utterance { track,
   t0_ms, t1_ms, text, request, final: true }` (`t1` = silence start).
   Empty text → nothing emitted. Feeding **stops** here; the session keeps
   its cache (a 300 ms–2 s pause is the same speaker mid-thought).
5. **turn boundary** — silence ≥ 2 s, PTT release, `Pause`, `SttDown`
   recovery: `stream.reset()` and `vad.reset()`. Per the model: reset on
   real boundaries, never per window; no CPU is spent on a muted
   participant because nothing is fed between close and the next onset.
6. the actor: `final: true` → one JSON line in `live.jsonl`,
   `MeetingEvent::Utterance`, feed the batcher, `touch` topics; `final:
   false` → `MeetingEvent::Utterance` and `touch` only (the control center
   replaces the open row in place; nothing is persisted).

`Utterance` gains `final: bool` in the README §3 contract (`#[serde(default
= "true")]`-style default so phase-1 bindings stay valid). `request` is
`true` for the mic utterance that closes because the PTT key was released
while it was open (or the first utterance to close within 1.5 s after
release, if the user released in a pause). The span is otherwise ordinary:
recorded, transcribed, in the file (decision 10).

Latency: text appears window by window (560 ms) plus the export's
algorithmic lookahead; a finished sentence is complete ≈ 300 ms (VAD) +
one flush window after the speaker stops. Word timestamps: none from the
streaming model — `t0/t1` are utterance-level from the VAD; phase 4's TDT
pass supplies word times for the record.

Failure: a session open or `feed` panic (`catch_unwind`, as `Transcriber`)
→ `Degradation::SttDown(track)`, the task exits, the recording continues
(decision 12). No automatic reload in v1.

`Pause` stops draining and resets both sessions; on `Resume` the ring is
discarded and the VAD reset.

Memory: two int8 sessions ≈ 1.8 GB resident during a meeting (53 §2.3's
2.4 GB estimate was for two TDT engines); dropped in `StopMeeting` before
the post pipeline loads its single TDT `Engine` (phase 4).

## 3. Batching (`meeting/src/batch.rs`, pure)

```rust
pub struct Batcher { pending: Vec<Utterance>, last_send: Option<Instant>, secs: u64, max: usize }
impl Batcher {
  pub fn feed(&mut self, u: Utterance, now: Instant) -> Option<Vec<Utterance>>;  // rules (a)(b)(c)
  pub fn tick(&mut self, now: Instant) -> Option<Vec<Utterance>>;                // rule (a) on the 1 s interval
}
```

Rules (53 §4.4): (a) `batch_secs` since `last_send` and ≥ 1 pending; (b)
`batch_utterances` pending; (c) a `System` utterance ending in `?` flushes
at once. `request == true` utterances never enter the batcher — they go out
immediately as `[meeting request, spoken] <text>`.

Line format (`session.rs`):

```
[meeting 00:14:32] live: SRE-2386, DR cutover · dismissed: KEDA
them: so the DR sync for two-three-eight-six is blocked on the image tags
me: right, I think the app patches those
```

`hh:mm:ss` = `t0_ms` of the first utterance in the batch on the recording
clock. `live:`/`dismissed:` are the actor's current label sets (`none` when
empty). Delivery: `turn_running` → `agent::Steer { session, id: InputId::new(),
text, files: [] }`; else `agent::SendInput { session: Some(id), text, files:
[] }`. Both are `ask` on `AGENT_ACTOR` (`crates/agent/src/lib.rs:97-122`).
Steers are unconditional: a batch is never dropped for a slow turn; if the
queue depth (`ListQueued`) exceeds 3, the next batch merges the pending
lines into one input (bound on backlog after a long tool call).

Other lines, all sent the same way (Steer if a turn runs):
`[meeting request, typed] …`, `[meeting tapped "X"]`, `[meeting: user
dismissed "X" — not useful]`, `[meeting deeper on "<title>"] - b1\n- b2`,
and the first input at start: `[meeting 00:00:00] started. context: <text
or none>`.

53 §4.4's detector seam: `Batcher::feed` is the only function that would
change.

## 4. Session setup and teardown

At `StartMeeting` (extends phase 1's handler): `CreateSessionOptions {
model: manifest.session_model, .. }` (§0.2 b); `SetCompaction { session,
at_tokens: Some(cfg.context_tokens), hint: Some(COMPACTION_HINT) }`;
`SetApprovals { session: Some(id), enabled: true, read_only:
cfg.allow_read_only }`; `RegisterTool` × 2 (§5); the `started.` line. The
`COMPACTION_HINT` const: *"This session is a live meeting. Keep: who is in
the meeting, what has been discussed in order, decisions and numbers said
aloud, the current live topics and every dismissed topic."*

At `StopMeeting`: `UnregisterTool("meeting_topic")`, `("meeting_brief")`;
`SetCompaction { session, at_tokens: None, hint: None }`; flush the
batcher's pending lines as one last input; drop the engines. The prompt
block (§5.3) is registered once at plugin start and stays — it is a pure
function of config and only speaks about `[meeting …]` lines.

## 5. Tools (`meeting/src/tools.rs`)

### 5.1 `meeting_topic` — `Risk::ReadOnly`

```rust
pub struct TopicArgs { pub label: String, pub brief: BriefBody }
pub struct BriefBody { pub title: String, pub bullets: Vec<String> }
```

`call` → `ask` `MEETING_ACTOR` `AddTopic { session, label, brief }` →
`Topics::insert` validates: no recording for this session → `ToolOutput::err
("no meeting is recording in this session")`; label > 4 words → err;
bullets ∉ 2..=3 → err; label equals (case-insensitive, trimmed) a live or
dismissed label → err `"topic already live/dismissed"`; more than
`max_topics` live → expire the oldest untouched (`TopicRemoved{Expired}`) and
insert. On success: brief stored unshown, `TopicAdded` published, both
appended to `briefs.jsonl`, reply `"topic registered"`.

ReadOnly argument (per `docs/plugins.md` §3.5): the only effect is a row in
the meeting's own store and a redraw; no world state.

### 5.2 `meeting_brief` — `Risk::ReadOnly`

```rust
pub struct BriefArgs { pub title: String, pub bullets: Vec<String>,
    pub topic: Option<TopicId>, pub parent: Option<BriefId> }
```

Same validation for bullets. `source` is inferred by the actor from what
prompted the turn: the last steer it sent for this session (`Tap` if a
`[meeting tapped]` is outstanding for `topic`, `Deeper` if a `[meeting
deeper]` is outstanding for `parent`, `Request`/`Typed` if a request line
is outstanding, else `Auto`). `topic: Some(t)` → `TopicRemoved{Viewed}` for
`t` and the topic's precomputed brief is superseded. `Brief` is published
at once (decision 11).

Both tools are registered only while a meeting records (§4). A session
without a meeting never sees them; the registry list is read per turn.

### 5.3 Prompt block

`tools::RegisterPrompt { plugin: "meeting", text: PROMPT }` at plugin start,
text = 53 §3.4 verbatim (one paragraph, static).

## 6. Actor additions and RPC

New messages (all `ask` → `Result<_, String>` unless noted): `TapTopic
{topic}`, `DismissTopic {topic}`, `Deeper {brief}`, `TypedRequest {text}`,
`PushToTalk {held}` (publishes `Listening`), `SetCursor {brief: Option<
BriefId>}` (stored; mirrored to `MeetingStatus.cursor`), `ListBriefs
{meeting}` → `Vec<Brief>` (from `briefs.jsonl` or memory), `ListTopics` →
`Vec<Topic>` (live set), `LiveText {meeting}` → `Vec<Utterance>` (from
`live.jsonl`; capped 2 000 lines). Internal (`tell`): `Utterance`, `AddTopic`,
`AddBrief`.

Expiry: the 1 s interval also runs `Topics::expire(now, ttl)` →
`TopicRemoved{Expired}`; `touch(label)` is called for every utterance whose
text contains a live label.

RPC: `meeting_tap(TopicId)`, `meeting_dismiss(TopicId)`,
`meeting_deeper(BriefId)`, `meeting_request(String)`, `meeting_ptt(bool)`,
`meeting_cursor(Option<BriefId>)`, `meeting_briefs(MeetingId) →
Vec<Brief>`, `meeting_topics() → Vec<Topic>`, `meeting_live(MeetingId) →
Vec<Utterance>`. `Status` gains `cursor`, `topics: Vec<Topic>`,
`approval_waiting: bool`. `export.rs`: `Utterance`.

## 7. Data on disk

`live.jsonl`: one `Utterance` per line, appended as produced. `briefs.jsonl`:
one `{"topic": Topic}` or `{"brief": Brief}` or `{"removed": {topic, why,
at_ms}}` per line. Both are truth for what happened live; phase 4's
transcript of record does not overwrite `live.jsonl`.

## 8. Not in this phase

Any UI (03). The detector model (53 decision 2, deferred). Post tools
`meeting_read/search/speakers/list` (04) — but §5.3's prompt already
mentions them; the model will get "unknown tool" until phase 4, acceptable
for a headless phase.

## 9. Check

Headless, scratch data dir, a fast model configured, a knowledge bundle with
one known ticket (`SRE-2386`) and one system name; Music/QuickTime playing a
recorded two-person conversation that mentions both, the mic live:

1. `cargo test -p meeting` — `Batcher` rules (a)(b)(c) and the merge-on-
   backlog bound; `Topics` validation (word count, bullet count, duplicate,
   max/expire order, touch); line formatting incl. `live:`/`dismissed:`
   rendering and `hh:mm:ss`. `cargo test -p voice` still green after the
   `Engine`/`Stream` additions; `Transcribe` on the smoke WAV unchanged; a
   `Stream` test feeds the smoke WAV in 8 960-sample windows plus one zero
   window and gets the smoke sentence back, punctuated.
2. `meeting_start` → both sessions open in < 5 s (log the load time);
   `Utterance{final:false}` events with `track: System` for the clip and
   `Mic` for your own speech arrive while the speaker is still talking
   (≤ 1.5 s behind the audio); `final:true` closes ≈ 300 ms + one window
   after a pause; `live.jsonl` gains only finals; CPU per track stays under
   one core (`top` while both talk); `list_tools(session)` shows
   `meeting_topic`/`meeting_brief` as `read_only`; `get_prompt` shows the
   block.
2b. Feeding stops at every utterance close (CPU falls to the VAD's ~30 µs
   per frame between sentences — `top` shows the drop); silence ≥ 2 s
   resets the session and the next word starts a new utterance with
   correct casing; the first word of an utterance is intact (pre-roll);
   a 40 s monologue → utterances closed every ≤ 15 s with no dropped words
   at the cuts (compare against the clip's script); a cough or keyboard
   burst < 160 ms does not open an utterance.
3. `subscribe_events(session)` shows `Input*` for a `[meeting 00:00:xx]
   live: none · dismissed: none` batch within `batch_secs`; a `?`-ending
   system utterance flushes early.
4. The model calls `meeting_topic` for `SRE-2386` → `TopicAdded` with a
   ≤ 4-word label and a 2–3 bullet brief in `briefs.jsonl`.
5. `meeting_tap` → `[meeting tapped]` steer → `Brief{source: Tap}` and
   `TopicRemoved{Viewed}` (if the model calls `meeting_brief`) or just
   `TopicRemoved{Viewed}` + the stored brief shown (if it does nothing —
   the actor shows the precomputed brief on tap regardless).
6. `meeting_dismiss` → steer line; the same label is not re-added within
   the next two batches; `meeting_topic` with that label → tool error.
7. `meeting_ptt(true)`, ask "what is the DR cutover date", `meeting_ptt
   (false)` → `Listening{true/false}`, an `Utterance{request: true}`, an
   immediate `[meeting request, spoken]` steer, a `Brief{source: Request}`.
8. `meeting_request("who owns KEDA")` → `Brief{source: Typed}`.
9. Six topics live → the seventh expires the oldest (`Expired`); wait
   `topic_ttl_secs` with a label unmentioned → `Expired`.
10. Set `context_tokens: 4000`; talk past it → a compaction epoch on the
    session (`/compact`-style `Compacted` event) whose head mentions live
    topics; batches continue as cache-hit prefixes (inspect one request
    log: prompt-cache tokens ≫ new tokens).
11. `allow_read_only: true` → a `knowledge_search` from the model runs
    without `ApprovalRequested`; a `Mutating` tool still asks and
    `ApprovalWaiting{true}` is published; `false` → ReadOnly asks as usual.
12. Kill the model provider (bad key) mid-meeting → `ModelDown` in `State`,
    `live.jsonl` still growing, files still growing.
13. `meeting_stop` → tools gone from `list_tools`, `SetCompaction`
    cleared (a `/compact` afterwards uses the plain instruction), engines
    released (RSS drops).

## 10. Status

Not started.
