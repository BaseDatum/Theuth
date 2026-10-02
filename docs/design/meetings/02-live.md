# Meetings phase 2 — live transcription, batching, topics and briefs

Status: phase design v0 · 2026-09-19 · implements 53 §0 decisions 2, 3, 5,
10 (the tagging half), 11 (the actor half) and §3.3/§3.4/§4.3–§4.7. Depends
on `01-capture.md` (rings, actor, session, store). Contract: `README.md` §3.

Goal of the phase: with the recorder from phase 1 running, the two live
streams are transcribed, batched into the meeting's session, and a real model
produces `meeting_topic`/`meeting_brief` calls that the actor validates,
persists and publishes — all observable headless over RPC. No UI.

## 0. Decisions local to this phase

1. **`voice::stt::Engine` (TDT) is *not* built in this phase.** Deferred to
   phase 4, the only consumer (reconciled 2026-09-20: the tree has `Stream`
   but no `Engine`; extracting it here would be code nobody calls until
   04). **Built in phase 4** (2026-09-20, `04-post.md` §10): `Engine`,
   `Piece`, `Word` as below, `Transcriber.model` is `Arc<Mutex<Option<Engine>>>`,
   `voice::TDT_MODEL_SUBDIR` names the checkpoint directory. The original
   design, as executed:
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
2. **Agent gains two small, general messages; nothing meeting-shaped.**
   (a) `agent::SetCompaction { session, at_tokens: Option<u32>, hint:
   Option<String>, head: Option<String> }` (`ask` → `Result<(), String>`), in-memory on the session
   actor: `at_tokens` replaces `compact_at × window` as the trigger limit
   (the two `limit =` sites in `loop_.rs`, ~3492 and ~3715 as of 2026-09-20);
   `hint` is appended as one paragraph to `COMPACT_INSTRUCTION`
   (`compact.rs:13`) for that session — note `loop_.rs` compares
   `instruction == COMPACT_INSTRUCTION` (~4056) to tell a summary from a
   harvest, so the hint must be carried beside the instruction, not
   concatenated into the compared string. `head` (MEETING-HEAD, 2026-10-02)
   is a ready-made head body: every compaction of the session that brings
   no head of its own (auto, and `Compact` without `head`) uses it as the
   given summary, and Summary style is forced over `agent.compact_style` —
   no summariser call, no snap frames, no pre-compaction harvest; `/clear`
   is untouched. `None` clears each. Sent at meeting
   start, cleared at stop — after the meeting the session compacts like any
   other, so nothing is persisted and no migration exists. (b)
   `CreateSessionOptions.model: Option<String>` (`lifecycle.rs:23-36`) — sets
   that session's model only. Verified: `SetModel` (`supervisor.rs`
   `impl Message<SetModel>`) always sets `default_model` and rewrites
   `agent.model` in `config.yaml` even with `session: Some`, which a meeting
   must not do.
   (c) **Dropped** (2026-09-20): 53 §4.5's "ReadOnly tools run without
   asking" is already the default — `tools::Policy::default()` has
   `read_only: PolicyAction::Allow` (`crates/tools/src/lib.rs:450-458`). A
   user who set `tools.policy.read_only: ask` chose that for every session;
   meetings do not override it. No `SetApprovals.read_only`, no
   `meeting.allow_read_only` key (removed from README §3).
