# 76 as-built — VOICE-OUT (talk, steps 1–4 in `voice`)

Branch `talk/voice-out` off `feature/talk` @ c98be2b6. Scope: 76 §0.3, §0.4,
§0.5, §0.10, §4.3, §10 steps 1–4 — everything `talk` needs from `crates/voice`,
with today's callers (43 speak button, 45 clips, 54 dictation, the meeting
crate) unchanged in behaviour. No `crates/talk`; nothing in `agent`, `tui`,
`desktop/ui`.

## 1. What was built

### 1.1 `voice::out` — the daemon's speaker (`crates/voice/src/out.rs`)

- `Ring` — the pure part the device callback drives: an SPSC ring of `f32`
  bits (`AtomicU32` slots, `written`/`read` `AtomicU64` cursors), `paused`,
  the reported `latency`, the last callback's frame count, and a
  `parking_lot::Mutex<Ledger>` the callback never locks. Device-rate samples
  throughout; `Played` converts to 24 kHz at the boundary.
  - `push(id, pcm) -> Result<usize, Retired>`: writes up to the free space,
    creates/extends the ledger `Entry { id, first, last, dropped, epoch }`;
    `Retired` when a flush cut `id` (epoch mismatch or `dropped`), so a
    writer that missed the flush cannot leak samples in behind it.
  - `fill(out, channels, latency)`: the callback; mono → every channel;
    zero-fill when paused or empty; advances `read` with `fetch_max` so a
    concurrent flush's jump is never undone.
  - `pause() -> Played` / `resume()`: one `AtomicBool`.
  - `flush() -> Played`: under the ledger lock — position before the cut,
    `epoch += 1`, `read = max(read, written)`, entries with unheard samples
    marked `dropped`, unpaused. Returning the pre-cut `Played` is the
    "ledger at flush time" §4.6 needs (the brief listed `flush()` without a
    return; adding one costs nothing and saves a `Pause` round trip).
  - `played()` → `Played { id, seq, samples }`: `position = read −
    latency` (40 ms when the backend reports none), the ledger entry
    containing it gives `id` and `seq = (position − first) / 1920`.
  - `ledger() -> Vec<Entry>`, `latency()`, `callback_frames()`, `written()`.
  - Entries older than 2 s behind `read` are pruned on `push`.
