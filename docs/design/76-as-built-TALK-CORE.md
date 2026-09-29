# 76 as built — TALK-CORE (the `talk` actor)

Branch `talk/core` off `feature/talk` @ 3bc7b549 (TALK-CONTRACT, VOICE-OUT,
AGENT-KEEP merged). Spec: 76 §4 entire, §0.6–§0.9, §10 steps 5–7. The
contract's types, config, RPC and prompt block are unchanged in shape; this
step fills in the behaviour.

## 1. What was built

### 1.1 `crates/talk/src/policy.rs` — the pure `Policy` (new)

Everything the talk decides, as a function of inputs with the time they
arrived; no clocks, actors or devices. The actor feeds `Policy::step(Input,
Instant) -> Vec<Action>` and carries the actions out in order.

- `Input`: `Onset`, `Close` (gate edges), `Text{utterance, text, is_final}`,
  `Timer(EndOfTurn | Hold | Decide)`, `Delta(text)` / `BlockEnd` /
  `TurnFinished(stop_reason)` (agent), `SpeechStarted(id)` /
  `SpeechFinished{speech, done}` / `Packet{speech, seq}` (voice),
  `PlayedSeq(seq)` (client sink).
- `Action`: `State`, `Heard`, `Speak{index, text}`, `Saying`, `Submit{text,
  interrupted}`, `Cancel(keep)`, `StopSpeech`, `Pause`, `Resume(reason)`,
  `Holding`, `Commit`, `Interrupted`, `Timer(kind, Some(after) | None)`.
- `Policy::spoken(index, SpeechId)` records `Speak`'s reply;
  `Policy::paused(Played)` records `Pause`'s; `Policy::commit(Option<Played>)`
  is the five cuts (§4.5) once the actor has the player position;
  `Policy::failed(index)` drops a sentence `Speak` refused.
- Turn policy (§4.7): finals append to `pending`; the end-of-turn timer is
  `end_of_turn_punct_ms` when the final ends in `. ? !`, else
  `end_of_turn_ms`; an onset cancels it; empty finals are ignored; the
  marker is `[spoken, N s]` with `N = ceil((last close − first onset)) ≥ 1`,
  or `[spoken, N s, interrupted]` after a barge-in. Partials only become
  `Heard{final: false}`.
- Barge-in (§0.6, §4.4): an onset in `Speaking` opens a `Candidate` and
  starts `Hold` (`barge_in_min_ms`); a close before it fires drops the
  candidate (a cough). On `Hold`: `Pause` (device sink, `hold: pause`),
  `Holding{since_speech_ms}`, state `Holding`, then decide on the first
  non-empty partial/final or on `Decide` (900 ms after onset, no text →
  real). Backchannel: every token — or listed multi-word phrase — is in
  `backchannels` (lowercased, punctuation stripped). Echo: the normalised
  candidate matches, at Levenshtein ratio ≥ `echo_ratio`, a same-length
  word window of the playing sentence plus the one before it (~2 s), logged
  at `debug` with both strings. Backchannel/echo → `Resume(reason)`, state
  `Speaking`, the candidate utterance's later text is not turn text
  (`discard`); an empty final while holding also resumes. Real → `Commit`.
- The five cuts (§4.5), in `commit`: heard prefix from the ledger →
  `StopSpeech` (actor: `Flush` event for the client sink, then `Voice::Stop
  {None}` — which also drops the `Player`, i.e. flush + stop in one
  message), `Cancel(Some(heard sentences joined by spaces))`, the gate is
  left open (no `Utterer::reset`), ledger closed (`turn_done`, unheard
  sentences `Dropped`), `Interrupted{heard, dropped}`, state `Listening`, the
  candidate's onset is the next turn's start, `interrupted = true`.
- Ledger (§4.6): `Sentence{text, speech, status: Queued | Playing | Played |
  Dropped, last_seq}`. Heard prefix, device sink: the leading run of `Played`
  sentences, cut at the sentence `Played.id` names (the one the player was
  inside when it paused/flushed). Client sink: `remote_heard`, advanced by
  `played_seq` — a `Played` sentence is heard when `seq ≥` its last
  published packet, or when the client's seq drops (it moved on).