7. **The live audio→utterance core is shared with dictation, not
   duplicated.** `crates/voice/src/dictation.rs` `Task` (`chunk`,
   `feed_full`, `close`, `append`, with `buf/base/total/open/text`) is
   exactly the per-track loop §2 describes. Extract it into
   `voice::live::Utterer` (or `stt::Live`; name by function): owns `Stream +
   Vad + Gate + FftFixedIn`, `pub fn open(dir) -> Result<Utterer>`
   (`Stream::open` first, then `Vad::new` — the `ort` order), `pub fn
   chunk(&mut self, pcm48: &[f32]) -> Result<Vec<Text>>` with `Text {
   utterance: u32, start: usize /* 16 kHz sample of the utterance start,
   after pre-roll */, text: String, is_final: bool }`, `pub fn close(&mut
   self) -> Result<Vec<Text>>`, `pub fn reset(&mut self)`. Dictation's `Task`
   keeps its mic loop, `Report`, levels and deadline and calls the core;
   the meeting's two tasks feed it from `ring_stt`. `Gate::MAX_OPEN` is a
   const (15 s) — `meeting.stt_max_utterance_ms` is removed from README §3
   rather than plumbed through for the same value.
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
crates/voice/src/live.rs       Utterer (extracted from dictation::Task; §0.7) — dictation.rs calls it
crates/meeting/src/stt.rs      two streaming tasks over ring_stt → Utterance (t0/t1 on the recording clock)
crates/meeting/src/batch.rs    Batcher (pure: feed(utterance, now) -> Option<Batch>; unit-tested)
crates/meeting/src/session.rs  line formats (`[meeting hh:mm:ss] …`), SendInput/Steer helpers
crates/meeting/src/topics.rs   Topics { live, dismissed, briefs }: validate/insert/expire/touch (pure; unit-tested)
crates/meeting/src/tools.rs    MeetingTopicTool, MeetingBriefTool, PROMPT, register/unregister
crates/agent/src/{lib.rs,loop_.rs,compact.rs,supervisor.rs,lifecycle.rs}   §0.2 (a), (b)
crates/tools/src/lib.rs                                                     nothing (§0.2 c dropped)
crates/rpc/src/api.rs, export.rs                                            §6
```

`voice::stt::Stream`: `pub fn open(dir: &Path) -> anyhow::Result<Stream>`
(`Nemotron::from_pretrained(dir, None)` after `ort_lib(dir)`), `pub fn feed
(&mut self, window: &[f32; 8960]) -> anyhow::Result<String>` (new text),
`pub fn reset(&mut self)`, `pub const WINDOW: usize = 8_960`. `!Sync`
by nature — one owner task per stream. `meeting/Cargo.toml` adds `tools`
(`voice` is already a dependency since phase 1; `wavekat-vad` stays in
`voice` only), `rubato` already present. Feature `ts` forwards `tools/ts`,
`voice/ts`. Config: `meeting.live_model_dir` (default: the same directory
dictation uses — `<data_dir>/models/nemotron-streaming-en`, `voice/src/lib.rs:680`;
lift that literal into a `pub const voice::STREAMING_MODEL_SUBDIR` both
plugins join onto `data_dir`, so neither reads the other's config section).

Since 54 (dictation, 2026-09-20) `voice::stt::Stream` (with the `ort`
init-once order — `Stream::open` before any `Vad::new`), `voice::vad::{Vad,
Gate, Edge}` and `voice::mic::Capture` already exist in `voice` with the
`wavekat-vad` dependency; phase 2 consumes them and does not build them —
the `Vad` in `meeting/src/stt.rs` reduces to `voice::vad::Gate` over
`voice::vad::Vad` probabilities, and `live_model_dir` defaults to
`voice.dictation_model_dir`.

## 2. Live STT (`meeting/src/stt.rs`)

Two tokio tasks started by `StartMeeting` step 4 (after capture), one per
track, each owning one `voice::live::Utterer` (§0.7: `Stream` + `Vad` +
`Gate` + resampler; opened in `spawn_blocking` from `cfg.live_model_dir`).
Both sessions load at start (≈ 0.9 GB each, int8) and are dropped at stop.
The loop below is what `Utterer` implements (it is `dictation::Task`'s
today); the meeting task only drains the ring, maps `Text.start` to the
recording clock, and handles PTT:

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
   (19 frames), or `Gate::MAX_OPEN` (15 s) of
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
   replaces the open row in place; nothing is persisted). **Amended by 07
   (2026-09-24):** with live diarization on, a `System` final is published at
   once but its `live.jsonl` line and batcher feed wait (≤ ~8 s) until the
   diariser has scored it, then go out with `speaker: Some("sN")` and a second
   `final: true` event for the same `track`/`t0_ms`; the alias scan and topics
   still run at close time. Mic finals and requests are never held.

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
at_tokens: Some(cfg.context_tokens), hint: Some(COMPACTION_HINT), head:
Some(head::render(initial)) }` (`context_tokens` defaults to 32 000);
`RegisterTool` × 2 (§5); the `started.` line. The
`COMPACTION_HINT` const: *"This session is a live meeting. Keep: who is in
the meeting, what has been discussed in order, decisions and numbers said
aloud, the current live topics and every dismissed topic."* — the
summariser's fallback while no head is set.

