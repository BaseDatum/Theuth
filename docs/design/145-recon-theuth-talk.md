# recon-ours — theuth talk / voice stack (2026-10-08)

Status: COMPLETE (skeleton filled, 25/25 cites re-opened ✓). Read-only recon of `/Users/Sky.Moore/Develop/theuth`. The brief named main `a109c4fc`; at
read time HEAD was `cb944b75` (committed 2026-10-08 13:56, i.e. main moved while I read — `git
rev-parse` only, nothing checked out). Cites are `path#La-Lb` relative to the repo root and are
lines I read. Legend: **BUILT** = on main; **DOC-NOT-BUILT** = designed only; **OPEN** = unresolved
question in a design's §12/Open or in knowledge.

Source of truth read: `docs/design/76-talk.md` (§0, §3–§8, §12–§13),
`docs/design/77-talk-followups.md` (§0, §1, §4.3), `docs/design/117-speech-tail.md`,
`crates/talk/src/{lib,policy}.rs`, `crates/voice/src/{lib,speak,live,out,stt,vad,mic,model}.rs`
(outlines + key ranges), `crates/rpc/src/api.rs` talk RPCs, `crates/agent/src/{lib,loop_}.rs`
Cancel/TrimAssistant/Abort::Interrupted, `crates/tools/src/lib.rs` ToolCx, desktop `lib/talk.ts`,
`lib/speech.ts`, `routes/talk.$session.tsx`, TUI `crates/tui/src/talk.rs`, knowledge
`facts/theuth-talk-as-built-deviations-2026-09-29`,
`facts/theuth-talk-first-audio-latency-cause-2026-09-29`,
`facts/theuth-stt-model-tiers-recon-2026-09-29` (hint text).

## 1. Architecture bet and layering

**Bet (BUILT): cascaded, not full-duplex.** Audio → `Utterer` (TEN-VAD gate + Nemotron streaming
STT) → the ordinary agent session (model, tools, history, permissions, exactly as typed input) →
committed markdown blocks → sentences → Pocket TTS → speaker. Moshi-style speech-to-speech and
hosted realtime APIs (OpenAI Realtime, Gemini Live) were explicitly rejected: "theuth's middle box
*is* the product", local STT/TTS is a requirement — `docs/design/76-talk.md#L24-L36`.

**Who talks / who works.** There is no separate "voice model". The one session model both talks and
works; the voice side is a *channel plugin*, not an intelligence. Three layers, three owners:
`voice` owns audio devices and inference and "knows nothing about turns"; `talk` owns the state
machine and "knows nothing about codecs or models"; `agent` is unchanged except that `Cancel` can
keep a prefix — `docs/design/76-talk.md#L376-L397`. Dependency order `… ← voice ← talk ← rpc` —
`docs/design/76-talk.md#L51`.

**Why a plugin, not an actor in `voice` or `agent`.** A talk is "many turns, a cancel path into the
agent and a policy layer", the shape of `signal`/`slack`/`meeting`; a fourth actor in `voice` would
make the crate carrying 20 ms frames wait on model turns — `docs/design/76-talk.md#L37-L50`.

**Process ownership (BUILT).** One Rust process; for local frontends the daemon owns the speaker
(`voice::out::Player`, cpal, 24 kHz mono) as design 54 made it own the microphone; the remote
(phone-through-relay) client keeps the Opus `SpeechAudio` leg and pushes PCM in —
`docs/design/76-talk.md#L52-L69`. The `Talk` actor is a single instance with `active: Option<Live>`
— one talk per daemon — `crates/talk/src/lib.rs#L512-L516`, `crates/talk/src/lib.rs#L770-L773`
("already talking").

**Pure policy core.** `policy::Policy::step(input, now) -> Vec<Action>` is a pure function over
`Input`
(Onset/Close/Text/Timer/Delta/BlockEnd/TurnStarted/TurnFinished/SpeechStarted/SpeechFinished/Packet/PlayedSeq)
— `crates/talk/src/policy.rs#L27-L70` — producing `Action`s (State, Heard, Speak, Saying, Submit,
Cancel, Trim, StopSpeech, Pause, Resume, Holding, Commit, Interrupted, Timer, VadThreshold) —
`crates/talk/src/policy.rs#L74-L150`. The actor's `run` loop executes actions in order, and the five
cuts are pushed to the front of the queue so nothing interleaves —
`crates/talk/src/lib.rs#L539-L548`, `crates/talk/src/lib.rs#L696-L704`.

