# 76 — Voice, phase 3: talk — a live, interruptible spoken conversation, STT and TTS local

Status: design v0 · 2026-09-28 · extends `43-voice.md` (the `Voice` actor
gains a queue and a device sink), `45-voice-notes.md` (the spoken-turn marker
and prompt-block convention), `54-dictation.md` (the daemon mic, `Utterer`,
`Gate`, the remote PCM leg), `53-meetings.md` / `meetings/01-capture.md` (the
macOS VoiceProcessingIO mic moves down into `voice`), `00-architecture.md` §4
(plugin `talk`), §5 (a `Cancel` that keeps a prefix), §7 (RPC), §10 (crate
list); `25-desktop.md` §3 (parity: `talk | /talk`), §7. Wins over 43 §9 where
the two disagree (43 said "no new agent message"; §0.7 below adds one field).
The research brief this answers is the barge-in map (cascaded vs full-duplex,
what must be cancelled, turn-taking vs interruption as separate layers); the
inventory it asked for is §2.

Names: **talk** = one live spoken conversation between the user and one
session, started and stopped by a frontend. **Utterance** = what the VAD gate
opens and closes (54 §0.4). **Turn** = the agent's unit (00 §5). **Barge-in** =
the user speaks while the agent is being heard. **Heard prefix** = the
sentences of an assistant turn whose last audio sample had left the speaker
when the interruption committed.

## 0. Decisions in one screen

1. **Cascaded, not full-duplex.** Audio → `Utterer` (TEN-VAD + Nemotron
   streaming, 54) → the agent session (the model, tools, history, permissions,
   exactly as typed input) → `markdown::Text::committed()` blocks → sentences
   → Pocket TTS (43) → speaker. Interruption is an explicit event that cuts
   five things at once (§4.5). Rejected: Moshi-style speech-to-speech
   (`kyutai-labs/moshi`) — it removes the barge-in state machine by fusing the
   voice, the turn-taking and the intelligence into one 7B-class model with no
   tool calls, no swappable provider, no session history the rest of theuth
   can read; theuth's middle box *is* the product (00 §1.8, 07). Rejected:
   hosted realtime speech APIs (OpenAI Realtime, Gemini Live) — VAD and
   interruption on someone else's server, audio leaves the machine, and the
   local playback still has to be flushed and the history still has to be
   truncated by us; the requirement is local STT and TTS.
2. **A new plugin `talk` (`crates/talk`, actor `TALK_ACTOR = "talk"`) owns the
   conversation loop; `voice` keeps owning the devices and the models.** The
   loop is a stateful, session-bound submitter of turns with its own routing,
   config, events and RPC — the shape of `signal`/`slack`/`meeting`
   (plugins.md §1: "its own state, its own actor, its own events, its own
   config section" → a plugin; constraint: routing belongs to the submitting
   plugin, `signal/src/channel.rs:94-214`). 54 put dictation *in* `voice`
   because "a live dictation is one message being typed"; a talk is many
   turns, a cancel path into the agent and a policy layer, which is what 53
   said does not belong in `voice`. Rejected: a fourth actor in `voice` (a
   crate that would then depend on `agent` for one actor; the mailbox that
   carries 20 ms frames should not also wait on model turns); putting it in
   `agent` (plugins.md §1 says turn *mechanics* go there — the one mechanic
   we need is §0.7; the rest is a channel).
   Dependency order (00 §10): `… ← voice ← talk ← rpc`.
