# 76 — as built: INTEGRATE (the closer before the squash)

Status: landed 2026-09-28/29 on `talk/integrate` (base `feature/talk` @
f68b4660 — all six design-76 workers merged, main c0e574bf merged in).
Commits (new only, nothing rewritten): `33c84de3` rename, `c1c29dad`
follow-ups, `a99d22c3` mirror + debug log, `b08dc65a` A2aChanged mirror bug,
plus this doc and the §13 paragraph.

## 1. What was built

### 1.1 Wire collision — `TalkEvent::Level` → `TalkEvent::TalkLevel`

| Where | Change |
|---|---|
| `crates/talk/src/lib.rs` | variant renamed; doc says why (`MeetingEvent::Level` shares the UI stream); publish site follows |
| `crates/tui/src/talk.rs` | reducer, `event_id`, the `sim` script and its test |
| `crates/desktop/ui/src/components/meeting/Island.tsx` | the `'mic_rms' in e` guard is gone; `on('Level')` is the meeting meter again |
| `bindings/`, `crates/desktop/ui/src/rpc/bindings/` | regenerated (`just ts-desktop`); `TalkEvent.ts` carries `TalkLevel`, `VoiceEvent.ts` carries `sink` on `SpeechStarted`/`SpeechAudio`/`SpeechFinished` |

**Tag is `TalkLevel`, not `talk_level`.** The brief wrote "(tag
`talk_level`)"; every `TalkEvent` variant (and every other bus event) uses
the variant name as its `type` tag (`TalkStarted`, `TalkState`, …). A
`#[serde(rename)]` on one variant would be the only snake_case tag on the
stream. The desktop never listened to the level (no meter, §0.12), so no
listener changed there.

### 1.2 Cross-worker follow-ups resolved

| From | Item | Done |
|---|---|---|
| DESKTOP-TALK | `SpeechStarted.sink` is real now (VOICE-OUT/TALK-CORE) | `speech.ts` talk leg claims `SpeechStarted` only for `sink === 'client'`; stale comment rewritten; `speech.test.ts` gains a device-sink utterance on the talk's own session that is *not* played; `talk.test.ts`/`talk-sim.ts` fixtures carry `sink` (the bindings require it) |
| TALK-CORE | `Dictation.leased` stuck when the model dies under the lease | `voice::Return(pub Option<Box<Utterer>>)`: `None` frees the slot and `set_model(Missing)` — the same path a dictation's own failure takes; `talk` sends `(*engine).map(Box::new)` from `Progress::Finished` and `Some(engine)` from the refused-start path. The `ponytail:` warn is gone |
| TALK-CORE | `Heard{text: "", final: true}` published for every empty utterance | `policy::text` pushes `Action::Heard` only for non-empty text; the turn logic below it is unchanged (empty finals still reset the clock) |
| TALK-CORE | 900 ms decide deadline vs STT latency | `talk.decide_ms` (default 900) in `TalkConfig`, used by `policy::pause`; `DECIDE_MS` const removed; mirrored in `DEFAULT_CONFIG` (`crates/theuth/src/main.rs`, one comment line). Observed need in the smoke: every barge-in committed on the deadline with no partial yet (debug build, STT window ≈ 1 s) |
| TUI-TALK | Esc leaves the view only; §5.2 says Esc during Speaking is the agent's cancel | `Model::talk_escape()`: `Thinking`/`Speaking`/`Holding` → `Command::Cancel(talk.session)`, view stays (the talk returns to `Listening` on `TurnFinished`); otherwise leaves the view. Header hint and `/talk` help text updated; covered in `talk_view_lifecycle` test |
| TUI-TALK | `/talk` while `awaiting_session` creates a second session | `toggle_talk` is a no-op while `session.is_none() && awaiting_session` |
| TUI-TALK (merge fallout) | `crates/tui/src/talk.rs` sim built `SpeechFinished` without `sink` — `cargo check --workspace --tests` failed on `feature/talk` | field added (`Sink::Device`) |
| AGENT-KEEP / TALK-CORE | frontends treat `stop_reason: "interrupted"` as an ordinary finish | **Left as is, deliberately.** An interruption is a normal conversational event; the transcript already shows `[spoken, N s, interrupted]`. Rendering it like `cancelled` (TUI `AgentState::Cancelled` + "↳ … cancelled" note, desktop `cancelled` row state + note) would add noise to every barge-in. Nothing to change |
| DESKTOP-TALK / VOICE-OUT / TALK-CONTRACT | stale statements ("no `sink` yet", tagged `Sink`/`Aec`, "TUI ignores events", stub handlers) | Superseded by sibling branches; their docs are historical records and were not edited. The integrated truth: `Sink`/`Aec` are flat (`"device"`), `SpeechStarted`/`SpeechAudio`/`SpeechFinished` carry `sink`, the TUI routes `UiEvent::Talk` to `apply_talk`, `crates/talk` has no stubs |

