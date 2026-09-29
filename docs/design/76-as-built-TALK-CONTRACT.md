# 76 — as built: TALK-CONTRACT (the `talk` plugin's public surface)

Status: contract landed 2026-09-28 on `talk/contract` (base `feature/talk` @
c98be2b6). Behaviour is stubbed; TALK-CORE fills the actor in. The UI
workers code against what is listed here.

## What was built

### `crates/talk` (new)

`Cargo.toml` per plugins.md §3.1; deps `runtime, tools, agent, voice,
markdown, kameo, tokio, async-trait, anyhow, serde, schemars, uuid, tracing`,
`ts-rs` optional behind `ts` (forwards `runtime/ts`, `voice/ts`). `agent` and
`markdown` are unused by the stub and kept so TALK-CORE does not touch
`Cargo.toml`/`Cargo.lock` on a sibling branch. Dev-dep: `serde_json`.

`src/lib.rs` — every type is `pub`, doc-commented, `schemars::JsonSchema`;
wire-visible ones derive `ts_rs::TS` behind `ts`:

| Item | Shape (JSON) |
|---|---|
| `TALK_ACTOR` | `"talk"` |
| `TalkId(Uuid)` | uuid v7, `#[serde(transparent)]`, `Display`/`FromStr` like `SpeechId` |
| `Sink { Client, Device }` | `"client" \| "device"` — local stand-in; TALK-CORE switches to `voice::Sink` when VOICE-OUT lands it |
| `Aec { Vpio, None }` | `"vpio" \| "none"` — same rule: re-export `voice::Aec` if it lands there |
| `State { Idle, Listening, Thinking, Speaking, Holding, Interrupted }` | snake_case strings |
| `Hold { Pause, None }`, `AecMode { Prefer, Off }`, `TurnModel { None, SmartTurn }` | config enums: `"pause"`, `"prefer"`, `"smart-turn"` (kebab) |
| `TalkConfig` `#[serde(default)]` | `enabled, barge_in_min_ms, end_of_turn_ms, end_of_turn_punct_ms, hold, backchannels, echo_ratio, aec, turn_model` with the §4.10 defaults |
| `StartTalk { session, sink, sample_rate: Option<u32> }` | `ask` → `Result<TalkId, String>` |
| `PushAudio { id, pcm: Vec<i16>, played_seq: Option<u32> }` | `ask` → `Result<(), String>` |
| `StopTalk { id }` | `ask` → `Result<(), String>` |
| `TalkStatus` | `ask` → `TalkState { active: Option<Active { id, session, state, aec }>, model: voice::ModelState }` |
| `ResumeReason { Backchannel, Echo }` | snake_case |
| `TalkDegradation { MicNoAec }` | `"mic_no_aec"` (named to avoid clobbering `meeting::Degradation`'s `Degradation.ts`) |
| `EndReason { Stopped, Disconnected, SessionClosed, Error { message } }` | `#[serde(tag = "reason")]`, snake_case (named to avoid clobbering `voice::FinishReason`'s `FinishReason.ts`) |
| `TalkEvent` `#[serde(tag = "type")]` | `TalkStarted{id, session, sink, aec, since_ms}`, `TalkState{id, state}`, `Level{id, rms: [f32; 8]}`, `Heard{id, utterance, text, final}`, `Saying{id, speech: SpeechId, text}`, `Submitted{id, text, interrupted}`, `Holding{id, since_speech_ms}`, `Resumed{id, reason}`, `Interrupted{id, heard_sentences, dropped_sentences}`, `Flush{id}`, `Degraded{id, degradation}`, `TalkFinished{id, reason}` |
| `Talk` (unit struct, `kameo::Actor`) | `StartTalk`/`PushAudio`/`StopTalk` → `Err("talk: not implemented yet")`; `TalkStatus` → `active: None`, `model` = the `Dictation` actor's `DictationStatus.model` (`Missing` when voice is off) |
| `TalkPlugin` | reads `talk:`; when `enabled` spawns `Talk` as `TALK_ACTOR` and registers `RegisterPrompt { plugin: "talk" }` with the §0.11 block (`## Live talk`: live speech, short spoken sentences, one sentence before a tool batch, may be cut off, history keeps only what was heard, `[spoken, N s]` / `[spoken, N s, interrupted]`) |

### Wiring

- Root `Cargo.toml`: `talk = { path = "crates/talk" }  # … (76)`.
- `crates/theuth/src/main.rs`: `Box::new(talk::TalkPlugin)` after `voice`; a
  `# talk:` block in `DEFAULT_CONFIG` with every §4.10 key
  (`player_buffer_ms` is `voice`'s per §4.10 and is not in `talk:`).
- `crates/rpc/src/api.rs`: `talk_start(session, sink, sample_rate?) -> TalkId`,
  `talk_stop(id)`, `talk_audio(id, pcm, played_seq?)`, `talk_status()`; all
  `with_extensions` + `peer(ext)?`. `talk_start` refuses a remote peer with
  `sink: device` ("speaker and microphone are on the harness host; pass sink:
  client and sample_rate to stream audio") and anyone with `sink: client` and
  no rate ("sink: client needs sample_rate"); a `Client` talk is stopped when
  its peer's connection closes (the `dictation_start` pattern). `talk_audio`
  decodes base64 LE i16 exactly as `dictation_audio`. Missing actor → `NOT_RUNNING`
  "talk is disabled".
- `crates/rpc/src/bridge.rs`: `TalkEvent` in `forward!` and subscribed.
- `crates/rpc/src/export.rs`: exports `TalkEvent, TalkState, TalkId, State, Sink, Aec`
  (`export_all` pulls `Active`, `EndReason`, `ResumeReason`, `TalkDegradation`).
- TUI: `UiEvent::Talk(talk::TalkEvent)` forwarded from the bus, ignored in
  `Model::apply` and `print_event`.
- `docs/design/00-architecture.md` §10: one `talk/` line.

## Verified

Commands and the lines that matter (all run in this worktree, toolchain per brief):

- `cargo clippy -p talk --all-targets --features ts` → `Finished dev profile … in 10m 48s`, no warnings.
- `just ts-desktop` → `test result: ok. 8 passed` (rpc `export_bindings`); `bindings/`
  gains `Active.ts Aec.ts EndReason.ts ResumeReason.ts Sink.ts State.ts TalkDegradation.ts
  TalkEvent.ts TalkId.ts TalkState.ts`; `methods.json` lists `talk_audio, talk_start,
  talk_status, talk_stop`. `TalkEvent.ts` shows all 12 variants (`TalkStarted, TalkState,
  Level, Heard, Saying, Submitted, Holding, Resumed, Interrupted, Flush, Degraded,
  TalkFinished`); `State = "idle" | "listening" | "thinking" | "speaking" | "holding" |
  "interrupted"`, `Sink = "client" | "device"`, `Aec = "vpio" | "none"`, `TalkId = string`,
  `EndReason = { "reason": "stopped" } | … | { "reason": "error", message: string }`.
  `u64` fields (`since_ms`, `since_speech_ms`) export as `bigint`, like every other
  `*_ms: u64` in the tree (`Transcript.duration_ms`, `MeetingEvent.t0_ms`).
- `cargo check --workspace --tests` → `Finished dev profile … in 9m 23s` (after `just ts`:
  `nodes`' test build `include!`s `bindings/methods.json`, absent in a fresh worktree —
  pre-existing, not talk's).
- `cargo test -p talk -p rpc` → rpc `test result: ok. 35 passed; 0 failed` (incl.
  `api::tests::talk_device_sink_refuses_relay_peers`), talk `test result: ok. 3 passed`
  (`wire_shape`, `config_defaults`, `stub_answers`).
- `cargo clippy -p talk -p rpc --all-targets` → `Finished dev profile … in 29m 23s`, no warnings.
- Headless smoke: `THEUTH_DATA_DIR=/tmp/talk-TALK-CONTRACT-data THEUTH_RPC__LISTEN=127.0.0.1:7791
  cargo run -p theuth -- --headless`, then a 10-line bun WebSocket script
  (`ws://127.0.0.1:7791/?token=<data_dir>/token`):
  - `talk_status []` →
    `{"jsonrpc":"2.0","id":1,"result":{"active":null,"model":{"state":"missing"}}}`
    (`missing` = the `Dictation` actor's real state before any prewarm).
  - `talk_start [<session>, "device", null]` →
    `{"jsonrpc":"2.0","id":2,"error":{"code":-32000,"message":"talk: not implemented yet"}}`.
  - `get_prompt [null]` contains `## Live talk\nA user message that starts with
    \`[spoken, N s]\` was said aloud to you just now, …` — the block is registered.

## Not verified

- Nothing behavioural: every command but `talk_status` is a stub by design.
- The desktop UI against the new `bindings/` (UI workers' job).
- `bindings/` is `.gitignore`d at the root (`/bindings`), so the brief's
  "commit them" cannot be done; `just ts-desktop` regenerates them and the
  variant list above is from the generated files.

## Deviations

- `FinishReason` → `EndReason`, `Degradation` → `TalkDegradation`: ts-rs
  writes one file per type name into a flat `bindings/`; the doc's names
  would have overwritten `voice::FinishReason` and `meeting::Degradation`.
- `Talk` is a unit struct: the stub holds no `Runtime`/`TalkConfig` (dead
  fields would be `dead_code` warnings under zero-warnings). TALK-CORE adds
  the §4.1 fields with the loop.
- `talk_start` also refuses `sink: client` without a rate for *local* peers
  (the actor could not run the remote leg without one); the doc only names
  the remote rule.
- No `RegisterLifecycle` for `CloseTree` yet (§0.10 "session closes"): that
  is behaviour, TALK-CORE — and it must add `"talk"` to the `owners` list in
  `start_plugins` when it does.
- Prompt block is a `const`, not a function of config (§4.11 "pure function
  of config"): nothing in it depends on config yet.

## Outside the allow-list

Minimal edits, each required for the listed step to compile or export:

- `crates/tui/src/model.rs` — `UiEvent` lives here, not in `lib.rs`: the
  `Talk(talk::TalkEvent)` variant (+1 doc line) and the one `=> {}` arm in `apply`.
- `crates/tui/Cargo.toml`, `crates/rpc/Cargo.toml`, `crates/theuth/Cargo.toml`
  — `talk = { workspace = true }`; `rpc`'s `ts` feature forwards `talk/ts`.
- `crates/rpc/src/export.rs` — the six `talk::` lines in the `export!` list.

## Follow-ups (TALK-CORE and others)

- Replace the three `Err("talk: not implemented yet")` handlers; add `Live`,
  `TurnLedger`, the audio task, the classifier (§4.1–§4.8).
- Switch `talk::Sink`/`talk::Aec` to re-exports of `voice::Sink`/`voice::Aec`
  once VOICE-OUT lands them; the JSON shape must stay `"client" | "device"`,
  `"vpio" | "none"`.
- `TalkStatus.active` is always `None` until then.
- `nodes`' test build reads `bindings/methods.json`; a fresh worktree must run
  `just ts` before `cargo check --workspace --tests` (pre-existing, not talk's).