- `Player` — owns the device on a `voice-out` `std::thread` (the `mic.rs`
  pattern; keeps `cpal::Stream`'s `Send`-ness out of the actor), `Arc<Ring>`,
  an optional `rubato::FftFixedIn` (24 kHz → device rate, producer side, 20 ms
  blocks) when the device offers no 24 kHz `f32` config.
  - `open(device: Option<&str>, buffer_ms) -> Result<Player>`: named output
    device or the default; `pick()` prefers an `f32` range containing
    24 kHz at the default channel count, else the default config's rate.
    Returns once the stream plays. Fields `device`, `rate`, `channels`.
  - `write(id, pcm) -> bool`: blocks in 2 ms sleeps while the ring is full
    (bounded run-ahead = `buffer_ms`); `false` = retired by a flush.
  - `pause() -> Played`, `resume()`, `flush() -> Played`, `position() ->
    Played`, `ring()`.
  - Drop stops the thread and closes the device.
- Latency per backend (read from cpal 0.16 source, not measured on the
  other hosts): CoreAudio and WASAPI set `playback = callback + one buffer's
  duration` (a fixed estimate — not the device's own latency property);
  ALSA uses `snd_pcm_delay`. The Player uses `playback − callback` from the
  last callback; 0/none → 40 ms.

### 1.2 `Voice` actor (`crates/voice/src/lib.rs`, `speak.rs`, `clip.rs`)

- `Sink { Client, Device }` — `#[serde(tag = "sink", rename_all =
  "snake_case")]`, `Default = Client`, ts export.
- `Speak { session, text, queue: bool, sink: Sink }` — `queue` and `sink`
  `#[serde(default)]`, so the existing wire shape parses unchanged.
  `queue: false` clears the queue (each `SpeechFinished { Stopped }`) then
  stops the current utterance — today's replacing semantics; `queue: true`
  appends to the FIFO. Every utterance gets its id at `Speak` time.
- `Stop { id: None }` clears the queue and the current utterance and
  **releases the speaker** (`player = None`); `Stop { id: Some }` stops that
  one wherever it is (current → the next queued starts; queued → removed,
  `Stopped`).
- New messages: `Pause` → `Result<Played, String>`, `Resume` →
  `Result<(), String>`, `Position` → `Result<Played, String>` (the brief's
  "`Played` query": named `Position` because the reply type is `Played`).
  `Err("no device sink open")` without a player.
- `VoiceEvent::SpeechStarted` / `SpeechFinished` gain `sink: Sink`
  (`#[serde(default)]`). `SpeechAudio` is published for `Sink::Client` only
  and did not gain the field (the brief names Started/Finished; the design's
  §5.3 aside about `SpeechAudio` carrying `sink` is moot when none are sent).
- `VoiceStatus.queued: Vec<SpeechId>`.
- The speaker opens on the first `Sink::Device` utterance
  (`cfg.player_device`, `cfg.player_buffer_ms`) and closes on `Stop { id:
  None }` — a talk's last word — so the device is held only while one runs;
  never at prewarm. If it fails to open, that utterance ends `Error` and the
  queue advances.
- `speak::Synthesizer` trait (`stream(spoken, voice) -> Pcm`), implemented by
  `ptts::Synth`; the actor holds `Option<Arc<dyn Synthesizer>>`, `clip::render`
  takes `&dyn Synthesizer`. This is what makes the actor testable without
  weights (the brief allowed it "if needed" — it was).
- `speak::Out { Client, Device(Arc<Player>) }`: the device path writes each
  1920-sample frame with `Player::write`; no Opus encoder is built; a `false`
  from `write` ends the task silently (the actor already published
  `Stopped`). `SpeechFinished { Done }` for the device sink means "every
  sample is in the ring" (≤ `player_buffer_ms` still to play), the same
  meaning as 43's "every packet published".
- `CloseTree` also drops queued utterances of the closing sessions.
- `Loaded` failure ends the current *and* queued utterances with `Error`.

### 1.3 `voice::mic` — `Capture::open_with(Aec)`

- `Aec { Vpio, None }` — `#[serde(tag = "aec", rename_all = "snake_case")]`,
  `Default = None`, ts export.
- `Capture::open_with(want) -> Result<(Capture, Receiver<Chunk>, Aec)>`:
  `Aec::Vpio` tries the VoiceProcessingIO unit (macOS), warns and falls back
  to the plain mic when it fails; the third value is what opened.
  `Capture::open()` = `open_with(Aec::None)` minus the report — unchanged.
- The `vpio` module moved verbatim from `meeting/src/capture/mic.rs`; its
  callback-sized buffers are rechunked to 960 samples (20 ms) on the
  `voice-mic` thread so both paths deliver the same chunk shape. The
  `AudioUnit` lives in `Capture` and is dropped first on `Drop`.
- `coreaudio-rs` moved from `meeting`'s to `voice`'s macOS deps.
- `meeting/src/capture/mic.rs` now calls `Capture::open_with(Aec::Vpio)`
  and maps `Aec::None → vec![Degradation::MicNoAec]`. `permission()` stays.

### 1.4 `Dictation::Lease` / `Return`

- `Lease` → `DelegatedReply<Result<Box<Utterer>, String>>`: "already
  dictating" while a dictation runs, "talking" while leased, loads first
  when the model is not loaded (parked as a `Waiter::Lease`, same path as
  `Prewarm`/`StartDictation`), the load error otherwise.
- `Return(Box<Utterer>)` → `()`: engine back, `leased = false`.
- `StartDictation` while leased → `Err("talking")` (both before and after a
  load). `Prewarm` while leased → `Ready` (no-op: `load()` returns early
  when the state is not `Missing`/`Failed`).

### 1.5 Config

`VoiceConfig { player_buffer_ms: u32 = 200, player_device: Option<String> =
None }`, mirrored in `DEFAULT_CONFIG` (`crates/theuth/src/main.rs`).

### 1.6 Bench

`cargo run -p voice --example audio_out -- [device]`
(`crates/voice/examples/audio_out.rs`): 3 s of 440 Hz through the Player,
`flush()` at 1.5 s, prints device/rate/channels, ring size, callback
period, reported latency, position vs written at flush, writer return time,
and the ledger 300 ms later.

## 2. Verified (this Mac, 2026-09-28)

Toolchain `stable-aarch64-apple-darwin`, `just`'s `PATH`.

### 2.1 Bench — `cargo run -p voice --example audio_out`

```
device: "Aggregate Device" rate: 48000 channels: 2 opened in 270.124084ms
ring: 9600 device samples (200 ms)
callback: 512 frames = 10.7 ms
reported output latency: 10.666667ms
at flush (t=1.5 s): written 40896 samples (1704 ms), position 35840 samples (1493 ms), id Some("tone") seq 18
position() before flush: 35840 samples; run-ahead (written - position): 210 ms
writer returned 3.707583ms after flush having written 40320 of 72000 samples
300 ms after flush: position 48064 samples, id None, ledger [(0, 81792, true)]
closed at 2.153478334s
```

Reading: the default output here is a 48 kHz aggregate device with no 24 kHz
`f32` config, so the **rubato path ran** (24 → 48 kHz, producer side).
CoreAudio callback = 512 frames = 10.7 ms; the reported latency is cpal's
one-buffer estimate (10.7 ms), as its source says. At the 1.5 s flush,
`position()` said 1493 ms — 7 ms behind wall time, i.e. within one 80 ms
frame (`seq 18` = 1440–1520 ms) of samples written − buffered − latency;
run-ahead 210 ms = the 200 ms ring + latency. The blocked writer returned
3.7 ms after the flush with `false` (retired), having pushed 21 frames. 300 ms
later the ledger shows the tone's single entry `dropped` and `id: None`.

### 2.2 Unit tests — `cargo test -p voice --lib`

```
test out::tests::flush_unpauses ... ok
test out::tests::pause_resume_flush ... ok
test out::tests::ring_bounds_and_order ... ok
test out::tests::position_and_ledger ... ok
test tests::queued_speaks_play_in_order ... ok
test tests::stop_all_clears_the_queue ... ok
test tests::replacing_speak_clears_the_queue ... ok
test tests::stop_one ... ok
test tests::device_controls_need_a_player ... ok
test tests::close_tree_stops_only_the_subtree_utterance ... ok
test dictation::tests::leased_engine_refuses_dictation ... ok
test dictation::tests::lease_needs_the_model ... ok
test dictation::tests::missing_model_fails_parked_start ... ok
test dictation::tests::remote_guards ... ok
```

- `out::` — the ring, ledger and position arithmetic as a pure struct
  (§10 step 1's check without a device): bounded push / ordered drain,
  position = read + silence − latency (40 ms default and a reported 10 ms),
  id/seq lookup, pause holds the cursor and zero-fills, resume continues
  from the paused sample, flush drops the buffer, retires the utterance
  (`push → Retired`), reports the pre-cut position and unpauses.
- `tests::` — §10 step 2 on a fake synth (5 silent frames at 20 ms each):
  three `Speak{queue: true}` → three `SpeechStarted` in order, one
  `SpeechFinished{Done}` each; `Stop{None}` after the second started →
  `Stopped` for 2 and 3, no further event for 1, nothing late; `Speak{queue:
  false}` during a queue → 2 and 3 `Stopped`, the new one speaks; `Stop{Some}`
  of a queued id removes it, of the current id starts the next.
- `dictation::` — leased: `StartDictation` → "talking", `Lease` →
  "talking", `Prewarm` → `Ready`; no model: `Lease` parks and fails with the
  path-bearing load error.
- `live::tests::utterer_returns_the_spoken_sentence_punctuated` failed once
  in the first full run (`"The deploy is blocked on the image tags"` without
  the final period) and passed on the rerun; it drives the real Nemotron
  model with `say` output and is untouched by this change.

### 2.3 Gates

- `cargo clippy -p voice -p meeting --all-targets` — clean (one
  `option_map_unit_fn` fixed on the way).
- `cargo test -p voice -p meeting`:
  ```
  meeting unittests: test result: ok. 56 passed; 0 failed
  meeting tests/note.rs: ok. 1 passed;  tests/oracle.rs: ok. 1 passed
  voice unittests:   test result: ok. 24 passed; 0 failed
  ```
  (a first attempt hit `cached cgu … should have an object file` in
  `surface` — a stale incremental dir from an interrupted earlier build;
  `rm -rf target/debug/incremental/surface-*` and it ran clean).
- `cargo check --workspace --tests` — `Finished dev profile`, no warnings
  from this tree (the `block v0.1.6` future-incompat note is pre-existing).
  It needs `bindings/methods.json` (`nodes` `include_str!`s it), which
  `just ts` produces — run `just ts-desktop` first in a fresh worktree.
- `just ts-desktop` — `export_bindings` 8 passed; `bindings/Sink.ts`
  (`{ "sink": "client" } | { "sink": "device" }`) and `sink: Sink` on
  `SpeechStarted`/`SpeechFinished` in `VoiceEvent.ts`. Both `bindings/` and
  `crates/desktop/ui/src/rpc/bindings/` are gitignored (`/bindings`,
  `crates/desktop/.gitignore`), so there is nothing to commit; `Played`/`Aec`
  are not yet reachable from an RPC type and so not exported until `talk`
  step 5 references them.

## 3. Not verified

- Windows/Linux: the Player and `Aec::None` paths compile behind no cfg
  (cpal is cross-platform) but were not run there; `Aec::Vpio` on those
  hosts returns `Aec::None` without trying anything.
- The 24 kHz-native device path (no resampler): this Mac's default output
  is a 48 kHz aggregate, so the bench ran the rubato branch; the identity
  branch is what the unit tests use (`Ring::new(RATE, …)`).
- `Sink::Device` end-to-end with the real model (needs `talk`, step 5); the
  device path is exercised by the bench (Player) and the actor tests
  (`Out::Client` with a fake synth). `Return` with a real `Utterer` — the
  handler is two assignments; the guards are unit-tested with `leased`
  set directly.
- Audible output was not checked by ear (headless run); the callback
  consumed the data (read cursor advanced at device rate).

## 4. Deviations and decisions

- `flush()` returns `Played` (brief: no return). See §1.1.
- The "`Played` query" message is `Position`.
- `SpeechAudio` does not carry `sink` (never published for the device sink).
- Speaker lifetime: opened on the first device-sink utterance, closed on
  `Stop { id: None }`. The brief says "open per talk, closed on drop" —
  `Voice` does not know about talks, and `Stop { None }` is the message a
  talk ends with (§4.5 step 2 also sends it on every barge-in, which
  re-opens the device for the next sentence: a few ms, hidden under the
  model's time-to-first-sentence). If the churn matters, `talk` step 5 can
  add an explicit close message.
- `Synthesizer` trait object behind `Arc<dyn _>` (brief: allowed if the
  actor is otherwise untestable without weights — it was).
- Ledger coordinates: a flush advances `read` past the dropped samples, so
  `Played.samples` counts flushed samples as passed; dropped entries are
  flagged rather than subtracted. Simpler than a second "heard" counter and
  what §4.6 asks ("a `Playing` sentence at flush time is `Dropped`").

## 5. Outside the allow-list

None beyond the listed files: root `Cargo.toml` (cpal line + one comment on
the `coreaudio-rs` line), `Cargo.lock`, `crates/meeting/Cargo.toml`
(coreaudio-rs removed), `crates/theuth/src/main.rs` (two `DEFAULT_CONFIG`
comment lines), `crates/rpc/src/api.rs` (the `speak` RPC fills `queue:
false, sink: Client`; wire signature unchanged).

## 6. Follow-ups

- Inter-sentence gap on the device sink: the next queued utterance starts
  synthesizing only when the previous one has written its last frame, so a
  ~200 ms first-frame latency meets a ≤ 200 ms ring. Pre-synthesizing the
  head of the queue (or a `first frame ready` handoff) is a `talk`-phase
  tuning item once §10 step 5 hears it.
- Device latency on CoreAudio is cpal's one-buffer estimate; the true
  `kAudioDevicePropertyLatency + SafetyOffset` could be queried through
  `coreaudio-rs` if §10 step 7's numbers need it.
- `Player::write` polls at 2 ms when the ring is full (`ponytail`: a
  condvar if the CPU shows).