### 1.3 Found and fixed in the live smoke (not in any as-built)

- **An attached TUI never received `TalkEvent`s (nor `VoiceEvent`s).**
  `rpc::client::mirror` re-publishes the daemon's `subscribe_events` JSON as
  bus types by trying each type in turn. Two faults:
  1. `talk::TalkEvent` was not in the list (`crates/rpc/src/client.rs`,
     `republish!`). Added after `voice::VoiceEvent`.
  2. `a2a::A2aChanged {}` is an internally tagged struct with no fields; serde
     does not check the tag on structs, so it deserialised **every** object —
     every event after it in the list (`signal`, `slack`, `knowledge`, …,
     `voice`, `meeting`, `talk`) was published as `A2aChanged`. Proven with a
     temporary `eprintln!` in the TUI: `MIRROR "TalkStarted" -> a2a::A2aChanged`.
     Now matched by `v["type"] == "A2aChanged"` before the typed list. This is a
     pre-existing bug on main for every attached frontend (dictation in an
     attached TUI could not have worked); fixed here because the TUI talk view
     depends on it. Outside talk's own files but inside the allow-list (`crates/rpc`).
- `crates/talk/src/lib.rs` `Talk::run`: one `tracing::debug!(?action, "talk")`
  per policy action — `RUST_LOG=talk=debug` now shows the state machine and
  the five cuts in order (the brief asked for these lines; the crate had no
  transition logging).

## 2. Verified

Toolchain: `export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$HOME/.bun/bin:/opt/homebrew/bin:$PATH"`.

### 2.1 Gate (a) — TS and desktop

- `just ts-desktop` → `test result: ok` (export), bindings copied;
  `bindings/TalkEvent.ts` has `TalkLevel` (1 hit), `VoiceEvent.ts` has `sink`.