- §0.9, gated on a transcript: a `Hold` while `Thinking` waits for the
  candidate's first text (an empty final drops it); the text →
  `StopSpeech`, `Cancel(None)`, ledger cleared, `Listening`; the new
  utterance is then submitted **on its own** as `[spoken, N s] <new>` (see
  §4: the cancelled input's user row stays in history). The agent's
  `TurnFinished{cancelled}` for our own cancel is ignored (`cancelled`
  flag), as is `TurnFinished{interrupted}` after a barge-in.
- Noise rules (from the smoke, §2.3): an empty final with nothing pending
  restarts the turn clock (`turn_start = None`), so `[spoken, N s]` counts
  speech, not the silence before it; a held candidate that closes with an
  empty final resumes playback (`Resumed{Backchannel}`) instead of
  committing — only *open and silent at the 900 ms deadline* is real.
- Speaking (§4.8): one `markdown::Text` per block run; `Delta` pushes and
  speaks every newly committed block (a closed paragraph is spoken while the
  next streams); `BlockEnd`/`TurnFinished` finish the tail. Each block →
  `markdown::spoken` → lines → `sentences()` (split after `. ? !` +
  whitespace) → one `Speak{queue: true}` each. `TurnFinished{cancelled |
  interrupted | error}` from elsewhere → `StopSpeech`, ledger closed,
  `Listening`. `Listening` when the turn is done and nothing is `Queued |
  Playing`; an empty reply goes straight back.

### 1.2 `crates/talk/src/lib.rs` — the actor

- `pub use voice::Sink; pub use voice::mic::Aec;` (brief step 1) — the
  contract's local enums are gone; wire stays `"client" | "device"`, `"vpio"
  | "none"` (see §4 for the voice-side change that made this possible).
- `Talk { rt, cfg, active: Option<Live> }`, `Talk::new(rt, cfg)`. `Live { id,
  session, sink, aec, policy, cancel: Arc<AtomicBool>, remote:
  Option<Remote>, timers: [u64; 3], paused, ending: Option<EndReason> }`.
- `StartTalk`: "already talking" while one runs (also while it is ending);
  "voice is disabled" without the `voice`/`dictation` actors; `Dictation::
  Lease` (waits for a model load); `Utterer::set_rate` (48 kHz mic, or the
  client's rate); `Sink::Device` → `Capture::open_with(Vpio | None per
  talk.aec)`; `Sink::Client` → the remote PCM inbox (dictation's `Remote`
  shape, 20 ms chunks); on failure the lease is `Return`ed. Then the audio
  task, `TalkStarted`, `Degraded{MicNoAec}` (device sink with `Aec::None`),
  `TalkState{Listening}`. The speaker itself is opened by `Voice` on the
  first device-sink `Speak` (VOICE-OUT's lifetime).
- Audio task (`spawn_blocking`, the dictation `Task` shape): every 20 ms
  chunk → RMS (`Level` every 8), `Utterer::chunk`; gate edges are read off
  `Utterer::is_open()` before/after the chunk (no accessor added to
  `live.rs`); `Onset`/`Close` are told before the chunk's texts. Ends on the
  cancel flag, actor death or a source error; `close()` flushes, `reset()`,
  the engine goes back with `Finished`.
- Timers: `Action::Timer` bumps a per-kind generation and spawns a sleep
  that tells `Fired{id, timer, generation}`; stale generations are dropped.
- `Action` execution (`Talk::run`): a `VecDeque` so `Commit` can splice the
  five cuts in front; `Speak` → `Voice::Speak{session, text, queue: true,
  sink}` → `policy.spoken`; `Submit` → `Agents::SendInput{session, text,
  delivery: NextTurn, id: None}` + `Submitted`; `Cancel` →
  `Agents::Cancel{session, keep}`; `Pause` → `Voice::Pause` →
  `policy.paused`; `Resume` → `Voice::Resume` if paused + `Resumed`;
  `Commit` → `Voice::Position` when the device sink is not paused.
- Subscriptions: `AgentEvent` (`TurnStarted`, `TextDelta`,
  `ToolCallPending`, `TurnFinished` of the talk's session) and `VoiceEvent`
  (`SpeechStarted`, `SpeechFinished`, and `SpeechAudio` for the client sink)
  via `rt.subscribe`, `subscriber_mailbox`. A turn typed elsewhere during
  the talk (`TurnStarted` while `Listening`) is spoken like ours, barge-in
  included.
- `StopTalk`, `CloseTree` (`RegisterLifecycle{owner: "talk"}`) → `end
  (reason)`: cancel flag, timers invalidated, `Voice::Stop{None}`,
  `TalkState{Idle}`; the task's `Finished` then `Return`s the lease and
  publishes `TalkFinished{reason}`. `TalkStatus.active` reports the live
  talk (`Idle` while ending).
- `PushAudio`: rechunks the client's i16 PCM; `played_seq` → `Input::PlayedSeq`.

### 1.3 Outside `crates/talk`

- `crates/rpc/src/api.rs` `talk_start`: the connection-close hook stops
  the talk for **any** sink (was: client only) — §0.10 binds the talk to
  the connection that started it, and a device talk holds the mic.
- `crates/theuth/src/main.rs`: `"talk"` in the lifecycle `owners` list
  (TALK-CONTRACT's follow-up).

## 2. Verified (this Mac, 2026-09-28, toolchain `stable-aarch64-apple-darwin`)

### 2.1 Unit tests — `cargo test -p talk`

```
test policy::tests::end_of_turn_timing_with_and_without_punctuation ... ok
test policy::tests::classifier_backchannel_echo_real ... ok
test policy::tests::heard_prefix_from_ledger_and_position ... ok
test policy::tests::barge_in_commits_the_five_cuts_and_marks_the_next_input ... ok
test policy::tests::backchannel_resumes_and_its_final_is_not_text ... ok
test policy::tests::hold_none_keeps_playing_until_the_decision ... ok
test policy::tests::speech_while_thinking_cancels_and_submits_the_new_text ... ok
test policy::tests::state_transitions_of_4_2 ... ok
test policy::tests::a_paragraph_is_spoken_when_it_closes_not_at_turn_end ... ok
test policy::tests::typed_turn_is_spoken_and_noise_does_not_age_the_clock ... ok
test tests::config_defaults ... ok
test tests::wire_shape ... ok
test result: ok. 12 passed; 0 failed
```

The brief's five: end-of-turn timing (700 / 250 ms, onset cancels, empty
finals ignored, `[spoken, 3 s]` marker), backchannel / echo / real
(`"Yeah."`, `"mm-hmm, okay"`, `"I see"` vs `"yeah but wait"`; `"tests were
skip"` is echo of `"Two tests were skipped on the way."` at ratio 0.83,
`"stop, that is wrong"` is not), heard prefix from a ledger + `Played` (device:
cut at `Played.id`, leading run when idle; client: `played_seq` incl. the
seq-drop rule), the §0.9 path, and the §4.2 transitions (Speaking → Listening
on the last `Done`; tool round keeps `Thinking`; a foreign `cancelled` stops
the queue; empty reply → `Listening`), plus the five cuts in order with
`Cancel(Some("One."))` / `Interrupted{1, 2}` / `[spoken, 1 s, interrupted]`.