While recording, the head is re-rendered (`head::of`) immediately before
every batch delivery (`deliver_batch`) and after every topic add
(`topic_added`), removal (`topic_gone`: viewed, dismissed, expired) and
brief (`show_brief`); when it differs from `Live::last_head` it is pushed
with the same `SetCompaction` (trigger, hint and head together) — one ask
per change, at most one per batch, never on a timer.

At `StopMeeting`: `UnregisterTool("meeting_topic")`, `("meeting_brief")`;
`SetCompaction { session, at_tokens: None, hint: None, head: None }`; flush the
batcher's pending lines as one last input; drop the engines. The prompt
block (§5.3) is registered once at plugin start and stays — it is a pure
function of config and only speaks about `[meeting …]` lines.

### 4.1 Meeting memory shape

The chat transcript is not the meeting's memory. Three layers:

- **Head** — deterministic meeting state rendered by the plugin
  (`meeting/src/head.rs`): title, start time, elapsed, speakers heard, live
  topics with their batch-line label and first-seen stamp, dismissed
  labels, the briefs given (last 20, "… and N earlier"), outstanding
  steers, the meeting id, and two fixed lines — a `meeting_read(meeting=…,
  to_ms=…)` pointer to the earlier transcript and the standing rule not to
  re-offer a live or dismissed topic. Stable ordering, no wall-clock
  "now" beyond the elapsed stamp; the same state renders the same text.
- **Tail** — the last minutes of `[meeting …]` lines verbatim, sized by the
  agent as a fraction of `context_tokens − fixed` (COMPACT-FLOOR): with a
  32k trigger and a ~7k system prompt roughly the last 8–9k tokens stay,
  and the session compacts every ~15k tokens of growth at no model cost.
- **Record** — the meeting store (`live.jsonl`, `briefs.jsonl`), read back
  with `meeting_read` / `meeting_search` when a question needs it;
  `meeting_read` is in the default `live_tools` so the head's pointer is
  actionable.

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
11. With the default `tools.policy`, a `knowledge_search` from the model
    runs without `ApprovalRequested`; a `Mutating` tool still asks and
    `ApprovalWaiting{true}` is published.
12. Kill the model provider (bad key) mid-meeting → `ModelDown` in `State`,
    `live.jsonl` still growing, files still growing.
13. `meeting_stop` → tools gone from `list_tools`, `SetCompaction`
    cleared (a `/compact` afterwards uses the plain instruction), engines
    released (RSS drops).

## 10. Status

Implemented 2026-09-20 (`crates/meeting` +≈1 900 lines incl. tests: `batch.rs`,
`session.rs`, `stt.rs`, `tools.rs`, `topics.rs`, actor/state additions;
`crates/voice/src/live.rs` +330 extracted from `dictation.rs`; agent +≈120;
rpc/theuth wiring). Verified headless over JSON-RPC on `/tmp/theuth-meet`
(`THEUTH_RPC__LISTEN=127.0.0.1:7791 ./target/debug/t --headless --data-dir
/tmp/theuth-meet`, plus `ORT_DYLIB_PATH` pointing at the TDT checkpoint's
`libonnxruntime.1.27.0.dylib` and `THEUTH_SECRETS__KEY_STORE=file` — the rebuilt
binary otherwise blocks on a Keychain dialog; `models/` symlinked to
`~/.theuth/models`), macOS 15, `anthropic/claude-haiku-4-5` as `session_model`,
a knowledge bundle with `tickets/sre-2386` and `systems/keda`, QuickTime
Player playing a 114 s two-voice `say` clip (Samantha/Daniel: SRE-2386 spoken
as digits, KEDA, a `?` question, a 40 s monologue), the external mic live,
built-in speakers (no headphones). Four meetings were run; the numbers below
are from runs 1–4.

### Exercised