3. **For local frontends the daemon owns the speaker, as 54 made it own the
   microphone.** `voice::out::Player` opens the default output with `cpal`
   (already in the graph as flexaudio's mic backend), 24 kHz mono, and is fed
   PCM straight from the speak task — no Opus round trip in-process. The TUI
   hears speech for the first time (today it ignores `SpeechAudio`,
   `tui/src/model.rs:5077-5080`); the desktop on the harness host stops
   playing in the webview while a talk is on; the remote client (a phone
   through the relay) keeps the Opus `SpeechAudio` leg and pushes PCM in,
   exactly the 54 remote dictation shape. Why: capture and playback in one
   process on one host clock is the precondition for a hard flush (one
   callback, ≤ 20 ms), a playback position the loop can trust, and an echo
   reference (§0.5); the brief's recurring failure ("recognition in one
   process, synthesis in another, glue in a third") is the webview/daemon
   split we have today. Rejected: keep the webview as the only speaker
   (unbounded scheduled buffer `speech.ts:120-134`, no position feedback, no
   TUI); a second audio process (63 §0: one engine, one process).
   `Speak` gains `sink: Sink { Client, Device }`; `Sink::Client` is exactly
   today's behaviour, so 43/45 callers do not change.
4. **`Speak` gains `queue: bool`; sentences are the unit.** The 43 §9 seam.
   `talk` subscribes to `AgentEvent::TextDelta` for its session, feeds one
   `markdown::Text` per turn, and on each committed block runs
   `markdown::spoken` and splits on the sentence-per-line it already emits
   (`markdown/src/lib.rs:155-172`); each sentence is one `Speak{queue: true}`
   with its own `SpeechId`. Block boundaries are inferred as Signal does
   (`TextDelta… → ToolCallPending | TurnFinished`, `signal/src/channel.rs:1670-1740`);
   there is no block-close event and this doc does not add one. The `Voice`
   actor keeps a FIFO of `Queued{id, session, text, sink}` behind `current`;
   `Stop{id: None}` clears the queue and the current utterance in one message.
   Rejected: one utterance per block (a block is a paragraph or more; first
   audio waits for the whole paragraph and an interruption throws it all
   away); speaking every `TextDelta` (mid-word synthesis, and `spoken` needs
   a closed block to strip markdown).
5. **Echo: platform AEC where it exists, an honest degradation where it does
   not, and text-domain rejection always.** The macOS `VoiceProcessingIO`
   mic wrapper the meeting crate already has (`meeting/src/capture/mic.rs:116-205`)
   moves down into `voice::mic` as `Capture::open(Aec::Prefer)`; VPIO cancels
   whatever the device plays, whichever process plays it, so the desktop's
   webview playback would also have been covered — the device sink is chosen
   for §0.3's other reasons. Where VPIO is absent (Windows, Linux) the talk
   runs with `Degradation::MicNoAec` (the meeting crate's existing word,
   `meeting/src/lib.rs:185-187`) and tells the frontend "no echo cancellation;
   headphones recommended" (the same string 53 shows). In every mode the
   barge-in classifier also rejects a candidate whose first transcript is a
   fuzzy prefix of what the agent said in the last two seconds (§4.4) — the
   loop knows exactly what it is saying and when, so the agent hearing itself
   is detectable in text even when it is not cancelled in audio. Rejected:
   shipping a software AEC in phase 1 — the candidates are C/C++ bindings
   (`webrtc-audio-processing`, `speexdsp`) that we have not built on the
   three hosts; §10 step 9 is the spike that decides, with numbers, and the
   `Aec` enum leaves the slot. Rejected: half-duplex (mute the mic while
   speaking) — that is the agent that "talks through a real cut-in".
6. **Interruption is a two-stage decision, not a VAD edge.** Stage 1, *hold*:
   the gate opens (160 ms of speech, 54 §0.4) and stays open for
   `talk.barge_in_min_ms` (default 400) → the player pauses within one 20 ms
   callback, the queue is kept, nothing is cancelled. Stage 2, *decide*: at
   the first `Utterer` partial (one 560 ms window) the candidate is (a) a
   backchannel — every word in `talk.backchannels` ("yeah", "mhm", "okay",
   "right", …) — or (b) an echo (§0.5) → resume from the paused sample; else
   (c) a real cut-in → commit (§4.5). Stage 1 exists because the decision
   costs ~600 ms and the agent must not be heard talking over the user during
   them; a resumed sentence after a "mhm" is a small, honest pause. Rejected:
   VAD-only interruption (the brief's "agent stops on every acknowledgment";
   LiveKit ships an audio model for exactly this reason); a learned
   interruption model in phase 1 (none is open with weights and an ONNX
   export we have verified; the word-list rule is the fallback every
   framework keeps, and `talk.hold` can be turned to `none` to measure it).
7. **A cancelled turn keeps its heard prefix: `Cancel` gains
   `keep: Option<String>`.** Today `Cancel` drops accumulated text
   (`Abort::Cancelled → Round::Stop("cancelled")`, `agent/src/loop_.rs:3612`)
   while a transport failure keeps it (`Abort::Partial`, `:3614-3629` — a
   *salvage*: persist, add a recovery note, ask the model to carry on). `talk` computes the heard prefix from
   the player's position (device sink) or the client's `played_seq` (remote
   sink) — whole sentences only, the cut sentence is not kept — and sends
   `Cancel{session, keep: Some(prefix)}`; the loop persists
   `ChatMessage::assistant(prefix)` the way the salvage does (`:3621-3623`)
   but then *stops* — a new `Abort::Interrupted{text}` beside `Partial`, no
   recovery note, `stop_reason: "interrupted"`. The kept text is the spoken
   form (plain sentences from `markdown::spoken`), not the raw markdown —
   it is what the user heard, and it reads as an ordinary assistant line. The next user input carries the
   marker `[spoken, N s, interrupted]`. The model then sees its own words —
   only the heard ones — as an assistant message, and a user line saying it
   was cut off; nothing else is added to history. Tools: calls are taken from the
   `StreamEnd` and dispatched only after the whole stream
   (`loop_.rs:3729-3780`), so an interruption
   while the agent speaks its pre-tool narration cancels the batch before any
   tool runs; an interruption *during* a tool batch cannot undo the tool
   (bodies run detached to completion, `tools/src/registry.rs:1061-1065`) and
   the loop already writes `"cancelled by user"` results — §12 names the
   `ToolCx` cancel token as the follow-up. Rejected: a user-role note quoting
   the prefix (the model reads its own sentence as the user's; the transcript
   loses the assistant message); persisting the full generated text (the next
   turn reasons about words the user never heard — the brief's poison case);
   a new `HistoryEntry` field (29 §0.2: turns are the unit; the store does
   not change).
8. **End of turn is a policy on top of the gate, separate from barge-in.**
   `Gate::Close` (300 ms quiet) closes an *utterance*; the *turn* is
   committed when, after Close, no new onset arrives for `talk.end_of_turn_ms`
   — default 700 ms, shortened to 250 ms when the final text ends in `.`, `?`
   or `!` (Nemotron emits punctuation and case, 54 §0.3, so the transcript
   itself says whether the thought closed). An onset inside that window
   reopens the same turn and the utterances concatenate. `Gate::Turn` (2 s)
   stays what it is: the `Stream::reset()` boundary. Rejected: silence-only
   at 300 ms (cuts the user mid-thought, the brief's fourth failure); a
   learned end-of-turn model in phase 1 (`pipecat-ai/smart-turn` v3 is open,
   ONNX, CPU ~12 ms and would run on the `ort` we already load — it is §10
   step 10, behind `talk.turn_model`, once its input contract is verified;
   nothing chosen twice).
9. **Speech while the agent is thinking (submitted, no audio yet) is a
   late addition, not a barge-in.** The user finished late; `talk` sends
   `Cancel{keep: None}` and resubmits the previous text plus the new
   utterance as one `[spoken]` input. Nothing was heard, so nothing is
   kept. Rejected: `Steer` (constraint: mid-stream input cannot enter the
   running call, `loop_.rs:1075-1093`; it would land after the reply the
   user is about to not want).
10. **One talk per daemon, bound to the frontend connection that started
    it, on the engine that connection is attached to.** Same slot discipline
    as dictation ("already dictating", `voice/src/dictation.rs:199-201`):
    talk, dictation and a meeting's mic track share the one resident
    `Utterer`; `talk` leases it from the `Dictation` actor (`Lease`/`Return`,
    §4.1) instead of loading a second 0.9 GB Nemotron. The talk ends when
    the frontend says stop, when the connection closes (54's
    remote-dictation rule, `rpc/src/api.rs:4157-4163`), or when the session
    closes (`CloseTree`). Under the mesh (69) the devices are the attached
    engine's and the session writes (`SendInput`, `Cancel`) are forwarded
    like `send_input` is — audio never crosses the mesh
    (knowledge `facts/theuth-voice-features-under-the-mesh-2026-09-25`).
11. **The user's speech enters the session as text with a marker, and one
    prompt block explains it — 45's rule, a second marker.** `[spoken, N s]`
    (and `[spoken, N s, interrupted]`); `talk` registers its own
    `RegisterPrompt` block: the user is speaking live, answer in short
    spoken sentences, one sentence of narration before tools, you may be cut
    off and if you are, your history shows only what was heard. No agent
    state, no `ContentPart`, survives compaction and forks. Rejected:
    reusing `[voice note, N s]` (a note is a finished recording the client
    made; the block for it says "when the user types again go back to
    normal", which a live talk should not trigger).
12. **The talk is a view of its own, not a composer mode (Sky, 2026-09-28).**
    Desktop: a **Talk** entry in the left rail between *New chat* and
    *Agents*; clicking it creates a session and opens `/talk/$session`
    (any existing chat starts talk mode from its composer, below; the TUI
    starts it on the current session with `/talk`). **Composer buttons
    (Sky, 2026-09-28; amends 54 §0.8):** the microphone button becomes one
    gesture control — *tap* toggles dictation (54); *press and hold* records
    a voice note while held (45), *swipe up while holding* locks the
    recording hands-free (tap to stop and send), *slide down / release
    outside* cancels — the Telegram/WhatsApp gesture, with the same haptic
    and visual cues on the phone. The former voice-note button becomes
    **Talk** and keeps its icon — lucide `AudioLines` is the Talk icon
    everywhere (rail entry, composer button, the spoken-turn card; FIX-1);
    the gesture button keeps `Mic`. Talk starts talk mode on this session
    and opens `/talk/$session`. The view: the session transcript on the **left**,
    collapsed by default and expandable (the ordinary chat pane, read-only
    composer); the normal right-hand panels available as in any session;
    the whole window pops out exactly like a session (`lib/popout.ts`).
    Centre, with everything collapsed by default: the **orb**
    (`npx shadcn@latest add @shadercn/orb-20`, states `idle | thinking |
    speaking`) mapped from `TalkState`: `Listening`, `Idle` → idle;
    `Thinking` → thinking; `Speaking`, `Holding` → speaking (Holding keeps
    the speaking look — the player is paused, not stopped — and drops to
    idle on `Interrupted`). Below the orb, minimal real-time **subtitles**:
    the agent's currently playing sentence while it speaks (from the
    ledger's `Playing` sentence, published as `TalkEvent::Saying`), the
    user's live partial while they speak (`Heard{final: false}`), the two
    minimally differentiated (agent: foreground; user: muted, no label, no
    avatar). No level meter, no state badge — the orb *is* the state; the
    headphones line (`Degraded`) is a quiet caption under the subtitles.
    TUI: `/talk` opens an equivalent full-pane view — an ASCII orb animated
    by the same three states, the two subtitle lines beneath, drawn only in
    the active theme's colours; Esc leaves the view (the talk keeps running
    until `/talk` again or Ctrl+T). Both frontends implement §5; they own no
    audio logic. **Mobile parity (Sky, 2026-09-28):** the same route on a
    phone through the relay is the remote client of §5.3 — the webview taps
    the mic and pushes PCM with `played_seq`, plays the Opus leg, cuts on
    `Flush`; the orb and subtitles are the same component at phone width,
    the transcript a sheet. Every voice feature works on the phone; so does
    this one. Rejected: the composer as the talk surface (54's read-only
    composer was right for one dictated message; a conversation needs the
    transcript out of the way and the state in one glance).

## 1. Why

Voice today is three half-duplex features: a reply read aloud on request
(43), a voice note transcribed and answered with clips (45), and dictation
into the composer (54). Each is record → wait → play. The user cannot talk
*with* the harness: they cannot cut a long answer short, cannot add a thought
while it is thinking, and on the TUI cannot hear it at all. The pieces a live
loop needs are built — streaming STT with a VAD gate (`voice::live::Utterer`),
a TTS stream with a per-utterance cancel token, a daemon microphone, a remote
PCM leg — and every one was left with a named seam for "phase 3, the live
agent" (43 §9, 45 §9, 54 §0.2). This doc is that phase: the loop, the
interruption path, and the two things the seams did not anticipate — that the
daemon must also own the speaker, and that a cancel must keep what was heard.

## 2. Facts relied on (recon 2026-09-28, main @ a6607439)

The inventory the research brief asked for, answered from the tree. Paths
relative to `crates/`.

### 2.1 Capture and playback

- The daemon captures the mic natively: `flexaudio` `SourceKind::Mic`,
  48 kHz mono, 20 ms `f32` chunks (`CHUNK 960`), one `std::thread`
  `voice-mic`, opened per use and closed on drop, never at prewarm
  (`voice/src/mic.rs:14-19, 35-44, 78-87`). No resampling there;
  `Utterer::set_rate` resamples with `rubato::FftFixedIn`
  (`voice/src/live.rs:84-104`).
- **The daemon never plays audio.** `voice/src/speak.rs` is the synth +
  Opus-encode task, not a player; there is no output-device code in
  `crates/voice`. Playback is only the webview: libopus-wasm decode, one
  `AudioBufferSourceNode` per 80 ms packet scheduled at
  `max(currentTime + 0.02, next)`, **unbounded run-ahead**; `cut()` stops
  every active source; retired-id packets are dropped
  (`desktop/ui/src/lib/speech.ts:105-134, 87-96, 115-117`). The TUI
  subscribes to `VoiceEvent` and ignores every speech variant
  (`tui/src/model.rs:5077-5080`).
- No playback position ever returns to the daemon; `SpeechFinished{Done}`
  means "every sample published, playback may still have seconds to go"
  (`voice/src/lib.rs:252`). `seq` (80 ms) is the only server-side marker.
- Echo cancellation exists once, in the meeting crate: the mic wrapped in
  Apple `VoiceProcessingIO` (AEC + AGC + NS), falling back to
  `voice::mic::Capture` with `Degradation::MicNoAec`
  (`meeting/src/capture/mic.rs:1-4, 38-63, 116-205`; `meeting/src/lib.rs:185-187`).
  Platform capability: `Support::Full` on macOS, `Partial("no echo
  cancellation; headphones recommended")` on Windows/Linux
  (`meeting/src/capture/system.rs:106-116`; knowledge
  `facts/theuth-meeting-capture-verified-windows-linux-2026-09-23`). The
  webview asks the browser for `echoCancellation: true` on its own tap
  (`desktop/ui/src/lib/dictation.ts:124`).
- Clock: the meeting crate aligns tracks by host receipt time, 60 ms lag,
  no drift estimation (`meeting/src/capture/encode.rs:25-33`). `Utterer`
  keeps a 16 kHz sample clock, `position()` (`voice/src/live.rs:106-109`).

### 2.2 STT and VAD

- `voice::stt::Stream` = Nemotron streaming (int8, ≈ 0.9 GB), `WINDOW =
  8_960` samples = 560 ms, `feed` returns only new text, append-only,
  punctuated and cased, `reset()` drops encoder cache and decoder state
  (`voice/src/stt.rs:32-34, 243-265`; 54 §0.3).
- `vad::Gate` over TEN-VAD (`wavekat-vad`, 16 ms frames): `PREROLL 20`
  (320 ms), `ONSET 10` (160 ms), `CLOSE 19` (≈ 300 ms), `MAX_OPEN 15 s`,
  `TURN 2 s`; `push(prob) → Option<Edge{Onset|Close|Turn}>`; pure, unit-tested
  (`voice/src/vad.rs:12-19, 91-100, 118-148`).
- `voice::live::Utterer` = Stream + Vad + Gate + resampler; `chunk(pcm) →
  Vec<Text{utterance, start, text, is_final}>`; rolling partials per
  non-empty window, a final on Close after padding one window plus one zero
  window; `Send` not `Sync`, one owner; shared by dictation and the
  meeting's track tasks — "knows nothing about microphones, clocks or
  actors" (`voice/src/live.rs:1-5, 20-58, 116-210`).
- The `Dictation` actor holds the one resident `Utterer` (`engine:
  Option<Utterer>`), one dictation at a time, mic opened per dictation,
  `AudioSource::{Mic, Remote{sample_rate}}`, remote PCM = mono LE i16
  base64 ≈ 100 ms per `dictation_audio` call
  (`voice/src/dictation.rs:38-59, 97-156, 199-236`; `rpc/src/api.rs:4135-4197`).
- Measured on this Mac: Nemotron window ≈ 130 ms (knowledge
  `facts/theuth-models-gpu-as-built-2026-09-23`, "STT 128–136 ms").

### 2.3 TTS

- Pocket TTS via `ptts` (`xn`, CPU, 4 threads hard-coded, RTF 0.08 on an
  M3 Max), 24 kHz, one Mimi frame = 80 ms = one Opus packet
  (`voice/src/model.rs:87-93`; `voice/src/speak.rs:14-20`). The `Voice`
  actor is single-slot: `Speak` replaces the current utterance, no queue
  (`voice/src/lib.rs:510-530`; check in 43: "two speaks → first Stopped").
- Stop: `CancellationToken` checked once per `ptts` yield before encoding;
  dropping the stream stops both `ptts` threads; late `Report`s for a
  retired id are dropped by the actor (`voice/src/speak.rs:83, 107-110`;
  `voice/src/lib.rs:414-428, 635-638`). Actual stop ≈ one yield.
- `markdown::spoken` strips markdown to one sentence per line (headings and
  items end as sentences, code blocks become "Code block omitted.")
  (`markdown/src/lib.rs:149-209`). The whole utterance goes to one
  `stream_with`; no server-side sentence chunking (`voice/src/speak.rs:84-85`).
- No agent crate references `VoiceEvent`, `Speak` or `Stop`; barge-in was
  left to "the client: `Stop` + `Cancel`/`Steer`" (43 §9).

### 2.4 The agent loop

- Inputs: `SendInput{session, text, files, delivery, id}` and `Steer` share
  one path (`agent/src/supervisor.rs:2264-2306` → `route`); a busy session
  queues, drained only between tool rounds or at turn end
  (`agent/src/loop_.rs:1075-1093, 3198-3209`). `Cancel{session}` cancels the
  session and every running descendant, `TurnFinished{stop_reason:
  "cancelled"}` (`supervisor.rs:2492-2510`, `CancelTurn` is a unit struct,
  `loop_.rs:305`).
- Cancel is observed at the next `select!` poll: stream open, every stream
  event, retry sleep, tool batch (`loop_.rs:2837, 2867, 2876, 3836-3848`).
  **On cancel mid-stream the accumulated text is dropped** — `call` returns
  `Abort::Cancelled` and `round` returns `Stop("cancelled")` without
  persisting (`:3612`); the transport-error path returns
  `Abort::Partial{text, err}` (`:2821`) and `round` persists the text, writes
  a `recovery` note and continues the turn (`:3614-3629`, 36 §2.4). Tool bodies outlive a cancel; the loop
  writes synthetic `"cancelled by user"` results (`:3875-3878, 4001-4005`;
  `tools/src/registry.rs:1061-1065`). `ToolCx` has no cancellation token
  (`tools/src/lib.rs:287-297`).
- Text commits once per model round at stream end (`:3736`); tool calls
  are read from the `StreamEnd` (`captured_tool_calls`, `:3729`) and
  dispatched after the whole stream (`:3808-4043`); `ToolCallPending` is
  published on the first naming chunk (`:2922`). No prefix truncation exists; undo/redo cut
  at turn starts (`agent/src/store.rs:1503-1570`; 29 §0.2).
- Events: `TurnStarted`, `TextDelta{text}` unbuffered, `ToolCallPending`
  on the first naming chunk, `ToolCallStarted/Finished`, `TurnFinished`
  (`agent/src/lib.rs:1551-1904`; `loop_.rs:2885-2922, 3284-3291`). No
  block-close event. `Round::Yield` continues a turn without `TurnFinished`
  (`:3213-3220`).
- Pattern for a non-frontend submitter: Signal `ask`s `SendInput`/`Steer`,
  subscribes to `AgentEvent`, keeps a per-session `Turn{in_flight, text,
  voice}`, flushes at `ToolCallPending`/`TurnFinished`, synthesizes when
  `voice` (`signal/src/channel.rs:328-363, 405-422, 483-495, 1748-1800`).
  `[voice note, N s]` is a client-built marker plus a static prompt block
  (`voice/src/lib.rs:125-147`).

### 2.5 What is missing (the brief's checklist against the tree)

| Barge-in needs | Today | Where this doc adds it |
|---|---|---|
| Listen while speaking | no: VAD only inside a user-started dictation | §4.2 |
| Backchannel vs cut-in | no | §4.4 |
| Flush playback incl. buffered | client `cut()` only; unbounded buffer; no daemon player | §0.3, §4.3 |
| Cancel generation | `Cancel` exists, drops text | §0.7 |
| Drop unplayed synthesis | `Stop` exists, single slot, no queue | §0.4 |
| Reset recognition | `Utterer::reset()` exists | §4.5 |
| Truncate history to heard prefix | no | §0.7, §4.6 |
| Drop the interrupted turn's tools | free before dispatch; not during a batch | §0.7, §12 |
| Echo reference / shared clock | macOS VPIO in `meeting` only; no clock | §0.5, §4.3 |

## 3. Shape

```
                 ┌──────────────── voice (devices, models) ────────────────┐
mic ─ Capture ──►│ Utterer (VAD·gate·Nemotron)  Voice{queue}─►Player(cpal)│─► speaker
  (VPIO on macOS)│         │ Text                    ▲  Speak{queue,sink}   │  ▲ position()
                 └─────────┼─────────────────────────┼──────────────────────┘  │ pause/flush
                           ▼                         │                         │
                 ┌──────────────────────── talk (one actor) ────────────────────┐
                 │  Listening ─► Thinking ─► Speaking ─► Holding ─► Interrupted │
                 │  turn policy · barge-in classifier · heard-prefix ledger      │
                 └─────────┬───────────────────────────▲──────────────────────┘
                           │ SendInput / Cancel{keep}  │ TextDelta · ToolCallPending · TurnFinished
                           ▼                           │
                        agent session (model, tools, history — unchanged)

remote client: PCM in (talk_audio + played_seq) ─► same Utterer; Opus SpeechAudio out (43)
```

Three layers, three owners: `voice` owns audio and inference and knows
nothing about turns; `talk` owns the state machine and knows nothing about
codecs or models; `agent` is unchanged except that `Cancel` can keep a prefix.

## 4. Plugin `talk` (`crates/talk`)

### 4.1 Actor, state, messages

`Talk` (`TALK_ACTOR = "talk"`), one instance, state:

```rust
pub struct Talk {
    active: Option<Live>,                 // the one talk
    prompt: &'static str,                 // registered block (§0.11)
}
struct Live {
    id: TalkId,                           // uuid v7, transparent, like SpeechId
    session: SessionId,
    peer: PeerId,                         // the frontend connection that started it
    sink: Sink,                           // Device | Client
    aec: Aec,                             // Vpio | None (reported, not chosen)
    state: State,                         // §4.2
    engine: Lease,                        // the Utterer, from Dictation
    cancel: CancellationToken,            // ends the audio task
    turn: Option<TurnLedger>,             // §4.6
    pending_text: String,                 // utterances of the turn being built
    since: Instant,                       // for `N s`
}
```

Commands (`ask`):

| Message | Reply | Meaning |
|---|---|---|
| `StartTalk { session, sink, sample_rate: Option<u32> }` | `Result<TalkId>` | one at a time; `sample_rate` only for `Sink::Client` (remote PCM), `None` = daemon mic |
| `PushAudio { id, pcm: Vec<i16>, played_seq: Option<u32> }` | `()` | remote leg: PCM in, playback position back |
| `StopTalk { id }` | `()` | idempotent; stops speech, ends audio task, returns the lease |
| `TalkStatus` | `TalkState { active: Option<{id, session, state, aec}> , model: ModelState }` | |

`voice` additions (contract other plugins code against):

| Message | Change |
|---|---|
| `Speak { session, text, queue: bool, sink: Sink }` | `queue: false, sink: Client` = today; `queue: true` appends behind `current` |
| `Stop { id: Option<SpeechId> }` | `None` now also clears the queue |
| `Pause { }` / `Resume { }` | device sink only; `Pause` returns `Played { id, seq, samples }` |
| `Dictation::Lease` → `Utterer` / `Dictation::Return(Utterer)` | the resident engine changes hands; `StartDictation` while leased → `"talking"` |

`Utterer` stays as it is; `talk` drives it like the dictation task does
(`chunk`, `close`, `reset`, `position`).

### 4.2 State machine

```
Idle ──StartTalk──► Listening ──turn committed (§4.7)──► Thinking ──first Speak──► Speaking
  ▲                    ▲   ▲                                 │                        │
  │                    │   └── TurnFinished, queue empty ◄───┴────────────────────────┤
  │                    │                                                              │ gate open ≥ barge_in_min_ms
  │                    │                                          Holding ◄───────────┘
  │                    │                                          │      │
  │                    │             backchannel / echo → Resume ─┘      │ real cut-in
  │                    └── Interrupted (Stop all · Cancel{keep} · reset STT) ◄──┘
  └──StopTalk / connection closed / CloseTree (from any state)

Thinking + gate open ≥ barge_in_min_ms ──► Cancel{keep: None}, merge, resubmit (§0.9)
```

The audio task (one `spawn_blocking`, like the dictation task,
`voice/src/dictation.rs:245-272`) runs the VAD in every state: `Utterer::chunk`
on every 20 ms chunk, `Text`s and `Edge`s reported to the actor by `tell`. The
actor is the only place state changes.

### 4.3 The device sink: `voice::out::Player`

- `cpal` default output, `f32`, 24 kHz mono (device rate negotiated, `rubato`
  if the device refuses 24 kHz), one ring buffer sized `talk.player_buffer_ms`
  (default 200) — the run-ahead cap the webview lacks.
- The speak task writes PCM frames straight into the ring (bypassing Opus
  when `sink == Device`); the callback reads; `played: AtomicU64` counts
  samples that have left the callback; `Player::position()` = played −
  `output_latency` reported by the device (or 40 ms if unknown, [INFERENCE:
  cpal exposes latency on CoreAudio and WASAPI, not on all ALSA/PipeWire
  paths — §10 step 3 measures]).
- `pause()` zero-fills from the next callback and returns the position;
  `resume()` continues from the paused sample; `flush()` empties the ring
  and retires the current id. All three are one atomic store; the audible
  effect is bounded by the callback period (≤ 20 ms) plus device latency.
- Each queued utterance records `(SpeechId, first_sample, last_sample)` as
  its frames enter the ring; that ledger, read at `position()`, is what
  §4.6 turns into "sentence n was heard".
- The webview, when a talk with `Sink::Device` is on, receives no
  `SpeechAudio` (the actor does not publish them for device-sink utterances)
  and shows the level from `TalkEvent::Level`. The `speak` button's own
  utterances keep `Sink::Client`; if the user presses it during a talk,
  `Speak` stops the talk's queue like any other `Speak` would today.

### 4.4 Barge-in classifier

Inputs at the decision point (the first `Text` partial of the candidate
utterance, or `talk.decide_ms` (900 ms) after onset if none came — then it is
**noise**, never a cut-in; FIX-1, 2026-09-29: an aquarium held the gate open
past `barge_in_min_ms`, never yielded a word, and the old "silent at the
deadline = real" fallback cut the agent off):

1. `Gate` open for ≥ `barge_in_min_ms` (400) — filters coughs, chair
   scrapes, the 160 ms onset of a word the user swallowed. While the agent
   speaks (`Speaking`/`Holding`) the gate's speech threshold is
   `talk.barge_in_vad` (0.7) instead of the 0.5 dictation and meetings use;
   the policy emits `Action::VadThreshold` on every state change and the audio
   task applies it to the leased `Utterer` (`Gate::set_threshold`), restoring
   0.5 when the lease goes back.
2. Backchannel: lowercase, strip punctuation, split on whitespace; every
   token in `talk.backchannels` → backchannel. Default list: `yeah yep yes
   mhm mm mm-hmm uh-huh ok okay right sure hm hmm i see got it`.
3. Echo: normalise the partial and the last ~2 s of sentences the ledger
   marks as playing; if the partial is a prefix (Levenshtein ratio ≥ 0.8) of
   that text → echo. Logged at `debug` with the two strings so `MicNoAec`
   hosts can be tuned.
4. No word: no partial at all, or none with a non-backchannel token of two or
   more characters once punctuation is stripped ("I", "a", ".") → noise. A
   wordless partial does not settle the hold; the next partial or the
   deadline does.
5. Otherwise real.

Noise, backchannel and echo resume playback (`Resumed{reason}`) and dismiss
the candidate utterance; a new candidate needs the gate to close and open
again (a candidate is only created on `Onset`), so steady noise cannot
hold-resume-hold every 900 ms — if the gate never closes, the speech
continues unpaused. The decisions a user reads from the default engine log
are `info` under target `talk`: `holding`, `resumed{reason, text}`,
`interrupted{heard, dropped, text}`.

`talk.hold: pause | none` decides whether stage 1 pauses the player (default
`pause`); `none` keeps speaking until the decision — the knob exists so the
two behaviours can be compared in §10 step 7, not as a user preference.

### 4.5 Commit an interruption — the five cuts

In this order, from one actor handler, nothing awaited between them:

1. `Player::flush()` (device) — or, remote, publish `TalkEvent::Flush{id}`
   and rely on the client's `cut()`.
2. `Voice::Stop{id: None}` — current utterance token cancelled, queue
   cleared, `SpeechFinished{Stopped}` per dropped id.
3. `Agents::Cancel{session, keep: Some(prefix)}` with the prefix from §4.6.
4. `Utterer::reset()` is **not** called here — the candidate utterance is
   the start of the next turn and its audio is already in the stream; the
   gate is left open. (`reset()` happens at `Gate::Turn` as today.)
5. The turn ledger is closed with `heard = prefix`; `pending_text` starts
   with the candidate's text; state → `Listening` with an open gate. The
   marker for the next input becomes `[spoken, N s, interrupted]`.

Then `TalkEvent::Interrupted{id, session, heard_sentences}`.

### 4.6 The turn ledger and the heard prefix

Per agent turn `talk` keeps `TurnLedger { sentences: Vec<Sentence{ text,
speech: Option<SpeechId>, status: Queued | Playing | Played | Dropped }> }`,
appended as blocks commit and sentences are enqueued, advanced by
`SpeechStarted`/`SpeechFinished` and, for the device sink, by the player's
ledger at flush time; for the remote sink by the last `played_seq` the client
sent (the client reports the highest `seq` whose source `onended`, once per
`talk_audio` call, ≈ 100 ms). Heard prefix = the concatenation of `Played`
sentences in order; a `Playing` sentence at flush time is `Dropped`. Sentences
of a block that never reached `Speak` (the model was still streaming) are
`Dropped`. The ledger is the only place that knows both what was generated and
what was heard; the store never learns about audio.

### 4.7 Turn policy

- On `Text{is_final}`: append to `pending_text` (space-joined), start the
  end-of-turn timer: 250 ms if the text ends in `.` `?` `!`, else
  `talk.end_of_turn_ms` (700). On `Edge::Onset` before it fires: cancel the
  timer, keep accumulating. On fire: submit `SendInput{session, text:
  "[spoken, N s] " + pending_text, delivery: NextTurn, id: None}` → `Thinking`.
- Partials (`is_final: false`) go to `TalkEvent::Heard{final: false}` only —
  frontends show them, the session never sees them.
- A `MAX_OPEN` (15 s) close is an ordinary final; the timer runs as usual.
- Empty finals (a cough the VAD opened on, no text) are discarded and do
  not start the timer.

### 4.8 Speaking the reply

Subscribe to `AgentEvent` for `session`; per turn one `markdown::Text`. On
`TextDelta` push; on `ToolCallPending` or `TurnFinished` commit and take the
closed blocks; for each block `markdown::spoken(block)` → lines → each line
`Speak{session, text: line, queue: true, sink}` → ledger `Queued`. First
`SpeechStarted` → `Speaking`. `TurnFinished` with an empty queue and
`SpeechFinished{Done}` for the last id → `Listening` (the gate may already be
open — the user often starts before the tail ends; that is an ordinary onset
in `Listening`, not a barge-in, and the tail is not cut). `Round::Yield` and
tool batches keep the state `Speaking` while the queue drains, then `Thinking`
until the next block. `TurnFinished{stop_reason: "cancelled" | "interrupted"}`
from anyone else (the user pressed Esc in the TUI) → `Stop{None}`, ledger
closed, `Listening`.

Blocks longer than one sentence begin to be heard after their first sentence
synthesises (~200 ms + one 80 ms frame, 43 §0.5). **Sentence streaming
(TALK-STREAM, 2026-09-29):** the open block — the pending tail after
`Text::committed()` — is spoken sentence by sentence while it streams when it
is plain prose: exactly one paragraph by pulldown-cmark, first line without
`|`. On every delta `spoken(tail)` is cut by the same splitter as committed
blocks (`policy::sentences`/`split`: `.` `?` `!` + whitespace, a short
abbreviation list — "e.g.", "Dr." — never splits, decimals have no whitespace
after the dot); each sentence that is complete (whitespace after its terminal
punctuation has arrived) is `Speak`-ed at once and counted in `streamed`;
the last fragment waits for the block to close, and the commit skips the
`streamed` sentences it already spoke. A sentence still holding inline markup
characters (`*`, `_`, a backtick, `[`, `<`, `~`, `|` — half-arrived emphasis, a link, a possible table
row) is held, with everything after it, until the close renders it. Headings,
lists, code fences, tables, block quotes and HTML still commit on close
whole; a first line that could yet become a list marker ("1", "-") has no
complete sentence, so it waits for its kind as before. Ledger indices stay
monotonic per turn, so `Cancel{keep}`, `TrimAssistant` and `Interrupted{heard,
dropped}` are unchanged; deltas that arrive after our own `Cancel` are
ignored. Known misses: an unlisted abbreviation or initial ("J. Smith")
splits; a paragraph later re-read as a setext heading or as a table whose
header row does not start with `|` has had its leading sentences spoken as
prose (same words). The model is still asked for short sentences by the
prompt block, now the only lever on time-to-first-sentence (§8).

### 4.9 Events (`TalkEvent`, `#[serde(tag = "type")]`, `ts` export)

| Variant | Fields | When |
|---|---|---|
| `TalkStarted` | `id, session, sink, aec: Aec, since_ms` | after the lease and the devices opened |
| `TalkState` | `id, state: State` | every transition of §4.2 |
| `Level` | `id, rms: [f32; 8]` | 160 ms, as dictation |
| `Heard` | `id, utterance, text, final: bool` | every `Utterer::Text` |
| `Saying` | `id, speech: SpeechId, text` | a sentence starts playing (device: first frame leaves the ring; client: `SpeechStarted`) — the agent subtitle |
| `Submitted` | `id, text, interrupted: bool` | the input the session received |
| `Holding` | `id, since_speech_ms` | stage 1 |
| `Resumed` | `id, reason: Backchannel \| Echo \| Noise` | stage 2 (a)/(b), or no word by the deadline |
| `Interrupted` | `id, heard_sentences: u32, dropped_sentences: u32` | stage 2 (c) |
| `Flush` | `id` | remote sink: cut everything now |
| `Degraded` | `id, degradation: MicNoAec` | at start on hosts without VPIO |
| `TalkFinished` | `id, reason: Stopped \| Disconnected \| SessionClosed \| Error{message}` | |

`VoiceEvent` gains nothing new for the device sink except that
`SpeechStarted`/`SpeechFinished` carry `sink`; `SpeechAudio` is published for
`Sink::Client` only.

### 4.10 Config

```yaml
talk:
  enabled: true              # false: no actor, `talk_*` RPC → "talk is disabled"
  barge_in_min_ms: 400       # stage 1
  decide_ms: 900             # stage 2 fallback: no word this long after onset → noise, resume
  barge_in_vad: 0.7          # gate speech threshold while Speaking/Holding (Listening/Thinking: the gate's 0.5)
  end_of_turn_ms: 700        # after Close without terminal punctuation
  end_of_turn_punct_ms: 250  # after Close with . ? !
  hold: pause                # pause | none
  backchannels: [yeah, yep, yes, mhm, mm, mm-hmm, uh-huh, ok, okay, right, sure, hm, hmm, i see, got it]
  echo_ratio: 0.8            # §4.4 step 3
  player_buffer_ms: 200      # voice.out ring (lives under `voice:` as `voice.player_buffer_ms`)
  aec: prefer                # prefer | off   (off = plain mic even on macOS; for measurements)
  turn_model: none           # none | smart-turn  (§10 step 10; refused until the model dir exists)
```

`voice:` gains `player_buffer_ms` and `player_device: Option<String>`. Every
key mirrored in `DEFAULT_CONFIG` (`theuth/src/main.rs:620-629` pattern).

### 4.11 Permissions and prompt

No new tools; no new `Risk`. The session's tool policy, approvals and
`Ask`/`Resolve` gate are untouched — an approval prompt during a talk is
answered on the frontend as today; the agent's spoken narration before the
batch is what the user hears while it waits. The prompt block (§0.11) is a
pure function of config (07 §4) registered with `RegisterPrompt{plugin:
"talk"}`, present whenever `talk.enabled`, like voice's note block.

## 5. Frontend contract (both UIs implement exactly this)

### 5.1 Start / stop

`talk_start(session, sink, sample_rate?)` → `TalkId`; `talk_stop(id)`;
`talk_status()`. Local peers pass `sink: "device"`, `sample_rate: null`;
remote peers must pass `sink: "client"` and a rate (refused otherwise, the
54 rule at `rpc/src/api.rs:4146-4151`). One talk per daemon: a second
`talk_start` answers `"already talking"` with the live id.

### 5.2 Render (§0.12)

Route `/talk/$session` (desktop) and the `/talk` pane (TUI). Orb state from
`TalkState`; agent subtitle from `Saying` (cleared on `SpeechFinished` for
that id, or on `Interrupted`); user subtitle from `Heard{final: false}`,
replaced by the final and then cleared on `Submitted`. The left transcript
is the ordinary session view, collapsed; a `[spoken, N s] …` user turn
renders as a card, not raw text (FIX-1): the transcript as the body in the
user-message styling, a quiet `AudioLines` caption `spoken · N s` (plus
`cut in` for `[spoken, N s, interrupted]`); the TUI collapses the marker to
a muted `spoken N s` tag before the text. The assistant's text — including
a kept prefix after an interruption — renders as any chat does. Esc during
`Speaking` is the agent's cancel as today; the
talk stays on and goes to `Listening` (§4.8). Leaving the view does not stop
the talk; the sidebar entry shows a live dot while one runs; `talk_stop`
is the orb's own control (click / Ctrl+T).

### 5.3 Remote client only

Push PCM as dictation does (`dictation.ts` tap and `Batcher`), to
`talk_audio(id, pcm, played_seq)`; on `TalkEvent::Flush` call `speech.cut()`
and free the decoder; keep `speech.ts`'s `push` filtering by id (`SpeechAudio`
now also carries `sink`, ignore `device` ones defensively).

## 6. RPC (`crates/rpc/src/api.rs`)

`talk_start`, `talk_stop`, `talk_audio`, `talk_status` (flat snake_case, the
`dictation_*` shape, `with_extensions` for peer identity); `TalkEvent` joins
the bridge macro list (`rpc/src/bridge.rs:115`) and `UiEvent::Talk` in the
TUI. `talk_audio` decodes base64 LE i16 exactly as `dictation_audio`. The
connection-close hook that stops a remote dictation also stops a talk whose
`peer` matches.

## 7. `agent` change (one field, one message)

`Cancel { session, keep: Option<String> }` (`agent/src/lib.rs:263-266`
today has `session` only). The `CancellationToken` carries no payload, so
`CancelTurn` (a unit struct today, `loop_.rs:305`) gains the same
`keep` and its handler stores it on the session state before it cancels; `call`'s cancel arms (`loop_.rs:2867, 2876`) return a new
`Abort::Interrupted { text: keep }` when one is stored and `Abort::Cancelled`
otherwise; `round` handles `Interrupted` beside `Partial` (`:3614`): persist
`ChatMessage::assistant(text)` when non-empty, no recovery note, return
`Round::Stop("interrupted")`. Tool calls named in the dropped stream are
not dispatched (they never were — calls are read from the `StreamEnd`,
`:3729`). If the cancel lands during a
tool batch, `keep` is ignored (the assistant message of that round was
already persisted at stream end; the narration was heard or not — the ledger
knows, the store keeps the round as generated, and the `[spoken … interrupted]`
marker tells the model it was cut; §12). Every existing `Cancel` sender passes
`keep: None` and behaves as today. Amend 00 §5 in the same commit.

**The post-turn case (`TrimAssistant`, built by KEEP-AFTER-TURN).** A short
reply finishes streaming — and is persisted in full at stream end — before
its audio finishes playing, so a barge-in during playback sends its cancel
outside any stream and `keep` would be ignored: the whole reply would stay
in history although the user heard part of it, the poison case of §0.7 for
every one-paragraph answer. The ledger knows which it is: when a
`TurnFinished` has already been seen for the turn being spoken, the commit
(§4.5) sends `TrimAssistant { session, turn: turn_seq, keep: heard_prefix }`
instead of `Cancel { keep }` (the `Stop{None}` + flush before it and the
`Interrupted` event and `[spoken, N s, interrupted]` marker after it are the
same on both paths); while the turn is still running, `Cancel { keep }` as
above. `TrimAssistant` is refused while a turn runs for that session (the
same idle check as `Undo`/`EditTurn`); otherwise the session actor replaces
the text of the LAST assistant message of that turn in the store and in the
in-memory frame with `keep` (empty `keep` deletes the message), leaves tool
messages intact, bumps the context revision and publishes
`ContextChanged { reason: "trim" }` — the event both frontends already
answer with a settled reload of the transcript, so no new event is needed.
If the turn ran tool calls after the spoken narration (rare), only the last
assistant message is trimmed and the actor says so at `debug`. `turn` is
`TurnStarted.turn_seq`, the turn's first user row, which after a cancelled
turn that left no reply (§0.9) is not a turn *start* in the store's sense;
the store therefore keys the trim on the user row, not on turn starts.

## 8. Latency budget (targets; §10 measures)

| Segment | Budget | Source |
|---|---|---|
| User stops → `Gate::Close` | 300 ms | `CLOSE 19` |
| Close → final text | ≈ 560 ms window pad + ~130 ms inference | 54; knowledge STT 128–136 ms |
| End-of-turn wait | 250 / 700 ms | §0.8 |
| Model time-to-first-sentence | provider-bound, 0.5–1.5 s | not local; the one segment this doc cannot shrink |
| First sentence → first audio | ~200 ms + 80 ms | 43 §0.5 |
| **Speech end → agent audible** | **≈ 1.9–3.3 s** | sum; the block wait of §4.8 is inside "first sentence" |
| Onset → hold (player silent) | 400 ms + ≤ 20 ms + device latency | §0.6, §4.3 |
| Onset → decision | ≤ 900 ms | §4.4 |
| Barge-in commit → speaker silent | ≤ 20 ms + device latency (device); one client `cut()` (remote) | §4.3 |

The second half is what "interruptible" means and it is entirely local; the
first half is dominated by the provider and by asking the model for short
first sentences.

## 9. Data on disk

Nothing new. Models: `<data_dir>/models/{nemotron-streaming-en, pocket-tts,
onnxruntime}` as today; `smart-turn/` only if §10 step 10 lands. The talk
writes no files; the session store holds the `[spoken]` turns and the
kept prefixes as ordinary messages.

## 10. Phases and checks

1. **`voice::out::Player`** (cpal, ring, pause/resume/flush, position,
   ledger). Check: `cargo test -p voice out::` — 5 s of a 440 Hz tone through
   the ring with `flush()` at 2 s: silence within one callback period,
   `position()` within 1 frame of samples written − latency; on the three
   hosts a `just run -- --bench-audio-out` prints device latency and callback
   period.
2. **`Speak{queue, sink}` + queue in `Voice`.** Check: three `Speak{queue:
   true}` → three `SpeechStarted` in order, one `SpeechFinished{Done}` each;
   `Stop{None}` mid-second → `Stopped` for the second and third, nothing for
   the first; `Speak{queue: false}` during a queue → queue cleared (today's
   semantics preserved for 43/45 callers, whose tests must still pass).
3. **`Capture::open(Aec::Prefer)`** — move VPIO from `meeting` to `voice`,
   meeting uses it. Check: `meeting` tests unchanged; on macOS the capture
   probe shows `aec: Vpio`; on Windows/Linux `Aec::None` and the
   `MicNoAec` degradation, exactly the 2026-09-23 verified outputs.
4. **`Dictation::Lease/Return`.** Check: lease → `StartDictation` refused
   `"talking"`; return → dictation works; a talk started while dictating
   refused `"already dictating"`.
5. **`talk` crate: actor, audio task, turn policy, `SendInput`, speaking
   the reply** — no barge-in yet. Check: TUI `/talk`, say "what time is
   it?", the reply is heard from the daemon speaker; transcript shows
   `[spoken, 2 s] What time is it?`; the level meter moves; `talk_stop`
   releases mic and speaker (OS indicators off).
6. **`Cancel{keep}`** in `agent` + ledger + the five cuts. Check
   (integration, fake provider streaming three sentences at 100 ms each,
   fake player advancing at real time): cancel with `keep` after sentence 2
   → history holds exactly the two sentences, `TurnFinished{interrupted}`,
   next input carries `[spoken … interrupted]`; the fake tool named in the
   stream never runs.
7. **Classifier and hold.** Check: with a recorded corpus (10 backchannels,
   10 cut-ins, 5 echoes played through the speaker on a `MicNoAec` host):
   ≥ 9/10 backchannels resume, ≥ 9/10 cut-ins commit within 900 ms of onset,
   ≥ 4/5 echoes rejected; `hold: none` vs `pause` compared on "agent audible
   after user onset" — expected ≤ 420 ms vs ≤ 900 ms.
8. **Desktop button, remote leg** (`talk_audio` + `played_seq` + `Flush`).
   Check: phone through the relay, interrupt mid-sentence, the phone goes
   silent on `Flush`, the heard prefix equals what the phone played (compare
   `played_seq` ledger to `onended` log).
9. **Software AEC spike (decides §0.5's slot).** Build `webrtc-audio-processing`
   (or `speexdsp`) on the Windows and Linux hosts; feed the player's PCM as
   far-end, the mic as near-end; measure ERLE and CPU on a 5 s echo. Land it
   only if it builds on both and the classifier's echo rejections drop to
   zero on the corpus; otherwise the degradation stands.
10. **Smart Turn v3 spike (decides `turn_model`).** Verify the ONNX input
    contract (16 kHz, ≤ 8 s, features) against `pipecat-ai/smart-turn`,
    run on `ort`, measure on the three hosts; compare end-of-turn latency
    and false commits against §0.8's punctuation rule on the same corpus.
11. **Amend 00 (§4, §5, §10), 25 §3/§7, 43 §9, 54 §0.2** in the same
    series; write §13 Status.

Steps 1–4 are `voice` and ship independently (the queue alone improves 45's
desktop voice mode). Steps 9 and 10 are spikes with a written exit either way.

## 11. Rejected (beyond §0)

- **Silero VAD** — TEN-VAD is loaded and gated already (54 §0.4); nothing to
  gain twice.
- **Speculative generation on `Close` (start the model before end-of-turn,
  cancel if the user continues)** — saves ≤ 700 ms per turn at the cost of a
  provider call per false end; `Cancel` drops text so it is safe, but tokens
  are money and the punctuation rule takes most of the win. Open (§12).
- **Agent-interrupts-user** — a research toy in the brief; nothing in the
  requirement.
- **Diarization to tell the user from the agent** — the meeting crate's
  labels are per track, not per voice (`meeting/src/diar.rs:1-9`); the agent
  knows what it said, so text-domain rejection is cheaper and exact.
- **A webview-owned loop** (VAD and playback in `speech.ts`) — nothing for the
  TUI or headless, two implementations, and the model runs in the daemon
  anyway.
- **A block-close `AgentEvent`** — Signal and `talk` infer it identically
  from two existing events; adding it changes 07 for no second consumer.

## 12. Open

- **Tools during a barge-in.** A cancel during a tool batch cannot stop the
  body (`ToolCx` has no token). Giving `ToolCx` a `CancellationToken` that
  `shell`/`python`/MCP honour is an `agent` + `tools` change with its own
  doc; until then a talk that cuts the agent during "I'll run the migration
  now" has already run it — the prompt block asks for narration *before*
  the batch, which is where the user can still stop it.
- **Speculative generation** (§11) once §10 step 7 gives real end-of-turn
  numbers.
- **`Speak` from the desktop button during a talk** clears the talk's queue
  (§4.3); acceptable, or should the talk own `Speak` while it runs?
- **Windows/Linux echo** — step 9 decides; if neither library builds,
  the honest state is `MicNoAec` + text rejection + "headphones recommended".
- **Meeting mic track + talk** share one `Utterer` lease; a talk during a
  recorded meeting is refused in phase 1. Is that the right answer, or does
  the meeting's `me` track *become* the talk's input?
- **Provider latency** is the largest segment of §8 and outside this doc;
  a "first sentence fast" hint in the prompt block is the only lever here.

## 13. Status

Not built. Recon on which §2 rests: two read-only passes over main @
a6607439 on 2026-09-28 (`~/.theuth/scratch/design-76/recon-voice.md`,
`recon-agent.md`). Third-party facts not verified in this pass and therefore
gated behind spikes: `cpal` output-latency reporting per backend (step 1),
`webrtc-audio-processing`/`speexdsp` buildability on the three hosts (step
9), Smart Turn v3's ONNX input contract (step 10).

**As built 2026-09-28** (`docs/design/76-as-built-*.md`, seven docs; closer:
`76-as-built-INTEGRATE.md`). Built on `feature/talk`: `voice::out::Player`,
`Speak{queue, sink}` and the `Voice` queue, `Capture::open(Aec)` with VPIO
moved down from `meeting`, `Dictation::Lease/Return`, the `talk` crate
(actor, `policy` state machine, turn policy, classifier, ledger),
`Cancel{keep}` in `agent`, the four `talk_*` RPCs, the TUI `/talk` pane and
the desktop `/talk/$session` view with the orb. Exercised live on this Mac
(debug build, Nemotron + Pocket TTS local, `openai/gpt-5.6-sol`): device
sink through the room mic (VPIO) and the daemon speaker from both the TUI
and the desktop — `[spoken, N s]` turn, spoken reply, barge-in →
`Holding` → the five cuts in one handler → `[spoken, N s, interrupted]`;
the remote leg (`talk_audio` + `played_seq` + `Flush`) with a scripted client.
Not exercised: audio by ear, a non-empty heard prefix (debug TTS is too slow
for a sentence to finish before the cut; §0.7's truncation therefore only
ran with `keep: ""`), the backchannel/echo corpus (§10 step 7), a phone
through the relay, Windows/Linux, the Tauri shell. Two integration bugs were
found and fixed on the way: `TalkEvent::Level` collided with
`MeetingEvent::Level` on the UI stream (now `TalkLevel`), and the attached
TUI's event mirror (`rpc::client`) swallowed every event after `A2aChanged`
(a field-less tagged struct) — pre-existing, hit dictation too. Added knob:
`talk.decide_ms` (§4.4's 900 ms).