### 2.2 Gates

- `cargo test -p talk -p voice -p rpc`:
  ```
  rpc:   test result: ok. 35 passed; 0 failed   (+ tests/prompts.rs: ok. 1 passed)
  talk:  test result: ok. 12 passed; 0 failed
  voice: test live::tests::utterer_returns_the_spoken_sentence_punctuated ... FAILED
         (live.rs:336, the final `.` missing — the flake VOICE-OUT §2.2 recorded;
          drives the real Nemotron model with `say`; untouched here)
  cargo test -p voice --lib (rerun): test result: ok. 24 passed; 0 failed; finished in 10.70s
  ```
- `cargo clippy -p talk --all-targets` → `Finished dev profile … in 48.77s`,
  no warnings (`clippy::all = deny`, `missing_docs = warn` in the workspace
  lints).
- `just ts-desktop` → `export_bindings` ran; `bindings/Sink.ts` is now one
  file, `export type Sink = "client" | "device";`, and `bindings/Aec.ts`
  `export type Aec = "vpio" | "none";` — the contract's wire shape, from
  `voice`'s types. (`bindings/` and `crates/desktop/ui/src/rpc/bindings/`
  are gitignored; nothing to commit.)
- `cargo check --workspace --tests` → `Finished dev profile … in 17m 55s`;
  only the pre-existing `block v0.1.6` future-incompat note.