1. `cargo test -p meeting` 13/13 (+7 new: `Batcher` rules (a)(b)(c) and the
   defer/merge bound; `Topics` validation, duplicate vs live/dismissed,
   expire-oldest-untouched at `max`, TTL by touch time, substring touch,
   `briefs.jsonl` record shape; §3's batch line verbatim, `hh:mm:ss`,
   `live:`/`dismissed:`/`none`, the other five lines). `cargo test -p voice`
   12/12 incl. `live::utterer_returns_the_spoken_sentence_punctuated`: a
   `say` sentence at 48 kHz in 20 ms chunks with a second of silence either
   side → one utterance, rolling pieces, a cased and punctuated final, the
   start inside the leading silence (pre-roll); skips with a message when
   the checkpoint or `libonnxruntime` is absent. `cargo test -p agent`
   232/232; `context_edit_is_idle_only_and_installs_committed_frame`
   extended: `SetCompaction` replaces the threshold (also with an unknown
   window), the hint rides behind `COMPACT_INSTRUCTION` and the prefix test
   still classes it a summary, `None` clears both. `cargo check --workspace
   --all-targets` clean; `cargo clippy -p meeting -p voice -p rpc -p theuth
   --all-targets --no-deps -D warnings` clean. **`cargo clippy -p agent` fails
   on 14 pre-existing sites** (`communication/network.rs`, `store/
   communication.rs`, `store/accounting.rs`, `supervisor.rs:3130`,
   `loop_.rs stream_once`, two `MutexGuard`-across-await in tests) that
   `git stash` reproduces without this phase's changes; not fixed here.
   `cargo check -p rpc --features ts`; `just ts` emits `Utterance.ts`,
   `SetCompaction`-free (not wire), `CreateSessionOptions.model`,
   `MeetingStatus.ts` (see deviation 8).
2. `meeting_start` → `Started` in 2.6–2.8 s; both Nemotron int8 sessions load
   in 1.99 s (1.08 + 0.85, logged); RSS 2.54–2.59 GB while recording.
   `Utterance{final:false}` on `System` for the clip and `Mic` for the
   user's speech arrive every 560 ms window while the speaker talks (first
   partial 0.6–1.1 s after speech starts); `final:true` closes 0.8–0.9 s
   after the speaker stops (≈300 ms VAD + one window + one flush); the
   user's *"Testing one two three. The Pulumi state migration is on my
   list."* / 3 s / *"Second sentence …"* came back as those words with
   `Second` capitalised after the ≥ 2 s turn reset. `live.jsonl` holds only
   finals (26 / 38 lines), every line non-empty. `list_tools` shows
   `meeting_topic` and `meeting_brief` as `read_only` only while recording;
   `get_prompt` contains the block. CPU per track: not measured with `top`
   (not exercised as a number).