- `crates/desktop/ui`: `bun install --frozen-lockfile` → `616 packages installed`;
  `packages/apps`: `bun install --frozen-lockfile` → `484 packages installed`
  (needed first — without it `tsc` reports 20 errors inside `packages/apps`,
  as DESKTOP-TALK's "outside the allow-list" note says).
- `bunx tsc --noEmit` → exit 0 (after the `sink` fixtures; before them 4 errors
  in `talk-sim.ts`/`talk.test.ts`, the regenerated bindings made `sink` required).
- `bunx vite build` → exit 0.
- `CI=1 bunx vitest run < /dev/null > /tmp/talk76/vitest.log` →
  `Test Files  113 passed (113)`, `Tests  645 passed (645)`, exit 0
  (`mobile-zoom.test.ts` did not time out this run).

### 2.2 Gate (b)/(c) — Rust

- `cargo check --workspace --tests` → exit 0 (first run failed:
  `error[E0063]: missing field 'sink' in initializer of 'VoiceEvent'`
  at `crates/tui/src/talk.rs:234` — merge fallout, fixed in `33c84de3`).
- `cargo clippy --workspace --all-targets` → exit 0, no `warning:`/`error:`
  lines (only cargo's `block v0.1.6` future-incompat note).
- `cargo build -p theuth` → `Finished dev profile` (16 m cold, 1–5 m incremental).

### 2.3 Gate (d) — `cargo test -p talk -p voice -p agent -p rpc -p tui -p meeting --no-fail-fast`

Run after the smoke (the daemon and the tests did not share the CPU); tally in §2.6.

### 2.4 Live smoke — `THEUTH_DATA_DIR=/tmp/talk-INTEGRATE-data`

Setup: `models` symlink, `~/.theuth/config.yaml` copied and edited
(`rpc.listen 127.0.0.1:7797`, slack/link/signal/schedule/mesh off,
`secrets.key_store: file`, voice/talk on); `~/.theuth/secrets.sqlite` copied
with `sqlite3 .backup`; the vault key exported from the keychain
(`security find-generic-password -s theuth -a "secrets:/Users/Sky.Moore/.theuth" -w > secrets.key`,
0600). One trap: this shell inherits `THEUTH_RPC__LISTEN=127.0.0.1:64708`
from the live app, which overrides the config — the daemon has to be started
with `THEUTH_RPC__LISTEN=127.0.0.1:7797` explicitly, else "Address already in use".
Daemon: `RUST_LOG=info,talk=debug THEUTH_RPC__LISTEN=127.0.0.1:7797 THEUTH_DATA_DIR=/tmp/talk-INTEGRATE-data ./target/debug/t --headless`
(debug build: TTS first audio 7–10 s after the reply, STT partial ≈ 1 s).

Devices: default input Logitech BRIO, default output "Aggregate Device"
(LG monitor); `aec: vpio`. **Nobody spoke into the microphone** — the
utterances were `say` played through the *MacBook Pro Speakers* (`say -a`),
a device VPIO does not use as its echo reference, so the BRIO heard them as
room audio. Played through the default device instead, VPIO cancels `say`
along with the agent and only garbage reaches STT ("What kind of books in
the Greek work did") — an environment artefact, not a talk bug.

**(i) TUI `/talk`** — `python3 /tmp/talk76/tui_drive.py ./target/debug/t`
(pty 110×32, `/talk`, `say` at +12 s, `say` over the reply at +42 s, Esc at
+80 s, Ctrl+T at +84 s). Screen after Esc (ANSI-stripped):

```
│ ◆ you                                                                     │
│ [spoken, 3 s] What are the resources in                                   │
Resources in what exactly?
│ ◆ you                                                                     │
│ [spoken, 3 s, interrupted] Wait till they                                 │
Okay, I’ll wait.
 cd306075  idle  18643↑ 10↓  ·  ● talk: listening (/talk)  ·  attached · pid 47856 · desktop
```

and after Ctrl+T: `● talk: idle (/talk)` then the talk is gone. While in the
view: the speaking orb (`●` core, 9 747 cells over the run), agent subtitle
`Resources in what exactly?` and user partials `What are the resour…`,
`Wait till` centred beneath it. Daemon actions (`RUST_LOG=talk=debug`):

```
00:01:38.831 [talk::Report]      Heard { utterance: 1, text: "What are the resources in", is_final: true }
00:01:39.534 [talk::Fired]       Submit { text: "[spoken, 3 s] What are the resources in", interrupted: false }
00:01:39.534 [talk::Fired]       State(Thinking)
00:01:48.079 [agent::AgentEvent] Speak { index: 0, text: "Resources in what exactly?" }
00:01:55.791 [voice::VoiceEvent] Saying { speech: …61a2, text: "Resources in what exactly?" }
00:01:55.791 [voice::VoiceEvent] State(Speaking)
00:01:58.688 [talk::Fired]       Pause
00:01:58.688 [talk::Fired]       Holding { since_speech_ms: 2897 }
00:01:58.688 [talk::Fired]       State(Holding)
00:01:59.189 [talk::Fired]       Commit
00:01:59.189 [talk::Fired]       StopSpeech
00:01:59.189 [talk::Fired]       Cancel(Some(""))
00:01:59.190 [talk::Fired]       Interrupted { heard: 0, dropped: 1 }
00:01:59.190 [talk::Fired]       State(Listening)
00:02:07.683 [talk::Report]      Heard { utterance: 4, text: "Wait till they", is_final: true }
00:02:08.386 [talk::Fired]       Submit { text: "[spoken, 3 s, interrupted] Wait till they", interrupted: true }
```

The five cuts land in one handler within 1 ms (Commit → StopSpeech →
Cancel → Interrupted → Listening). The speaker went silent on `StopSpeech`
(`SpeechFinished{stopped, sink: device}` ×1 on the bus) — not verified by
ear; nobody was in the room.

**(ii) Desktop** — `VITE_THEUTH_URL=ws://127.0.0.1:7797 VITE_THEUTH_TOKEN=… bunx vite --port 5187`,
headless Chromium (WebGPU orb rendered). Rail **Talk** → new session,
`/talk/01a0ea64-…`, talk started (`Talk (running)`, live dot, **Stop
talking**). `say` → `[spoken, 4 s] What time is it? Entering three short
presentations` → orb `speaking` with subtitle **It's 12:41 a.m.**
(in-app screenshot taken at `State(Speaking)` + 1.2 s); `say` over it →
`Holding{since_speech_ms: 3977}` → `Interrupted{heard: 0, dropped: 2}` →
orb `idle`, agent line cleared, user partial **Wait, tell me** (screenshot at
`Interrupted` + 0.8 s). Transcript pane (⌘⇧T, `/tmp/talk76/desktop-talk-transcript.png`):

```
[spoken, 4 s] What time is it? Entering three short presentations
It’s 12:41 a.m. on Tuesday, September 29th.
[spoken, 3 s, interrupted] Wait, tell me the date and
It’s Tuesday, September 29th, 2026, at 12:41 a.m.
```

**Stop talking** → `Start talking`, `talk_status.active: null`. Full-screen
`screencapture -x` files (`/tmp/talk76/orb-*.png`) show the user's desktop,
not the headless tab; the in-app screenshots are the visual evidence.

**Client sink (remote leg), the deterministic run** — `bun smoke_client.ts`
(TALK-CORE's script, port changed; `say -o` PCM pushed via `talk_audio`
with a pretend player's `played_seq`), log `/tmp/talk76/smoke_client.log`:

```
[  2.41s] TalkStarted {sink:"client",aec:"none"} → TalkState listening
[ 13.25s] Heard {utterance:0, text:"What time is it right now? Answer in three sentences", final:true}
[ 13.95s] Submitted {text:"[spoken, 4 s] What time is it right now? Answer in three sentences"}
It’s 12:20 AM. It’s Tuesday, September 29, 2026. Your time zone is UTC plus one.
[ 28.03s] SpeechStarted {sink:"client"} · Saying {text:"It’s 12:20 AM."} · TalkState speaking
[ 30.54s] >>> speaking "Stop, wait. Tell me the date instead."
[ 36.42s] Holding {since_speech_ms:8386} · TalkState holding
[ 36.84s] Heard {utterance:1, text:"Stop", final:false}
[ 36.84s] Flush · SpeechFinished {stopped} ×3 · Interrupted {heard_sentences:0, dropped_sentences:3} · TalkState listening
[ 39.71s] Submitted {text:"[spoken, 3 s, interrupted] Stop, wait. Tell me the date instead", interrupted:true}
It’s Tuesday, September 29th, 2026.
[ 82.42s] talk_stop → TalkState idle → TalkFinished {stopped}
--- transcript: user [spoken, 4 s] … / assistant (full first reply) / user [spoken, 3 s, interrupted] … / assistant …
```

Here stage 2 decided on the first partial ("Stop", 410 ms after `Holding`);
in the device-sink runs the partial came after the 900 ms deadline and the
commit was the fallback — the reason `talk.decide_ms` exists.

### 2.5 Not verified

- Audio **by ear**: nobody heard the speaker; the evidence is
  `SpeechStarted{sink: device}` / `Saying` / `SpeechFinished` and the `say`
  utterances being transcribed through the room mic.
- `heard_sentences ≥ 1`: every barge-in in the debug build came while the
  first sentence was still being synthesised (TTS ≈ 7–10 s to first frame),
  so the heard prefix was always empty (`Cancel(Some(""))`) and the kept
  assistant row is the whole reply (TALK-CORE's "short replies are never
  truncated"). Needs `--release` and a longer reply.
- Backchannel/echo `Resumed` on purpose (one `Resume(Backchannel)` fired on an
  empty final while holding, 23:45:23 — noise, not a spoken "mhm").
- Phone through the relay; Windows/Linux; Tauri/WKWebView orb (the smoke used
  headless Chromium against the Vite dev server).
- The desktop `Esc` cancel path and the `talk_stop` on connection close were
  not exercised in this pass (TALK-CORE covered the latter).

### 2.6 Gate (d) tally

`cargo test -p talk -p voice -p agent -p rpc -p tui -p meeting --no-fail-fast > /tmp/talk76/cargo-test.log 2>&1`
on the final tree (`63a697d7`): exit 0; `grep -c '^test result'` → 16;
summed: **573 passed, 0 failed, 0 ignored**. Per binary: agent 326, meeting
56 (+ note 1, oracle 1), rpc 35 (+ prompts 1), talk 12, tui 116 (+ no_emoji 1),
voice 24 (the `live::tests::utterer_returns_the_spoken_sentence_punctuated`
flake did not bite), six empty doctest binaries. The first run had one red:
`policy::tests::speech_while_thinking_cancels_and_submits_the_new_text`
asserted the empty `Heard` final I stopped publishing (§1.2); the test now
asserts nothing is emitted. Re-run alone and in the full set: green.
`cargo clippy --workspace --all-targets` on the same tree: exit 0, zero warnings.

## 3. Deviations

- `TalkLevel` tag is the variant name, not `talk_level` (§1.1).
- `stop_reason: "interrupted"` stays an ordinary finish in both frontends (§1.2).
- `talk.decide_ms` is a new config key the design (§4.10) does not list; the
  design doc was not amended beyond §13 (the brief's one allowed edit).

## 4. Outside the allow-list

- `crates/theuth/src/main.rs`: one `DEFAULT_CONFIG` comment line (`decide_ms`).
- `crates/rpc/src/client.rs` is inside the allow-list (`crates/rpc`) but the
  `A2aChanged` fix affects every attached frontend, not just talk (§1.3).
- `packages/apps`: `bun install --frozen-lockfile` only, no file changes.

## 5. Follow-ups (left on purpose)

- **Short replies are never truncated** (TALK-CORE): the model finishes
  streaming before the first sentence plays, so `Cancel{keep}` finds no turn
  in flight and the whole reply stays in history. Needs an `agent` message
  that rewrites the last assistant row to the heard prefix — KEEP-AFTER-TURN's
  seam, not touched here.
- Echo through a mic VPIO does not cover (USB webcam + aggregate output): two
  of the device-sink barge-ins in the smoke were the agent hearing itself
  (`Holding` with no text, commit on the deadline). `echo_ratio`/`decide_ms`
  are the knobs; the software-AEC spike (§10 step 9) is the real answer.
- Inter-sentence gap on the device sink, CoreAudio latency query, `Player::write`
  2 ms poll (VOICE-OUT) — untouched.
- Integration test with fake `Voice`/`Agents` actors (TALK-CORE) — not written;
  the live smoke above is the end-to-end check.
- `Holding` motion hint in the TUI orb (TUI-TALK) — cosmetic, untouched.
- The `talk.ts` `applySubtitles` still accepts an empty `Heard` final should
  one ever arrive; `talk` no longer publishes them.