- `cargo fmt -p talk` clean.

### 2.3 Live smoke — `THEUTH_DATA_DIR=/tmp/talk-TALK-CORE-data`

Setup: `models` symlink + `~/.theuth/config.yaml` copied; the vault
(`secrets.sqlite`) copied with `secrets.key_store: file` and the keychain
key exported to `secrets.key` (the keychain item is bound to the absolute
data dir, so the copy could not decrypt it otherwise); slack/signal/link/
schedule/mesh disabled; `rpc.listen 127.0.0.1:7793`. Daemon:
`./target/debug/t --headless` (debug build — the TTS is far slower than
release, see §3). Driver: two bun WebSocket scripts (throwaway, in the temp
dir): `smoke.ts` (device sink) and `smoke_client.ts` (client sink, pushes
`say -o … LEI16@16000` PCM in 100 ms `talk_audio` calls with a pretend
player's `played_seq`, 80 ms per packet).

**Client sink, the full loop with a barge-in** (`smoke_client.ts`, times
since connect; `TextDelta` printed inline):

```
[  1.73s] TalkStarted {"sink":"client","aec":"none","since_ms":0}
[  1.73s] TalkState {"state":"listening"}
[  3.23s] >>> speaking "What time is it right now? Answer in three sentences."
[ 11.23s] Heard {"utterance":0,"text":"What time is it","final":false}
[ 13.98s] Heard {"utterance":0,"text":"What time is it right now? Answer in three sentences","final":true}
[ 14.68s] Submitted {"text":"[spoken, 4 s] What time is it right now? Answer in three sentences","interrupted":false}
[ 14.68s] TalkState {"state":"thinking"}
[ 14.69s] TurnStarted {"text":"[spoken, 4 s] What time is it right now? Answer in three sentences","model":"openai/gpt-5.6-sol"}
It is 10:04 p.m. Right now it is Monday, September 28, 2026. Your time zone is UTC plus one.
[ 18.84s] TurnFinished {"stop_reason":"completed"}
[ 18.84s] VoiceModel {"state":"loading"}   … [ 20.34s] ready
[ 29.44s] SpeechStarted {"id":"…6ad9…","chars":16,"sink":"client"}
[ 29.44s] Saying {"speech":"…6ad9…","text":"It is 10:04 p.m."}
[ 29.44s] TalkState {"state":"speaking"}
[ 31.94s] >>> speaking "Stop, wait. Tell me the date instead."
[ 38.44s] Holding {"since_speech_ms":8997}
[ 38.44s] TalkState {"state":"holding"}
[ 38.85s] Heard {"utterance":1,"text":"Stop","final":false}
[ 38.85s] Flush {}
[ 38.85s] SpeechFinished {"id":"…6e92…","reason":"stopped"}  ×3 (the three queued sentences)
[ 38.85s] Interrupted {"heard_sentences":0,"dropped_sentences":3}
[ 38.85s] TalkState {"state":"listening"}
[ 40.99s] Heard {"utterance":1,"text":"Stop, wait. Tell me the date instead","final":true}
[ 41.69s] Submitted {"text":"[spoken, 3 s, interrupted] Stop, wait. Tell me the date instead","interrupted":true}
[ 41.70s] TurnStarted {"text":"[spoken, 3 s, interrupted] Stop, wait. Tell me the date instead"}
It’s Monday, September 28th, 2026.
[ 55.99s] Saying {"text":"It’s Monday, September 28th, 2026."}
[ 71.73s] talk_stop null → TalkState idle → TalkFinished {"reason":"stopped"}
--- transcript (4 entries) ---
user:      [spoken, 4 s] What time is it right now? Answer in three sentences
assistant: It is 10:04 p.m. Right now it is Monday, September 28, 2026. Your time zone is UTC plus one.
user:      [spoken, 3 s, interrupted] Stop, wait. Tell me the date instead
assistant: It’s Monday, September 28th, 2026.
```

Reading: stage 1 held 6.5 s after the barge-in audio began to be pushed
(the debug-build STT window; the hold itself is `barge_in_min_ms` after the
gate opened), stage 2 committed on the first partial 410 ms later, the five
cuts landed in one handler tick (`Flush`, three `Stopped`, `Interrupted`,
`listening` all at 38.85 s), and the next input carried `interrupted`.
`heard_sentences: 0` is right for this run: the first sentence's synthesis
(debug build) was still trickling packets when the cut came, so its last
packet had never been published, let alone played. The kept prefix in
*history* is the whole first reply — see §3 (the model had finished
streaming 10 s before the first audio, so `Cancel{keep}` landed on an idle
session).

**Device sink** (`smoke.ts`, `sink: device`; the turn typed with
`send_input` because the room mic did not pick up `say` — see §3):

```
[  2.43s] TalkStarted {"sink":"device","aec":"vpio","since_ms":0}
[  5.43s] >>> send_input "What time is it? Answer in three short sentences."
[  5.45s] TurnStarted … → TalkState {"state":"thinking"}
It is 10:05 PM. Today is Monday. The date is September 28, 2026.
[ 17.52s] SpeechStarted {"id":"…7c63…","chars":15,"sink":"device"}
[ 17.52s] Saying {"text":"It is 10:05 PM."} → TalkState {"state":"speaking"}
[ 24.68s] Holding {"since_speech_ms":7154} → TalkState {"state":"holding"}
[ 25.18s] SpeechFinished {"reason":"stopped","sink":"device"} ×3
[ 25.18s] Interrupted {"heard_sentences":0,"dropped_sentences":3} → TalkState {"state":"listening"}
[ 25.37s] Heard {"utterance":4,"text":"That's","final":true}
[ 26.95s] Heard {"utterance":5,"text":"right. County death has been","final":true}
[ 27.65s] Submitted {"text":"[spoken, 3 s, interrupted] That's right. County death has been","interrupted":true}
```

daemon log: `voice: speaker open device=Aggregate Device rate=48000
channels=2` at each device-sink reply (the `Player` opened on the first
`Sink::Device` utterance and closed on the talk's `Stop{None}`).

Reading: the speaker path runs (`SpeechStarted{sink: device}`, the Player
opened, `SpeechFinished{done}` for the first sentence in an earlier run at
28.37 s); the mic path runs under VPIO (`aec: vpio`); the state machine
follows a typed turn. The interruption here was a *false* cut-in: the gate
opened on something (the `say` output or the agent's own echo through the
BRIO webcam mic), no partial had arrived by the 900 ms deadline (debug-build
STT latency: the first partial came 200 ms *after* the deadline) and §4.4
says silent-at-deadline is real. The transcript then received a garbled
echo. This is the design's rule doing what it says on a host where VPIO does
not cancel the echo (the default output is an aggregate device; the mic is a
USB webcam) — the `MicNoAec` situation without the report.

An earlier device run also exercised **§0.9 live**: a partial arrived while
`Thinking` → `TurnFinished{cancelled}` for the in-flight turn, the new text
went in as its own `[spoken, 1 s]` input (that run's history showed the
cancelled input's user row kept beside the resubmitted one — the reason the
merge was dropped, §4).

## 3. Not verified

- **"Say 'what time is it' into the room and hear the reply"**: not
  possible from this seat. `say` through the MacBook speakers was not
  recognised by the default input (a Logitech BRIO); `say` on the default
  output is what VPIO cancels. The STT path was proven with the same audio
  pushed through the client sink, and the speaker path with a typed turn.
  No ear confirmed the speaker output (headless run; the Player consumed the
  frames and `SpeechFinished{done}` arrived).
- **Release-build latencies.** The daemon was a debug build: Pocket TTS ran
  slower than real time (first audio 10–15 s after the model finished;
  `since_speech_ms: 8997` for a 16-char sentence), and the STT partial came
  ~1.1 s after onset instead of ~0.3 s. §8's numbers need `--release`.
- **`heard_sentences ≥ 1` end to end.** Every barge-in in the smokes came
  before the first sentence's last packet (debug TTS); the ledger arithmetic
  is unit-tested for both sinks.
- **`Cancel{keep}` truncating history.** The model finished streaming
  before any audio in every run (short replies + slow debug TTS), so the
  cancel found no turn in flight and history kept the whole reply — the
  case 76 §7 leaves as is. AGENT-KEEP's actor test covers the in-flight case.
- **Backchannel / echo resumes live** (`Resumed` events): classifier
  unit-tested; no live corpus (§10 step 7).
- Windows/Linux (`Aec::None` → `Degraded{MicNoAec}`), the mesh, `CloseTree`
  ending a talk (handler is four lines; not driven live), a second
  `talk_start` → "already talking" (guard is the first line of the handler).

## 4. Deviations and decisions

- **`voice::Sink` / `voice::Aec` lost their serde `tag`** (`{"sink":
  "device"}` → `"device"`, `{"aec": "vpio"}` → `"vpio"`). The brief asks to
  re-export them *and* keep the contract's wire shape; VOICE-OUT had given
  them a tagged shape, and the two `Sink` types were both exporting
  `bindings/Sink.ts` (flat directory — whichever ran last won). Design §5.1
  spells `sink: "device"`, so the voice side moved. `VoiceEvent::
  SpeechStarted/SpeechFinished.sink` change with it; no frontend read that
  field yet (grep of `desktop/ui`, `tui`).
- **Flush = `Voice::Stop{None}`** for the device sink. `Voice` exposes no
  `Flush` message; `Stop{None}` clears the queue, stops the current
  utterance and drops the `Player` (device closed → silent within its
  callback), and VOICE-OUT documents that as the intended "talk's last
  word". The heard-prefix position comes from the hold's `Pause` reply, or
  `Voice::Position` when `hold: none`. The cost is a device re-open on the
  next reply (VOICE-OUT §4).
- **Heard prefix, device sink** is the leading `Played` run cut at
  `Played.id`, not the Player's entry ledger (not reachable through
  `Voice`). Ceiling (`ponytail:` note in `Ledger::heard`): a sentence
  shorter than the 200 ms ring can be `Played` (all samples pushed) while
  the cursor is still in the gap before it and `Played.id` is `None`; it
  would count as heard.
- **Client-sink heard prefix** uses `played_seq` against the last
  `SpeechAudio.seq` per sentence plus a "seq dropped → moved on" rule; the
  client does not name the utterance its seq belongs to.
- **Sentences, not blocks.** `markdown::spoken` emits one *block* per line
  (a paragraph of three sentences is one line); the design's "sentence per
  line" claim does not hold, so `policy::sentences` splits after `. ? !` +
  whitespace. No abbreviation list (`ponytail:` note).
- **`Saying` at `SpeechStarted`** for both sinks: the Player does not
  report "first frame left the ring" as an event.
- **`Speaking` on `SpeechStarted`** (§4.8), not on the first `Speak` (the
  §4.2 diagram): a `Hold` before any audio takes the §0.9 path (nothing was
  heard), which is what the queued-but-not-started case is.
- **`Level` is `[f32; 8]`** as the contract's `TalkEvent::Level` says.
- **Cancel token = `Arc<AtomicBool>`**, not `tokio_util::CancellationToken`
  (no new dependency on `talk`; the task polls every 50 ms anyway).
- **A model failure under the lease** leaves `Dictation.leased = true`
  (there is no "lease lost" message); logged as a warning, follow-up.
- **`StopTalk` does not `Cancel` the agent turn**: the reply still lands
  as text in the transcript (§4.1 lists speech, task, lease only).
- The prompt block stays a `const` (nothing in it depends on config).
- **§0.9 without the merge, and gated on text.** The design says "Cancel
  {keep: None} and resubmit the previous text plus the new utterance as one
  input". Live, `Cancel` leaves the cancelled input's *user row* in the
  store (turn_seq 2 stayed beside the resubmitted row in the 21:51 run), so
  merging duplicates the text the model already sees; the new utterance
  goes in alone and reads as a continuation. And the cancel waits for the
  candidate's first partial: the first device run lost a whole turn to a
  noise the VAD opened on for 400 ms while the TTS model was loading
  (`TurnFinished{cancelled}`, "cancelled by user" tool result, nothing to
  resubmit).
- **A held candidate that closes empty resumes** (`Resumed{Backchannel}` —
  the enum has no `Noise`); only "still open and silent at 900 ms" commits
  as §4.4 says. Seen live: a 200 ms noise held and then cut the second reply
  at 42.05–42.41 s in the last device run, before this rule.
- **`[spoken, N s]` counts from the first onset that led to text**: an
  empty final with nothing pending resets the clock (the 21:51 run produced
  `[spoken, 41 s]` from noise onsets minutes earlier).
- **A typed turn during a talk is spoken** (`TurnStarted` → `Thinking`),
  so the state machine and barge-in cover it; the design does not say, and
  the alternative (the agent speaking while the talk sits in `Listening`,
  every echo a new turn) was what the first device runs did.

## 5. Outside the allow-list

- `crates/voice/src/lib.rs` (1 line), `crates/voice/src/mic.rs` (1 line):
  `#[serde(rename_all = "snake_case")]` without `tag` on `Sink` / `Aec`.
- `crates/theuth/src/main.rs` (1 word): `"talk"` in `owners`.
- No accessor was needed in `crates/voice/src/live.rs` or `out.rs`.

## 6. Follow-ups

- `Voice`: a `Ledger`/entries query so the device heard prefix is exact
  for sub-ring sentences; a `Dictation::Lost` for a model failure under a
  lease.
- The classifier corpus check (§10 step 7) and `hold: none` vs `pause`
  numbers.
- An integration test with fake `Voice` and `Agents` actors (brief step 9,
  "welcome if it fits") — not written; the policy tests cover the decisions,
  the smoke covers the wiring.
- Frontends: `TurnFinished{interrupted}` renders as an ordinary finish
  (AGENT-KEEP's note); the `[spoken … interrupted]` line and the kept
  assistant row are what the transcript shows.
- ~~**Short replies are never truncated.**~~ Closed by KEEP-AFTER-TURN
  (`docs/design/76-as-built-KEEP-AFTER-TURN.md`): when the ledger's turn has
  finished, the commit sends `agent::TrimAssistant { turn, keep }` instead
  of `Cancel { keep }` and the last assistant row is rewritten to the heard
  prefix (76 §7, the post-turn case).
- **The 900 ms decide deadline vs. STT latency.** In the debug build the
  first partial arrived after the deadline and a noise cut the agent. Either
  measure the release-build partial latency (§10 step 7) or make the
  deadline `barge_in_min_ms + one STT window + margin`; a `talk.decide_ms`
  knob would be the cheap form.
- **Echo through a mic VPIO does not cover** (USB webcam mic + aggregate
  output here): the agent hears itself garbled ("Today it is my day the…"
  for "Today is Monday, September 28."). `echo_ratio` can be lowered per
  host; a looser echo test (word-overlap on the playing sentence) is the
  next lever before §10 step 9's software AEC.
- `Heard{text: "", final: true}` is published for every empty utterance
  (dictation does the same); a noisy room produces one every few seconds.
  Frontends drop empty finals, or `talk` stops publishing them.