2b. The 40 s monologue closed at every ≤ 15 s cut with continuous text
   across the cuts (*"… the moving of T-4 from 200 to about 40,000
   messages in …"*). Pre-roll: first words intact (*"Okay, let's get
   started"*, *"Testing"*). A cough/keyboard burst was not staged. The
   between-sentence CPU drop was not measured.
3. Batches: `[meeting 00:00:03] live: none · dismissed: none` + `them:`/`me:`
   lines as `TurnStarted` when idle and as `InputQueued` → `InputInjected`
   while a turn ran (runs 3–4, many); the `?`-ending system utterance (*"Is
   it still October 3rd?"*) flushed within a second of its close. Prompt
   cache: `TurnFinished.usage` on batch turns `prompt 12 936 / cached 12 933`.
4. The model called `meeting_topic` for `SRE-2386` (label *"SRE-2386 image
   tags"*, run 4) and for KEDA (*"KEDA scaling too aggressively"*, run 1;
   *"SQS quota hitting weekly"*, run 3 — after the actor refused *"SQS quota
   hitting twice weekly"* with `label has 5 words; at most 4` and the model
   shortened it); `TopicAdded` with a 3-bullet brief, both in `briefs.jsonl`.
5. `meeting_tap` → `TopicRemoved{Viewed}` + the stored brief as
   `Brief{source: Tap}` at once; `[meeting tapped "…"]` steered; the model
   then called `meeting_brief` (without `topic`, so its brief was `Auto`).
6. `meeting_dismiss` → `TopicRemoved{Dismissed}`, `[meeting: user dismissed
   "Cutover date confirmation" — not useful]` steered; a later
   `meeting_topic` with that exact label → `topic already dismissed`; the
   label never reappeared in three further batches.
7. PTT (clip paused, so the mic was clean): `meeting_ptt(true)` →
   `Listening{true}`; partials *"What is the DR cut over date"* while
   speaking; `meeting_ptt(false)` 35 s later → `Listening{false}`, one
   `Utterance{request: true, final: true}` spanning the held time, and
   `[meeting request, spoken] What is the DR cut over date` as `TurnStarted`
   0.1 s after the release. The model answered with **text**, not
   `meeting_brief` (see not exercised).
8. `meeting_request("who owns KEDA")` → `[meeting request, typed]` →
   `Brief{source: Typed, title: "KEDA ownership"}` (run 1).
9. Six live topics; the seventh and eighth `meeting_topic` each produced
   `TopicRemoved{Expired}` for the oldest untouched plus `TopicAdded`.
   TTL expiry (600 s unmentioned) was not waited for; unit-tested.
10. `THEUTH_MEETING__CONTEXT_TOKENS=4000`: `Compacted{style: summary}` on the
    meeting session every round (`before` 13–16 k — the system prompt and
    tools alone exceed 4 k — `after` ≈ 1 k); the head lists *"Live topics:
    "Cutover date confirmation", "SQS quota hitting weekly" — none
    dismissed"* and the numbers said aloud (the hint's ask; the plain
    instruction has no such heading). Batches after it: cached ≈ all of the
    prompt (item 3).
11. Default `tools.policy`: `knowledge_get`/`knowledge_put`/`meeting_*` ran
    with no `ApprovalRequested`; a `shell` call → `ApprovalRequested` +
    `ApprovalWaiting{true}` + `Status.approval_waiting: true`;
    `resolve_approval(deny)` → `ApprovalResolved`, `ApprovalWaiting{false}`.
12. `set_model(session, "openai/gpt-4o-mini")` (no key) → the next request
    turn `TurnFinished{error}` → `State{Recording, degraded: [ModelDown]}`,
    `live.jsonl` and the three `.opus` still growing; `set_model` back →
    the next `end_turn` cleared `ModelDown`.
13. `meeting_stop` 0.44 s: tools gone from `list_tools`, `State{Done}`,
    RSS 2 537 → 1 117 MB and 2 576 → 1 157 MB (both sessions released),
    `briefs.jsonl` = `topic`/`brief`/`removed` lines. A `/compact` after
    stop ran (epoch 23); that it used the plain instruction is covered by
    the unit test only — the head carries the earlier summary forward, so
    the wire cannot tell.

### Not exercised

- `Brief{source: Request}` and `Brief{source: Deeper}`: in runs 3–4 the
  fast model answered `[meeting request, spoken]`, `[meeting request,
  typed]` and `[meeting deeper …]` with a text reply instead of
  `meeting_brief` (its compaction head had paraphrased the prompt as
  "respond immediately to direct questions"). In run 1 the same model did
  call `meeting_brief` for the typed request and after the tap. The actor's
  source inference for `Request`/`Deeper` is code-path identical to `Typed`
  (exercised) and unit-free; not observed on the wire.
- PTT while the meeting app is playing through speakers: the mic track
  carries the far end (01 §9 check 5's AEC residual), so the first held
  span in run 4 was 45 s of mangled far-end text. Headphones, or the island
  muting the far end, are the real fix; not this phase's.
- CPU per track and the between-sentence drop (check 2 / 2b) as numbers;
  a < 160 ms burst not opening an utterance (only the `Gate` unit tests).
- Topic TTL expiry on the clock; `Pause`/`Resume` with STT running (the
  flag path is in `stt::run`; only the capture-side pause was exercised in
  phase 1); the STT panic/`Err` → `SttDown` path; a `live_model_dir` without
  a checkpoint (the load `Err` → `SttDown(Mic)`, `SttDown(System)` branch).
- `ListQueued > 3` deferral (`Batcher::defer`): unit-tested; the session
  never had more than two steers queued.
- The dictation actor end to end on the shared `Utterer` (`cargo test -p
  voice` and `cargo check` only; 54's `Gate` tests still pass).

### Deviations from the design above

1. **`Utterer::append` concatenates verbatim (was: one space between
   pieces).** `dictation::Task::append` inserted a space between two
   non-whitespace pieces; SentencePiece already marks word starts with a
   leading space (`▁` → `" "`, `parakeet_rs::Nemotron::decode_single`), so a
   window boundary inside a word gave `"image t ags"` (the smoke test caught
   it). Dictation inherits the fix.
2. **A new utterance drops a leading punctuation run.** The flush window
   does not always pull the last `.`/`?` out of the decoder; it arrived at
   the start of the next utterance (`". The Pulumi …"`, `"? What is the …"`).
   `Utterer::piece` strips `.,?!;:` from the front. Cosmetic; the previous
   utterance loses its final mark.
3. **`meeting_topic` arguments are flat: `{label, title, bullets}`**, not
   `{label, brief: {title, bullets}}` (§5.1). The python runtime renders a
   nested struct as `brief: Any`; the model guessed a list and spent three
   retries (run 1). Flat, the signature reads
   `meeting_topic(label: str, title: str, bullets: list[str])`.
4. **Both live tools are `DIRECT`** (08 §0.4: their reply is an
   acknowledgement, not data). Without it the model called them as direct
   tools, got `not a direct tool here; call it from python` on every try
   and gave up (runs 1–3: 12 refused calls, 4 registered topics; run 4:
   4 topics in the first 25 s).
5. **Push-to-talk tags the whole held span, not the utterance open at
   release.** Mic finals that close while `held` are deferred; the release
   force-closes what is open and reports one `Utterance{request: true}` with
   the joined text (the 1.5 s post-release grace is gone). §2's rule missed
   the common case — the user stops, the VAD closes 300 ms later, the key
   lifts 500 ms after that: nothing open, the question already batched
   (run 2: `"? What is the"` tagged, the full question not). `Flags.held`
   mirrors `Live.held`; partials still stream while held.
6. **Punctuation-only finals are dropped** (`.` from a breath or the far
   end's echo): `said_something` = one alphanumeric char. `live.jsonl` and
   batches carry no `me: .` lines.
7. **`t0`/`t1` are computed by the actor from receipt `Instant`s**
   (`Piece { at, behind }` → `RecordingClock::elapsed_at(at) - behind`), so
   the STT tasks never hold a clock copy across pauses. `t1` is the close
   time (the stamp of the chunk that closed it), not the silence start.
8. **`meeting::Status` exports as `MeetingStatus.ts`**: `Status.ts` was
   silently overwritten by the todo item state's `Status` (phase 1 emitted it
   into the same file; unnoticed).
9. **Start order**: session → `SetCompaction` → directory → devices → STT
   sessions (both in one `spawn_blocking`, sequential — the `ort` init is
   process-wide once); a load failure degrades `SttDown(Mic)` and
   `SttDown(System)` and the recording proceeds. Tools are registered in
   the `Begun` handler, before `Started` and the `started.` line.
10. **Stop order**: `capture.stop()` (the STT tasks share its stop flag and
    flush their open utterance, returning the pieces) → those finals to
    `live.jsonl` and the event stream → `Batcher::drain()` + finals as one
    last input → `SetCompaction{None, None}` → `UnregisterTool` × 2 → `Done`.
    The engines die with the tasks (RSS in check 13).
11. **Brief `source` inference** keeps outstanding prompts for 120 s and
    consumes them on match, rather than "the last steer": a `meeting_brief`
    without `topic`/`parent` after a tap is `Auto` (check 5, as §5.2 says),
    a `Request`/`Typed` prompt is consumed by the first unmatched brief.
12. **`AgentEvent::TurnFinished{stop_reason: "error"}` sets `ModelDown`; a
    later non-error `TurnFinished` removes it from `manifest.degraded`** —
    the one degradation that clears (02 §0.4), so the manifest's `degraded`
    is not a strict union for `ModelDown`.
13. `Batcher::new` counts the `started.` line as the last send; `feed`/`tick`
    return the batch, `defer(batch)` puts a held-back batch in front of new
    pending (the `ListQueued > 3` merge lives in the actor's `send_batch`).
14. Test infrastructure: the smoke sentence is rendered by `say` at test time
    (the repo has no WAV fixture); `voice::live` tests need
    `$ORT_DYLIB_PATH` (or the dylib beside the Nemotron checkpoint) and skip
    otherwise. The agent's pre-existing clippy lints (item 1) are not this
    phase's.