**What the voice side may NOT decide (BUILT by construction).** `talk` never chooses tools, never
edits history beyond `Cancel{keep}`/`TrimAssistant`, never sees permission policy ("No new tools; no
new `Risk`. The session's tool policy, approvals and `Ask`/`Resolve` gate are untouched") —
`docs/design/76-talk.md#L658-L665`. Partials never reach the session ("frontends show them, the
session never sees them") — `docs/design/76-talk.md#L570-L571`. The store never learns about audio —
`docs/design/76-talk.md#L560-L561`.

**Plugin wiring (BUILT).** `TalkPlugin::start` reads the `talk` config section, spawns the actor,
registers `TALK_ACTOR`, subscribes to `AgentEvent` and `VoiceEvent`, registers a lifecycle hook and
a `RegisterPrompt{plugin:"talk"}` block — `crates/talk/src/lib.rs#L1219-L1255`.

## 2. Voice I/O pipeline

**STT (BUILT, local).** Streaming Nemotron (`nemotron-streaming-en`, `STREAMING_MODEL_SUBDIR`) for
talk/dictation/meeting live; Parakeet TDT (`parakeet-tdt`) for voice notes and the meeting
transcript of record — `crates/voice/src/lib.rs#L51-L55`; `stt::Stream(Nemotron)`,
`Engine(Mutex<ParakeetTDT>)`, model rate 16 kHz, window 8 960 samples (560 ms) —
`crates/voice/src/stt.rs#L26-L34`, `crates/voice/src/stt.rs#L170-L187`,
`crates/voice/src/stt.rs#L237-L253`. Knowledge: Nemotron streaming 0.6B int8 ≈ 0.9 GB, 560 ms
window, always loaded; TDT 0.6B v3 1.75 GB (`facts/theuth-stt-model-tiers-recon-2026-09-29`, hint
text).

**VAD + gate (BUILT).** TEN-VAD at 16 kHz, 256-sample frames; `Gate` constants: PREROLL 20, ONSET 10
frames (160 ms), CLOSE 19 (≈300 ms), MAX_OPEN 15 s, TURN 2 s, THRESHOLD 0.5, with `set_threshold` —
`crates/voice/src/vad.rs#L16-L19`, `crates/voice/src/vad.rs#L108-L135`.

**Utterer (BUILT).** One `Utterer` owns the Nemotron session, the VAD, the gate and a `rubato`
resampler and turns any-rate PCM into punctuated `Text{utterance, start, text, is_final}` pieces;
"knows nothing about microphones, clocks or actors" — `crates/voice/src/live.rs#L1-L58`. Talk
*leases* the resident Utterer from the `Dictation` actor rather than loading a second 0.9 GB model —
`crates/talk/src/lib.rs#L782-L786`, design `docs/design/76-talk.md#L166-L171`.

**Mic (BUILT).** cpal capture at 48 kHz, 20 ms chunks, `Aec` enum, `Capture::open_with(want) ->
(Capture, Receiver, Aec)`; macOS VoiceProcessingIO in a `vpio` module —
`crates/voice/src/mic.rs#L22-L29`, `crates/voice/src/mic.rs#L37-L65`,
`crates/voice/src/mic.rs#L218`. Talk maps `AecMode::Prefer → Aec::Vpio`, `Off → Aec::None` —
`crates/talk/src/lib.rs#L790-L799`.

**TTS (BUILT, local).** Pocket TTS via the `ptts` crate (`ptts::synth::Synth`), 24 kHz PCM, 80 ms
Mimi frame = 1 920 samples = one Opus packet at 32 kbit/s (~320 B) —
`crates/voice/src/speak.rs#L1-L23`; a `Synthesizer` trait allows a fake in actor tests —
`crates/voice/src/speak.rs#L28-L45`. Device sink writes PCM straight into the `Player` ring, no
codec — `crates/voice/src/speak.rs#L1-L4`.

**Output (BUILT).** `voice::out`: RATE = 24 kHz, FRAME 1920, BLOCK = 20 ms, DEFAULT_LATENCY 40 ms
when the device does not report, `Ring` with `push/fill/pause/resume/flush/position`, `Played`
positions and a per-utterance `Ledger` — `crates/voice/src/out.rs#L29-L39`,
`crates/voice/src/out.rs#L55-L105`, `crates/voice/src/out.rs#L206-L263`.

**Provider abstraction / fallbacks.** `Sink { Client (Opus over the bus), Device (daemon speaker) }`
— `crates/voice/src/lib.rs#L190-L202`. `Speak{session, text, queue, sink}`, `Stop{id: Option}` (None
clears the queue), `Pause`/`Resume`/`Position` device-only — `crates/voice/src/lib.rs#L204-L245`.
`VoiceStatus` reports model state, ONNX execution provider (`DmlExecutionProvider |
CPUExecutionProvider`), voices, `speaking`, `queued` — `crates/voice/src/lib.rs#L252-L271`. There is
**no cloud STT/TTS provider and no provider trait beyond `Synthesizer`**; "STT tiers" (small/large
checkpoint choice, `voice.stt_tier: auto|small|large`) is **DOC-NOT-BUILT** —
`docs/design/77-talk-followups.md#L40-L43`, `docs/design/77-talk-followups.md#L414-L436`.

**Echo.** Platform AEC (VPIO) on macOS; Windows/Linux run `MicNoAec` + "headphones recommended";
text-domain echo rejection always (classifier §3) — `docs/design/76-talk.md#L84-L102`. Software AEC
(webrtc-audio-processing / speexdsp) is **OPEN** — `docs/design/76-talk.md#L863-L864`.

**Audio formats and framing.** Remote leg: base64 LE mono i16 at the client's declared rate, ~100 ms
per `talk_audio` call, with `played_seq` — `crates/rpc/src/api.rs#L1374-L1384`; remote chunking
`rate/50` (20 ms) — `crates/talk/src/lib.rs#L800-L808`. Client playback: `speech.ts` decodes Opus
(libopus wasm) and schedules packets gapless on one `AudioContext`; one shared output `GainNode` is
the analyser tap — `crates/desktop/ui/src/lib/speech.ts#L1-L35`.

## 3. Turn-taking and duplex

**Turn state machine (BUILT).** `State { Idle, Listening, Thinking, Speaking, Holding, Interrupted
}` — `crates/talk/src/lib.rs#L84-L97`; diagram — `docs/design/76-talk.md#L446-L460`. Only the actor
changes state; the audio task runs the VAD in every state — `docs/design/76-talk.md#L462-L465`.

**Two-stage barge-in (BUILT).** Stage 1 *hold*: gate open ≥ `barge_in_min_ms` (400) → `Pause`
(player silent within one 20 ms callback), queue kept. Stage 2 *decide* at the first STT partial or
at `decide_ms` (900) after onset: backchannel / echo / **noise (no word by the deadline)** →
`Resume`; else real → `Commit` — `docs/design/76-talk.md#L103-L117`,
`docs/design/76-talk.md#L491-L529`; code `crates/talk/src/policy.rs#L662-L702`. The
noise-at-deadline rule is FIX-1 from the aquarium test (steady noise never cuts the agent) —
`docs/design/76-talk.md#L493-L497`. VAD threshold rises to `barge_in_vad` 0.7 while Speaking/Holding
— `crates/talk/src/lib.rs#L140-L142`, applied via `Action::VadThreshold` —
`crates/talk/src/lib.rs#L719`.

**Backchannel list (BUILT).** `yeah yep yes mhm mm mm-hmm uh-huh ok okay right sure hm hmm i see got
it` — `crates/talk/src/lib.rs#L170-L173`. Echo: Levenshtein ratio ≥ `echo_ratio` 0.8 against the
last ~2 s of playing sentences — `crates/talk/src/lib.rs#L151-L153`,
`crates/talk/src/policy.rs#L1013-L1048`.

**The five cuts (BUILT).** `commit(played)`: compute `heard` from the ledger + player position, mark
the rest Dropped, `StopSpeech` (device: `Voice::Stop{None}`; client: publish `Flush` first), then
`Cancel(Some(kept))` if the turn is still running or `Trim{turn, keep}` if it already finished,
publish `Interrupted{heard, dropped, text}`, state → Listening, arm end-of-turn —
`crates/talk/src/policy.rs#L704-L741`; actor side `crates/talk/src/lib.rs#L658-L666`,
`crates/talk/src/lib.rs#L634-L657`. Design order — `docs/design/76-talk.md#L531-L547`.
`Utterer::reset()` is deliberately *not* called: the candidate is the next turn's first utterance —
`docs/design/76-talk.md#L540-L542`.

**Heard prefix (BUILT).** `Ledger` of `Sentence{text, speech, status
Queued|Playing|Played|Dropped}`; `heard(sink, played)`; remote sink uses `played_seq` —
`crates/talk/src/policy.rs#L154-L267`; design `docs/design/76-talk.md#L549-L561`. Whole sentences
only; a `Playing` sentence at flush is Dropped.

**End of turn = policy on top of the gate (BUILT).** Gate Close (300 ms) closes an utterance; the
turn commits after `end_of_turn_ms` 700 or `end_of_turn_punct_ms` 250 when the final ends in `. ?
!`; an onset inside the window reopens the same turn — `docs/design/76-talk.md#L146-L158`,
`crates/talk/src/lib.rs#L143-L146`, `crates/talk/src/lib.rs#L167-L168`. `submit()` joins pending
utterances, computes `N s`, prefixes `[spoken, N s]` or `[spoken, N s, interrupted]`, enters
Thinking — `crates/talk/src/policy.rs#L573-L597`. A learned end-of-turn model (Smart Turn v3,
`turn_model: smart-turn`) is a **DOC-NOT-BUILT** slot ("refused until `<data_dir>/models/smart-turn`
exists") — `crates/talk/src/lib.rs#L119-L127`.

**Speech while Thinking = late addition, not barge-in (BUILT, deviates from doc).** `late_addition`:
`StopSpeech`, `Cancel(None)`, reset the ledger, submit the new utterance on its own (the cancelled
input's user row stays in history, so merging would duplicate) —
`crates/talk/src/policy.rs#L743-L762`. Knowledge records the deviation: §0.9's "merge texts and
resubmit" was dropped, and a Thinking turn is only cancelled once a transcript exists
(`facts/theuth-talk-as-built-deviations-2026-09-29`).

**Input arbitration (BUILT).** Typed turns on the session during a talk are spoken like spoken ones
(`Input::TurnStarted` in Listening → Thinking) — `crates/talk/src/policy.rs#L389-L398`.
`TurnFinished{cancelled|interrupted|error}` from anyone else (Esc in the TUI) drops the queue and
returns to Listening — `crates/talk/src/policy.rs#L885-L911`. `Speak` from the desktop button during
a talk clears the talk's queue — **OPEN** `docs/design/76-talk.md#L861-L862`.

**Response slots / "safe insertion window".** None as a named concept. The only arbitration is the
one FIFO behind `current` in the `Voice` actor and the ledger; the tail of a reply is never cut by
an onset in Listening ("not a barge-in, and the tail is not cut") —
`docs/design/76-talk.md#L582-L585`.

**Half-duplex rejected.** "mute the mic while speaking — that is the agent that talks through a real
cut-in" — `docs/design/76-talk.md#L101-L102`.

## 4. Nonblocking work and background tasks

**How a spoken request becomes work (BUILT).** `Action::Submit` → `agent::SendInput{session, text,
delivery: NextTurn}` — `crates/talk/src/lib.rs#L599-L613`. From there it is an ordinary agent turn:
tools, Monty runtime, subagents, jobs. Talk adds nothing.

**Tool batches during a talk (BUILT, by inheritance).** Tool calls are read from the `StreamEnd` and
dispatched after the whole stream, so a barge-in during pre-tool narration cancels the batch before
any tool runs — `docs/design/76-talk.md#L133-L137`. `ToolCallPending` is `Input::BlockEnd` for the
policy (speak the prose so far) — `crates/talk/src/lib.rs#L1033-L1035`; the state stays Speaking
while the queue drains, then Thinking until the next block — `docs/design/76-talk.md#L585-L587`.

**Cancellation semantics.** *Confirmed* at the agent level: `Cancel{session, keep, scope}` where
`CancelScope::Root` (default) leaves running descendants working and `Tree` cancels them (design 133
§2) — `crates/agent/src/lib.rs#L305-L337`. Talk always sends `scope: Root` —
`crates/talk/src/lib.rs#L634-L641`. *Not confirmable for tool bodies*: `ToolCx { session, call_id,
quiet, resources }` has **no cancellation token** — `crates/tools/src/lib.rs#L294-L306`; a cancelled
turn's tool body runs to completion and the registry writes a synthetic cancelled result ("Caller
gives up: synthetic cancellation upstream, real work continues") —
`crates/tools/src/registry.rs#L1915`. **OPEN** since 76 §12 — `docs/design/76-talk.md#L853-L858`.

**Background queueing per owner / status queries / delegation.** Nothing talk-specific. The
substrate's jobs (`shell` → `{job, log}` + `<job_result>`), PTY jobs, `remind_me_in`, subagents and
tasks exist for every session, voice or not, but none is wired to the talk state machine (no `Input`
for a job result, a reminder or a child message — `crates/talk/src/policy.rs#L27-L70` is the
complete input set). What the user hears while a long tool call runs is the pre-tool narration
sentence the prompt asks for — `crates/talk/src/lib.rs#L411-L412`; the UI shows `Working…` from the
session's `activity` because the talk state does not distinguish it —
`crates/desktop/ui/src/lib/talk.ts#L26-L44`.

**Filler acks (DOC-NOT-BUILT).** Option B (neutral filler clip without a classifier, pre-rendered
clip cache, phrase decks) and option C (filler chosen by the `laya` classifier, gated on a ≥150-turn
shadow benchmark) are documented in 77 and not built — `docs/design/77-talk-followups.md#L33-L39`.

## 5. Result delivery back into the conversation

**Speaking the reply (BUILT).** `talk` subscribes to
`AgentEvent::TextDelta/ToolCallPending/TurnStarted/TurnFinished` for its session —
`crates/talk/src/lib.rs#L1025-L1051`; per turn one `markdown::Text`; committed blocks →
`markdown::spoken` → sentences → `Speak{queue: true}` each with its own `SpeechId` —
`docs/design/76-talk.md#L576-L589`, `crates/talk/src/lib.rs#L564-L588`. **Sentence streaming
(TALK-STREAM, BUILT):** the open prose block is split at sentence boundaries while it streams;
markup-holding sentences wait for the close; headings/lists/code/tables commit whole —
`docs/design/76-talk.md#L591-L614`; `speak_committed`/`speak_open` on every delta —
`crates/talk/src/policy.rs#L382-L386`.

**Announcement queue.** The `Voice` actor's FIFO behind `current` (`queued: Vec<SpeechId>` in
`VoiceStatus`) — `crates/voice/src/lib.rs#L266-L270`. One queue per daemon; there is no priority, no
per-owner lane, no "later" bucket.

**Playback receipts (BUILT).** Device sink: the player's sample ledger + `Position`/`Pause` →
`Played` — `crates/voice/src/out.rs#L206-L258`. Client sink: `played_seq` back on every `talk_audio`
call ("the highest `SpeechAudio.seq` whose playback ended on the client since the last call — the
heard-prefix position") — `crates/rpc/src/api.rs#L1374-L1384`; `Input::Packet`/`Input::PlayedSeq`
feed the ledger — `crates/talk/src/policy.rs#L427-L432`. `SpeechStarted` → `Saying` subtitle;
`SpeechFinished{done}` → Played or Dropped — `crates/talk/src/policy.rs#L400-L426`.

**Claims across clients.** One talk per daemon, bound to the frontend connection (`peer`) that
started it; ends on stop, connection close, or `CloseTree` — `docs/design/76-talk.md#L166-L177`,
`crates/talk/src/lib.rs#L1085-L1097`. Other clients of the same owner see `TalkEvent`s and the
sidebar live dot but do not get the audio (device sink publishes no `SpeechAudio`) —
`docs/design/76-talk.md#L485-L489`. `speech.ts` claims only `sink == client` packets (deviation note
in `facts/theuth-talk-as-built-deviations-2026-09-29`).

**Context injection / no-replay-after-interruption (BUILT).** History keeps only the heard prefix:
`Abort::Interrupted{text}` persists `ChatMessage::assistant(text)` and stops with `stop_reason:
"interrupted"`, no recovery note — `crates/agent/src/loop_.rs#L5517-L5525`; post-turn case
`TrimAssistant{session, turn, keep}` replaces the last assistant message of the turn —
`crates/agent/src/lib.rs#L278-L288`. Deltas arriving after our own Cancel are ignored
(`!self.cancelled`) — `crates/talk/src/policy.rs#L381-L387`. The prompt tells the model "your
history keeps only the sentences they actually heard … do not repeat the rest" —
`crates/talk/src/lib.rs#L413-L416`.

**What is NOT delivered by voice (gap, see §19).** A finished background job (`<job_result>`), a
reminder, a child/subagent final message or a peer `<message>` lands in the session as an *input*
that starts a turn; if a talk is live, that turn's reply is spoken (typed/other turns are spoken,
`crates/talk/src/policy.rs#L389-L398`) — but there is no announcement etiquette (no "may I
interrupt", no deferral while the user speaks beyond the ordinary Listening onset rule), and I found
no code that gates such a turn on the talk state.

## 6. Presence, progress and animation

**Orb (BUILT).** Three looks for six states: `Listening/Idle/Interrupted → idle`, `Thinking →
thinking`, `Speaking/Holding → speaking` (Holding keeps the speaking look — paused, not stopped) —
design `docs/design/76-talk.md#L206-L217`; desktop `orbState` —
`crates/desktop/ui/src/lib/talk.ts#L19-L24`; TUI `Look {Idle, Thinking, Speaking}` with the same
mapping — `crates/tui/src/talk.rs#L11-L29`. Desktop orb = shadcn `orb-16` (`components/orbs/*`,
490-line renderer, GPU variant).

**Phase line (BUILT, TALK-UI-A).** `Listening` / `Thinking…` / `Working…` (a tool call, taken from
the session's `activity`) / `Speaking`; `Holding` shows `Speaking` —
`crates/desktop/ui/src/lib/talk.ts#L26-L44`. Hint under the orb: "Tap the orb to speak / stop
listening / stop" — `crates/desktop/ui/src/lib/talk.ts#L46-L49`.

**Subtitles (BUILT).** Agent line from `Saying` (follows *this client's playback* when it plays the
audio itself; a cut sentence is held until `Interrupted`/next state), user line from
`Heard{final:false}` — `crates/desktop/ui/src/lib/talk.ts#L51-L60`; events `Heard`, `Saying`,
`Submitted` — `crates/talk/src/lib.rs#L323-L353`.

**Level / waveform (BUILT).** `TalkEvent::TalkLevel{rms: [f32; 8]}` — RMS per 20 ms of the last 160
ms, ~6 events/s, renamed from `Level` to avoid the `MeetingEvent::Level` tag collision —
`crates/talk/src/lib.rs#L315-L322`; the electric-waveform band under the orb shows the mic while
listening and the agent's own voice while speaking via the shared output `GainNode` —
`docs/design/76-talk.md#L902-L904`, `crates/desktop/ui/src/lib/speech.ts#L27-L35`.

**Audible cues.** A turn chime (`chime` from `@/lib/cue`) is imported by `talk.ts` —
`crates/desktop/ui/src/lib/talk.ts#L7` (TALK-UI-A "phase line + turn chime", knowledge project
record). No spoken progress, no filler, no "thinking" sound beyond the chime.

**What is hidden.** Tool names, arguments, results, subagent activity, approvals: none is spoken or
projected on the orb beyond `Working…`. The transcript pane (collapsed by default, remembered per
session) shows them as in any chat — `crates/desktop/ui/src/routes/talk.$session.tsx#L1-L3`,
`#L30-L39`.

## 7. Latency

**Budget (design, targets).** User stops → Gate Close 300 ms; Close → final text ≈ 560 ms window +
~130 ms inference; end-of-turn wait 250/700 ms; model time-to-first-sentence 0.5–1.5 s
(provider-bound, "the one segment this doc cannot shrink"); first sentence → first audio ~200 ms +
80 ms; **speech end → agent audible ≈ 1.9–3.3 s**; onset → hold 400 ms + ≤ 20 ms + device latency;
onset → decision ≤ 900 ms; barge-in commit → silent ≤ 20 ms + device latency —
`docs/design/76-talk.md#L752-L768`.

**Measured / claimed.** Knowledge: Pocket TTS ≈ 85 ms to first audio on a release build, 7–15 s on a
debug daemon; the 11:05 log showed three `Speak`s released together at `TurnFinished` — the
first-audio problem was the committed-block policy, fixed by TALK-STREAM
(`facts/theuth-talk-first-audio-latency-cause-2026-09-29`). Sentences synthesise serially — no
prefetch of N+1 while N plays (same fact). STT 128–136 ms inference per window (76 §8 row 2 cites
knowledge).

**Instrumentation (BUILT).** `info` logs under target `talk`: "first sentence spoken after {ms}
(turn started → first Speak)" and "first audio after {ms}" measured from `TurnStarted` —
`crates/talk/src/lib.rs#L564-L567`, `crates/talk/src/lib.rs#L589-L594`; `RUST_LOG=talk=debug` prints
every action — `crates/talk/src/lib.rs#L546-L547`; `holding`/`resumed`/`interrupted` at `info` —
`crates/talk/src/lib.rs#L682`, `#L710`.

**Where time goes (as understood).** 560 ms Nemotron window pad + end-of-turn wait + provider TTFT
dominate; the local half (hold/flush) is bounded by the 20 ms callback. **DOC-NOT-BUILT** levers:
filler acks B/C (77 §2–§3), speculative generation (76 §11/§12 —
`docs/design/76-talk.md#L859-L860`), a smaller STT tier (Parakeet EOU 120M, 77 §4). Nothing
pipelines TTS sentence N+1 behind N today (knowledge; not re-verified in `speak.rs` this pass).

**iOS tail (BUILT, design 117).** `TAIL_MS = 400` grace before suspending the AudioContext so the
rendered tail is not stranded; a context iOS left non-running is cut before `unlock` resumes it —
`docs/design/117-speech-tail.md#L24-L28`, `crates/desktop/ui/src/lib/speech.ts#L4-L5`.

## 8. Prompt and tool contract for the voice model

**Fixed prompt block (BUILT).** One `RegisterPrompt{plugin:"talk"}` block, present whenever
`talk.enabled` — `crates/talk/src/lib.rs#L1246-L1251`. Its text: you are talking out loud; people in
the room hear you; "say hello to my friend" means just say it; do not DM or look up contacts unless
asked ("a voice in the room, not a messenger"); `[spoken, N s]` is a live transcript, read through
slips, never quote them back; write for the ear — short sentences, no markdown/lists/URLs/paths; one
short sentence before a tool batch "that sentence is … their chance to stop you"; you may be cut off
and then your history keeps only the heard sentences; keep the style while the marker is present —
`crates/talk/src/lib.rs#L399-L416`. Design rationale and why `[voice note]` was not reused —
`docs/design/76-talk.md#L178-L187`.

**Prompt ownership.** Prompt blocks must be a pure function of config (07 §4); registered by the
plugin, not by a frontend — `docs/design/76-talk.md#L663-L665`. The block has a unit test that it
says "heard aloud" and "not a messenger" — `crates/talk/src/lib.rs#L1321-L1325`.

**Tools.** The full session toolset is available unchanged; no tool is gated or added for voice —
`docs/design/76-talk.md#L658-L661`. There is **no separate voice-side model** and therefore no
"optional tools for the voice model" question: one model, full tool access, Monty runtime.

**Marker contract (BUILT).** `[spoken, N s]` / `[spoken, N s, interrupted]`, N = ceil(seconds from
first onset to last close), min 1 — `crates/talk/src/policy.rs#L577-L586`. The desktop renders the
marker as a card ("spoken · N s", plus "cut in") — `docs/design/76-talk.md#L682-L687`.

**Boundary tests.** None exercising the *model's* adherence (no eval of "did it stay short", no
replay corpus). Policy tests cover the state machine, not the prompt.

## 9. Permissions and confirmations over voice

**BUILT:** nothing voice-specific. "an approval prompt during a talk is answered on the frontend as
today; the agent's spoken narration before the batch is what the user hears while it waits" —
`docs/design/76-talk.md#L661-L663`. The talk state machine has no `Input` for an `Ask`/`Resolve`
event — `crates/talk/src/policy.rs#L27-L70`. Consequences: during an approval the orb sits in
Thinking/Speaking with `Working…` (from `activity`, `crates/desktop/ui/src/lib/talk.ts#L31-L38`),
nothing is spoken about it, and the user cannot say "yes" — the approval card must be tapped.
Secrets: secret placeholders (the vault's double-brace syntax) and the vault are untouched by talk;
the model is never given the value, so there is nothing to redact in speech beyond the general rule.

**DOC-NOT-BUILT / OPEN:** I found no design that proposes spoken approvals, spoken ids for pending
approvals, or a redaction rule for speech. The "double" (owner's stand-in for approvals, `double`
skill) is a separate mechanism and not voice-aware as far as this pass saw (not read in depth).

## 10. Memory, personalization, notes

**Per-user persistence (BUILT, substrate).** Session history is the ordinary store; a talk leaves
`[spoken, N s]` user rows and assistant rows (truncated to the heard prefix after a cut) —
`crates/agent/src/loop_.rs#L5517-L5525`. Knowledge base (`knowledge_*`), notes (`doc_*`), meetings
and briefs are available to the session model as to any session — nothing voice-specific is
injected. The talk prompt block is the only voice-specific injection —
`crates/talk/src/lib.rs#L399-L416`.

**Voice settings (BUILT).** `voice.voice` (the TTS voice), voices list from the checkpoint —
`crates/voice/src/lib.rs#L262-L265`; model dir discovery (`WEIGHTS`, `TOKENIZERS`, `voices/`) —
`crates/voice/src/model.rs#L10-L37`. Desktop `lib/talk-settings.ts` (20 lines, exists; not read).

**Authority levels.** None for voice. There is no speaker identification in talk (diarisation lives
in the meeting crate's Sortformer path, not leased by talk), so "who is speaking" is not known and
every utterance is the owner's.

**Not built:** per-user voice profiles, speaker verification, spoken-preference memory ("talk
slower"), a per-talk summary note.

## 11. Session, journal and recovery

**What persists.** Only the agent store rows; the talk itself has no journal. `TalkEvent` is a live
bus event
(`TalkStarted/TalkState/TalkLevel/Heard/Saying/Submitted/Holding/Resumed/Interrupted/Flush/Degraded/TalkFinished`)
— `crates/talk/src/lib.rs#L294-L396`; nothing writes them to disk.

**Restart semantics (BUILT).** The talk ends when the frontend says stop, when the starting
connection closes, or when the session closes (`CloseTree`) — `docs/design/76-talk.md#L171-L174`,
`crates/talk/src/lib.rs#L1085-L1097`; `EndReason { Stopped, Disconnected, SessionClosed,
Error{message} }` — `crates/talk/src/lib.rs#L276-L288`. After an engine restart the talk is gone
(actor state is in memory, `active: Option<Live>` — `crates/talk/src/lib.rs#L512-L516`); the user
must tap the orb again. The history is intact and the next spoken turn resumes the same session.

**Failure paths (BUILT).** `send_input` failure → back to Listening via `TurnFinished("error")` —
`crates/talk/src/lib.rs#L624-L632`; `Speak` failure → `policy.failed(index)` —
`crates/talk/src/lib.rs#L582-L586`; lease returned when the mic fails to open —
`crates/talk/src/lib.rs#L810-L816`; `Dictation::Return(Option<Utterer>)` frees the lease when the
model dies (deviation fact). Partial model-stream failure is the agent's salvage (`Abort::Partial`,
`max_salvages`) — `crates/agent/src/loop_.rs#L5526-L5537`.

**Reconnect/backoff.** Not talk's: the relay/engines layer reconnects the UI; the talk does not
survive its peer's disconnect (`Disconnected`). Mid-utterance recovery after a crash: none (no
journal of audio or partials).

## 12. Clients and transport

**RPC (BUILT).** `talk_start(session, sink, sample_rate?) -> TalkId`, `talk_stop(id)`,
`talk_audio(id, pcm_b64, played_seq?)`, `talk_status() -> TalkState`, all `with_extensions` for peer
identity; a relay peer must pass `sink: "client"` + rate — `crates/rpc/src/api.rs#L1357-L1388`.
Events ride `subscribe_events` as `TalkEvent` (`#[serde(tag="type")]`, variant-name tags).

**Clients (BUILT).** Desktop/web route `/talk/$session` (orb centre, subtitles, collapsed transcript
left, session panels right, phone = transcript sheet) —
`crates/desktop/ui/src/routes/talk.$session.tsx#L1-L3`; `lib/talk.ts` keeps the one talk's state
and, for a relay-linked client, streams the mic in and plays the Opus leg via `speech.ts` —
`crates/desktop/ui/src/lib/talk.ts#L1-L3`. TUI `/talk` pane with an ASCII orb, pure reducer in
`crates/tui/src/talk.rs#L1-L9`. Mobile parity is by design: same route on a phone through the relay
= the remote client — `docs/design/76-talk.md#L222-L226`.

**Local vs remote.** Local engine: the engine captures natively (daemon mic + speaker); the UI only
receives `TalkLevel` packets. Remote engine: the UI opens a mic tap and pushes PCM (`talk.ts: remote
? openTap('talk')`, knowledge `facts/theuth-desktop-voice-ui-seams-2026-10-03`). Under the mesh,
devices are the attached engine's and session writes are forwarded; audio never crosses the mesh —
`docs/design/76-talk.md#L174-L177`.

**Multiple clients on one owner.** One talk per daemon (`already talking`) —
`crates/rpc/src/api.rs#L1361-L1362`; any authenticated peer may stop it —
`crates/rpc/src/api.rs#L1370-L1373`. No hand-off of a running talk between devices.

**Protocol detail.** JSON-RPC over the existing connection; PCM as base64 i16 in ~100 ms calls (no
WebRTC, no binary frames); TTS as Opus packets in `SpeechAudio` events. Relay-peer refusal tests
exist — `crates/rpc/src/api.rs#L7785-L7817`.

## 13. Wake word, sleep and always-on

**BUILT: none.** No wake word, hotword or sleep controller anywhere in `docs/design` (grep for "wake
word|wake-word|hotword": no matches). The talk starts and stops on an explicit gesture (orb tap,
`/talk`, Ctrl+T) — `docs/design/76-talk.md#L188-L201`, `docs/design/76-talk.md#L689-L692`. "Idle"
behaviour: Listening keeps the VAD running continuously with the Utterer leased, the gate deciding
utterances — `docs/design/76-talk.md#L462-L465`; cost = the resident 0.9 GB Nemotron session plus
the mic; there is no low-power or sleep tier.

**Related but different:** `70-as-built-WAKE-LIVE-CHILD.md` / `STALE-WAKE-DESKS` are about *desk
wakes* (agent scheduling), not audio. `scratch-always-on-recon-theuth.md` is a 2026-09-23 fact sheet
on always-on *agents* (routines/desks), not microphones —
`docs/design/scratch-always-on-recon-theuth.md#L1-L14`.

**Note:** `facts/theuth-voice-features-under-the-mesh-2026-09-25` is cited by 76 §0.10 for the mesh
rule (not re-read).

## 14. Vision / multimodal input

**BUILT: none in talk.** `SendInput{…, files: Vec::new()}` — talk never attaches files or frames —
`crates/talk/src/lib.rs#L602-L608`. Images reach the model only through the ordinary chat composer /
`computer_screenshot` / attachments; there is no camera path, no frame buffer, no bounded image ring
in the voice stack. Nothing in 76/77/117 mentions vision.

## 15. Reminders, scheduling and proactive speech

**Substrate (BUILT, not voice-aware).** `remind_me_in` → `<reminder>` input; schedules/routines →
runs delivered to chat channels; jobs → `<job_result>`. Each is an *input to a session*, so if a
talk is live on that session the reply it provokes is spoken (any `TurnStarted` on the session while
Listening → Thinking — `crates/talk/src/policy.rs#L389-L398`). Nothing speaks *unprompted on a
different session*, and nothing proactively starts a talk.

**Not built:** an "announce" path (speak a job/reminder/child result without a model turn), a
do-not-disturb rule while the user is mid-utterance, a "later" queue, or routines that address the
speaker. Filler/progress speech while tools run is DOC-NOT-BUILT (77 B/C).

## 16. Testing and evaluation

**Unit tests (BUILT).** Policy test module `crates/talk/src/policy.rs#L1051-L1918` (names from the
outline): `end_of_turn_timing_with_and_without_punctuation`, `classifier_backchannel_echo_real`,
`heard_prefix_from_ledger_and_position`, `barge_in_commits_the_five_cuts_and_marks_the_next_input`,
`backchannel_resumes_and_its_final_is_not_text`,
`open_and_wordless_at_the_deadline_is_noise_and_needs_a_new_onset`,
`vad_threshold_follows_the_state`, `hold_none_keeps_playing_until_the_decision`,
`speech_while_thinking_cancels_and_submits_the_new_text`, `state_transitions_of_4_2`,
`a_paragraph_is_spoken_sentence_by_sentence_as_it_streams`,
`three_char_deltas_release_each_sentence_at_its_boundary`,
`abbreviations_and_decimals_do_not_split`, `blocks_that_are_not_prose_are_not_streamed`,
`barge_in_mid_stream_keeps_heard_sentences…` (≈ 20 test fns). Actor tests: `wire_shape`,
`config_defaults`, `prompt_says_it_is_heard_aloud_and_not_a_messenger` —
`crates/talk/src/lib.rs#L1259-L1326`. Voice: a `Synthesizer` fake so the queue is testable without
weights — `crates/voice/src/speak.rs#L28-L34`. Desktop: `lib/talk.test.ts` (184 lines, vitest, mocks
speech/mic-tap/engines) — `crates/desktop/ui/src/lib/talk.test.ts#L1-L12`; `speech.test.ts` per 117
— `docs/design/117-speech-tail.md#L3`. RPC relay-refusal tests —
`crates/rpc/src/api.rs#L7785-L7817`. A `talk-sim.ts` scripted source exists for the desktop (180
lines) and the TUI has "the scripted source in `lib.rs`" — `crates/tui/src/talk.rs#L3`.

**Eval harness / corpus:** none. 76 §13 is explicit: "Not exercised: audio by ear, a non-empty heard
prefix …, the backchannel/echo corpus (§10 step 7), a phone through the relay, Windows/Linux" —
`docs/design/76-talk.md#L892-L895`; the deviations fact adds "all smokes used `say`, nothing
verified by ear". Sky's live acceptance (aquarium test, mobile+desktop) is recorded in knowledge,
not in CI. **CI measures:** cargo tests + vitest counts (wave records), no latency or WER numbers.

## 17. Numbers

- **LOC (git ls-files, wc -l):** `crates/talk` 3 276 (lib.rs 1 326, policy.rs 1 918, Cargo.toml 32);
  `crates/voice` 4 518 (lib.rs 1 220, dictation.rs 729, out.rs 659, stt.rs 481, live.rs 361, mic.rs
  307, vad.rs 270, speak.rs 181, model.rs 102, clip.rs 62, examples/audio_out.rs 98, Cargo.toml 48).
  Desktop talk UI: talk.ts 327, talk.test.ts 184, talk-sim.ts 180, talk-settings.ts 20, speech.ts
  513, routes/talk.$session.tsx 243, components/talk/{Orb 38, Subtitles 60}, orbs/* ≈ 1 026. TUI
  talk.rs 565. Total talk-specific ≈ 6.4 k lines plus `voice`.
- **Design text:** 76-talk.md 904 lines; 77-talk-followups.md 528; 117-speech-tail.md 48; eight
  76-as-built docs; 43, 45, 53, 81, 83, 89 as-builts touch voice.
- **Constants:** barge_in_min 400 ms, decide 900 ms, barge_in_vad 0.7, end_of_turn 700/250 ms,
  echo_ratio 0.8 — `crates/talk/src/lib.rs#L161-L181`; VAD frame 256 @16 kHz, onset 160 ms, close
  ≈300 ms, max open 15 s, turn 2 s — `crates/voice/src/vad.rs#L108-L118`; TTS 24 kHz, 80 ms frames,
  Opus 32 kbit/s — `crates/voice/src/speak.rs#L17-L23`; mic 48 kHz / 20 ms —
  `crates/voice/src/mic.rs#L22-L24`; STT window 8 960 samples (560 ms) —
  `crates/voice/src/stt.rs#L34`; player default latency 40 ms, ring keep 2 s —
  `crates/voice/src/out.rs#L35-L39`.
- **Models:** Nemotron streaming 0.6B int8 ≈ 0.9 GB; Parakeet TDT 0.6B v3 1.75 GB; candidates
  Parakeet EOU 120M (WER 9.3 %) / 1120 ms Nemotron export (6.94 %) — knowledge
  `facts/theuth-stt-model-tiers-recon-2026-09-29` (hint text). Voice stack peak RAM ~4–6 GB —
  `docs/design/77-talk-followups.md#L417-L419`.
- **Latency claims:** speech end → audible 1.9–3.3 s budget — `docs/design/76-talk.md#L761`; TTS ≈
  85 ms first audio (release) — knowledge.
- **Cadence:** design 76 written 2026-09-28, shipped 2026-09-29 (squash 4d11125a), same-day
  follow-ups TALK-STREAM/UI-A/UI-B (561fa6c0), waveform 82/83 on 10-03, speech tail 117 on 10-06 —
  `docs/design/76-talk.md#L880-L904`, knowledge project record. Stars/forks/contributors: private
  repo, n/a.

## 18. What the live designs already plan

| Axis | Promise | Status |
|---|---|---|
| 1 Architecture | cascaded, `talk` plugin, three owners | BUILT (76 §0.1–0.2, §3) |
| 2 I/O | VPIO AEC on macOS; software AEC spike for Win/Linux (76 §10 step 9) | AEC BUILT; spike OPEN — `docs/design/76-talk.md#L863-L864` |
| 2 I/O | STT tiers `voice.stt_tier` + `meeting.stt_tier`, optional downloads, Settings › Voice | DOC-NOT-BUILT, pending a 1.5 h EOU-120M benchmark — `docs/design/77-talk-followups.md#L40-L43`, `#L414-L436` |
| 2 I/O | one Nemotron session for both meeting streams (77 §4.4) | DOC-NOT-BUILT |
| 3 Turn-taking | two-stage barge-in, backchannel/echo/noise, heard prefix | BUILT |
| 3 Turn-taking | Smart Turn v3 end-of-turn model behind `turn_model` | slot BUILT, model DOC-NOT-BUILT — `crates/talk/src/lib.rs#L119-L127` |
| 3 Turn-taking | speculative generation after end-of-turn numbers | OPEN — `docs/design/76-talk.md#L859-L860` |
| 4 Background | `ToolCx` cancel token honoured by shell/python/MCP | OPEN — `docs/design/76-talk.md#L853-L858` |
| 4/7 | filler acks B (neutral clip) and C (laya classifier) | DOC-NOT-BUILT — `docs/design/77-talk-followups.md#L33-L39` |
| 5 Delivery | sentence streaming of the open block | BUILT (TALK-STREAM) — `docs/design/76-talk.md#L591-L614` |
| 5 Delivery | post-turn barge-in → `TrimAssistant` | BUILT — `crates/agent/src/lib.rs#L278-L288` |
| 6 Presence | orb, phase line, chime, hint, waveform band, TTS analyser tap | BUILT (76 §0.12, TALK-UI-A/B, 82, 83) |
| 7 Latency | first-sentence/first-audio logs | BUILT — `crates/talk/src/lib.rs#L564-L567` |
| 7 Latency | iOS tail grace 400 ms | BUILT (117) |
| 9 Permissions | approvals answered on the frontend as today | BUILT-as-nothing; no spoken-approval design exists |
| 12 Clients | desktop, TUI, phone via relay; one talk per daemon | BUILT; multi-device hand-off not designed |
| 13 Wake | — | nothing designed |
| 14 Vision | — | nothing designed |
| 15 Proactive | — | nothing designed for voice |
| 3/12 | talk during a recorded meeting (shared Utterer lease) | refused in phase 1, OPEN — `docs/design/76-talk.md#L865-L867` |
| 3 | `Speak` button during a talk clears the queue | OPEN — `docs/design/76-talk.md#L861-L862` |

## 19. Gaps

Concrete things a voice-agent runtime is expected to have that theuth lacks or has only as a plan:

1. **No voice during a long tool call.** The user hears exactly one narration sentence (prompt rule,
   `crates/talk/src/lib.rs#L411-L412`), then silence; the orb says `Working…` from `activity`
   (`crates/desktop/ui/src/lib/talk.ts#L31-L38`). No periodic progress, no filler (77 B/C
   DOC-NOT-BUILT), no "still working" after N seconds.
2. **The user can keep talking, but only to cancel.** Speech while Thinking is a late addition that
   **cancels** the running turn (`late_addition`, `crates/talk/src/policy.rs#L743-L762`) — there is
   no way to add a side remark, answer a question the agent asked mid-work, or queue a second
   request without killing the first. And the cancel cannot stop a running tool body (`ToolCx` has
   no token, `crates/tools/src/lib.rs#L294-L306`).
3. **No unprompted speech path.** A finished job, reminder, schedule run, or child result becomes a
   session input that triggers a model turn; if a talk is live that turn is spoken
   (`crates/talk/src/policy.rs#L389-L398`) but with no etiquette: no check that the user is
   mid-utterance beyond the ordinary gate, no "may I interrupt", no deferral queue, no announcement
   without a model round-trip. On a session without a live talk nothing is ever spoken.
4. **Child/subagent results reach the speaking turn only as text inputs.** `<message from=…>` and a
   child's final message are ordinary inputs; the policy's `Input` enum has no notion of them
   (`crates/talk/src/policy.rs#L27-L70`). Whether they are read aloud depends on the model's reply,
   and the whole reply is spoken including anything verbose the child wrote.
5. **Permission prompts are not voice-reachable.** Approvals are answered on the frontend card
   (`docs/design/76-talk.md#L661-L663`); nothing is spoken, no spoken "yes/no", no spoken id; a
   phone user with the screen off hears silence while an approval waits.
6. **No recovery after an engine restart mid-talk.** Actor state is in memory
   (`crates/talk/src/lib.rs#L512-L516`); the talk ends and must be restarted by hand; no journal of
   partials or audio; the heard-prefix truncation only runs if the cancel landed in the stream.
7. **One talk per daemon, bound to one connection.** No multi-device hand-off, no second listener,
   no talk on two sessions — `crates/rpc/src/api.rs#L1361-L1362`.
8. **No wake word / sleep / always-on tier** (axis 13) and **no vision** (axis 14).
9. **No AEC off macOS** — Windows/Linux are `MicNoAec` + text echo rejection + "headphones
   recommended" (`docs/design/76-talk.md#L90-L93`); the software AEC spike is OPEN.
10. **No eval corpus or CI measurement** of barge-in precision, end-of-turn accuracy, WER or
    latency; 76 §13 lists what was never exercised (`docs/design/76-talk.md#L892-L895`).
11. **Serial TTS** — no prefetch of sentence N+1 while N plays (knowledge; not re-verified in
    `speak.rs`).
12. **No speaker identity** in talk (every utterance is the owner); meetings have diarisation, talk
    does not lease it.
13. **STT is one fixed large streaming model**; smaller/faster tiers are DOC-NOT-BUILT (77 §4).
14. **Learned turn-taking** (Smart Turn) and **speculative generation** are slots/OPEN, not built.

## 20. Strengths

1. **Fully local STT + TTS + VAD + AEC**: Nemotron streaming, Parakeet TDT, TEN-VAD, Pocket TTS,
   VPIO — audio never leaves the machine (`docs/design/76-talk.md#L24-L36`,
   `crates/voice/src/stt.rs#L26-L34`, `crates/voice/src/speak.rs#L1-L23`). Works offline except for
   the LLM provider; under the mesh audio never crosses the network
   (`docs/design/76-talk.md#L174-L177`).
2. **One model with full tool access** — the voice is a channel onto the same session that has the
   Monty runtime, every MCP server, subagents, jobs, knowledge, notes, meetings, schedules; no
   "voice model vs worker model" split and no tool-availability gating
   (`docs/design/76-talk.md#L658-L661`). A cloud speech-to-speech runtime cannot read the user's
   session history or run their tools.
3. **Honest history after interruption** — `Cancel{keep}` / `Abort::Interrupted` / `TrimAssistant`
   persist exactly the heard sentences and nothing else (`crates/agent/src/loop_.rs#L5517-L5525`,
   `crates/agent/src/lib.rs#L278-L288`); the model is told so (`crates/talk/src/lib.rs#L413-L416`).
   Playback receipts exist for both sinks (`Played` ledger; `played_seq`).
4. **Pure, tested policy core** — `Policy::step` is deterministic over typed inputs and timers, with
   ~20 state-machine tests including the five cuts, noise-at-deadline, hold:none, streaming splits
   (`crates/talk/src/policy.rs#L1051-L1918`); the actor is a thin executor
   (`crates/talk/src/lib.rs#L539-L548`).
5. **Two-stage barge-in with text-domain echo rejection** — pause within one 20 ms callback, decide
   on the first partial, resume for backchannels/echo/noise (`crates/talk/src/policy.rs#L662-L702`);
   works even without AEC because the loop knows exactly what it is saying
   (`docs/design/76-talk.md#L93-L97`).
6. **Daemon-owned devices** — one process, one clock, hard flush, trustworthy position, the TUI
   hears speech (`docs/design/76-talk.md#L52-L67`, `crates/voice/src/out.rs#L206-L263`).
7. **Three parity clients from one contract** — desktop, TUI and phone-through-relay implement 76 §5
   with the same events; frontends "own no audio logic" (`docs/design/76-talk.md#L218-L226`).
8. **Sentence streaming with markdown awareness** — speaks prose as it streams, holds half-arrived
   markup, never splits abbreviations/decimals (`docs/design/76-talk.md#L591-L614`).
9. **Meetings substrate next door** — the same `Utterer`, VPIO capture and TDT transcript-of-record
   are shared code (`crates/voice/src/live.rs#L1-L5`), so a future "talk inside a meeting" is a
   lease question, not a rewrite (OPEN in 76 §12).
10. **Instrumented** — first-sentence / first-audio timings and every policy decision are one
    `RUST_LOG=talk=debug` away (`crates/talk/src/lib.rs#L546-L547`, `#L564-L567`, `#L589-L594`).

## Cites checked

Re-opened with `read_file` after the report was written; each range contains the quoted keyword.

- ✓ `docs/design/76-talk.md#L24-L36` — contains "Cascaded, not full-duplex"
- ✓ `docs/design/76-talk.md#L376-L397` — contains "Three layers, three owners"
- ✓ `docs/design/76-talk.md#L658-L665` — contains "No new tools"
- ✓ `docs/design/76-talk.md#L752-L768` — contains "1.9–3.3 s"
- ✓ `docs/design/76-talk.md#L853-L858` — contains "ToolCx"
- ✓ `docs/design/76-talk.md#L892-L895` — contains "Not exercised"
- ✓ `docs/design/77-talk-followups.md#L33-L39` — contains "documented, not built"
- ✓ `crates/talk/src/lib.rs#L84-L97` — contains "Holding"
- ✓ `crates/talk/src/lib.rs#L161-L181` — contains "barge_in_min_ms: 400"
- ✓ `crates/talk/src/lib.rs#L399-L416` — contains "not a messenger"
- ✓ `crates/talk/src/lib.rs#L599-L613` — contains "Delivery::NextTurn"
- ✓ `crates/talk/src/lib.rs#L1219-L1255` — contains "RegisterPrompt"
- ✓ `crates/talk/src/policy.rs#L27-L70` — contains "PlayedSeq"
- ✓ `crates/talk/src/policy.rs#L662-L702` — contains "ResumeReason::Noise"
- ✓ `crates/talk/src/policy.rs#L704-L741` — contains "Action::Trim"
- ✓ `crates/talk/src/policy.rs#L743-L762` — contains "Cancel(None)"
- ✓ `crates/voice/src/lib.rs#L190-L202` — contains "Device"
- ✓ `crates/voice/src/speak.rs#L17-L23` — contains "24_000"
- ✓ `crates/voice/src/vad.rs#L108-L118` — contains "THRESHOLD"
- ✓ `crates/rpc/src/api.rs#L1357-L1388` — contains "talk_audio"
- ✓ `crates/agent/src/lib.rs#L328-L337` — contains "keep: Option<String>"
- ✓ `crates/agent/src/loop_.rs#L5517-L5525` — contains "interrupted"
- ✓ `crates/tools/src/lib.rs#L294-L306` — contains "resources"
- ✓ `crates/desktop/ui/src/lib/talk.ts#L31-L44` — contains "Working"
- ✓ `crates/tui/src/talk.rs#L11-L29` — contains "Look::Speaking"

25/25 checked ✓.
