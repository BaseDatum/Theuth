# 45 — Voice, phase 2: voice notes in, spoken progress out

Status: design v0 · 2026-09-17 · extends `43-voice.md` (the `voice` plugin
grows speech-to-text and a clip encoder), `21-slack.md` and the Signal channel
(voice notes as input, voice replies per prose block), `25-desktop.md` §3/§7
(a microphone in the composer, `transcribe` over RPC), `00-architecture.md`
§9/§10 (a second model directory, four crates). §12 lists the amendments.

Scope: the user sends a **voice note** on Signal, Slack, or from the desktop
or phone app. The harness transcribes it locally, the model is told the
message was spoken and answers for the ear, and every block of prose it
writes — the "let me find that for you" before its tool calls as much as the
final answer — goes back on the same channel **as text at once and as a voice
message as soon as the clip is synthesized**. The middle ground between 43
(press a button, hear a reply) and a live, interruptible voice agent, which
still gets its own doc (43 §9).

## 0. Decisions in one screen

1. **Speech-to-text runs in the harness, in the `voice` crate, with
   `parakeet-rs` 0.3 (NVIDIA Parakeet TDT 0.6B v3, ONNX Runtime, CPU).** One
   model serves every channel; the transcript is text by the time the agent
   sees it, so the agent loop, the session store and the model API learn
   nothing about audio. Measured on an M3 Max with the int8 checkpoint at
   hand (§2.1): load 0.7–0.9 s, RTF 0.028 (8.7 s of speech in 240 ms),
   transcript verbatim. Rejected: whisper.cpp (`whisper-rs`, a Metal build,
   slower on CPU); the Slack-native transcript on audio clips (Slack only,
   and Signal/desktop still need ours); a cloud STT (the user asked for this
   model; every voice note would leave the machine).
2. **The channel owns "voice mode"; the agent owns nothing new.** A voice
   note becomes a user text line `[voice note, N s] <transcript>`, produced
   by `voice::note_text`, and one static prompt block (07 §4: pure function
   of config) registered by the `voice` plugin tells the model what that
   prefix means and how to answer — short, spoken, one sentence of narration
   before each batch of tool calls, back to normal when the user types. No
   per-turn hint field, no new `ContentPart`, no session flag in `agent`:
   the marker is the flag, and it survives history, compaction and forks
   for free. Rejected: a `voice: bool` on `SendInput`/`UserInput` (three
   structs, a store column, and the prompt would still have to say what it
   means); `message_user` as the narration channel (the model already
   narrates in prose; making it call a tool to do so costs a round trip and
   it forgets).
3. **Delivery unit is the prose block: text now, clip when ready, in
   order.** A "block" is the assistant text written before a batch of tool
   calls, or the tail of the turn. Signal today sends one text batch at
   `TurnFinished`; it now flushes at the first `ToolCallPending` after
   prose as well — for every turn, spoken or typed, because a progress line
   is worth a message on a phone either way. Slack already streams the text
   bubble (21 §3); it gains the clip per block. When the turn is a voice
   turn the flushed markdown also goes to `voice::Synthesize`, and the clip
   is sent as a voice message — after the previous block's clip, never
   before it (a `JoinHandle` chain per session, §4.3). Rejected: one clip
   for the whole turn at `TurnFinished` (jarvis; the user waits through the
   whole tool run in silence — the opposite of the requirement); sentence-
   level clips (a Signal thread of twelve two-second voice notes).
4. **Clip container: MP3, 24 kHz mono, 48 kbit/s, via `mp3lame-encoder`.**
   ≈ 6 KB/s (52 KB for the 8.7 s smoke reply). Plays as a native voice
   bubble on Signal Android, iOS and Desktop and with Slack's inline player
   (MP3 and WAV are the two formats Slack documents). Rejected: Ogg/Opus
   (the codec 43 already ships — but Signal-iOS does not play Ogg/Opus
   voice notes, `Signal-iOS#5771`, and Slack does not promise a player);
   AAC/M4A (Signal's native recording format, but no Rust encoder without
   `fdk-aac`); WAV (48 KB/s, 1.4 MB for a 30 s answer on a phone).
5. **Inbound decoding: `symphonia` (aac, isomp4, mp3, ogg, wav, mkv) plus
   the `opus` crate for Opus packets.** Covers what the three surfaces
   actually produce: Signal iOS/Android `audio/aac` in M4A, Slack clips
   `audio/mp4`, WebKit's `MediaRecorder` `audio/mp4`, Chromium/WebKitGTK
   `audio/webm;codecs=opus` or `audio/ogg`. symphonia demuxes all of them
   and decodes all but Opus; Opus packets go to `opus::Decoder` (libopus,
   already linked for 43). Resample to 16 kHz with `rubato`. No ffmpeg on
   the user's machine, no subprocess.
6. **Desktop voice mode is client state.** The composer's microphone records
   with `MediaRecorder`, uploads the container as-is through
   `put_attachment` (audio extensions now allowed), asks `transcribe(path)`
   over RPC and sends the `[voice note …]` text through the same
   `send_input`/`steer` call a typed message uses. While a session is in
   voice mode (last input was a voice note) the client speaks every prose
   block with the `speak` RPC from 43 — at `ToolCallPending` and
   `TurnFinished`, queued so a block never cuts off the previous one. The
   harness has no per-client state to keep; the same code runs on the
   phone through the relay. Rejected: the harness auto-`Speak`ing for
   sessions in voice mode (which client? the TUI cannot play it, the phone
   may be closed); recording PCM in the webview and resampling client-side
   (1.3 MB base64 for a 30 s note over the relay vs 120 KB of AAC).
7. **The STT model unloads after ten idle minutes.** Resident it is
   1.75 GB (int8 encoder, ONNX Runtime arenas). Pocket TTS stays resident
   (43 §4.3, 650–940 MB) because the desktop button wants it warm; a voice
   note tolerates the 0.8 s reload after a quiet spell. `voice.stt_idle_secs`
   tunes it, `0` keeps it resident.
8. **STT is offline, whole-note.** The TDT model transcribes a finished
   note in 3 % of its duration; streaming STT (Parakeet EOU/Nemotron) is
   the live-agent doc's concern. Notes over three minutes are transcribed in
   three-minute pieces (TDT's ~4–5 min sequence limit).

## 1. Problem

43 §9 named the seams for a voice agent and stopped. What the user wants
next is not the live agent but the asynchronous one: talk to the harness the
way one talks to a person on Signal — a voice note, then get on with the day
— and get voice back, including *while* the agent works. Today:

- Signal (presage) already downloads attachments and knows the
  `VoiceMessage` flag (`signal/src/presage.rs:979`), then hands the model
  `[attachment: …/signal/inbox/<ts>-0.m4a (audio/aac, 48 KB, voice note)]`
  and nothing else (`channel.rs:1179-1219`). Slack does the same for
  `files[]` (`slack/src/channel.rs:1194-1248`). The model reads a path it
  cannot open.
- Signal delivers one text batch at `TurnFinished` (`channel.rs:1319-1408`);
  a five-tool turn is a typing indicator for a minute, then a wall of text.
- The desktop has no microphone (`grep MediaRecorder crates/desktop` →
  nothing), `put_attachment` and `agent::check_files` refuse anything but
  images and PDF, and `NSMicrophoneUsageDescription` is absent on both Apple
  targets.
- The `voice` plugin produces Opus packets on the bus (43 §5) and nothing a
  channel could attach.

## 2. Facts relied on

### 2.1 Speech-to-text: `parakeet-rs` and the checkpoint

- `parakeet-rs` 0.3.7 (crates.io, MIT/Apache): `ParakeetTDT::from_pretrained(dir,
  None)` reads `encoder-model.onnx` (falls back to `encoder-model.int8.onnx`),
  `decoder_joint-model.onnx`, `vocab.txt`; `Transcriber::transcribe_samples(audio:
  Vec<f32>, 16_000, 1, Some(TimestampMode::Sentences)) -> TranscriptionResult
  { text, tokens }`. Takes `&mut self`; one transcription at a time per
  model. Refuses a sample rate other than the model's 16 kHz
  ("Please resample your audio first", `audio.rs:215`).
- Depends on `ort` 2.0.0-rc.13. **Static linking is out**: pyke's
  `libonnxruntime.a` exports protobuf 3.21 with default visibility, and
  `sentencepiece-sys` (ptts's `sp` tokenizer) bundles protobuf-lite 3.14 —
  in one binary sentencepiece's generated code resolved to the 3.21
  runtime, libprotobuf's version check fired and the process aborted the
  moment the TTS model loaded (reproduced in the `voice` test binary). So
  `ort` is `load-dynamic`: the `Transcriber` `dlopen`s
  `libonnxruntime*.{dylib,so}` from `stt_model_dir`, else `$ORT_DYLIB_PATH`,
  else Homebrew's (`brew install onnxruntime`), with an error naming
  Microsoft's release tarball (≥ 1.17) when none is found. Its symbols
  stay private to the dylib. Also `tokenizers` 0.23 (`onig`), `ndarray`,
  `realfft`, `hound`. Nothing is downloaded at build; the binary does not
  grow.
- Checkpoint at hand: `parakeet-tdt-0.6b-v3-int8-smoothquant`
  (Olicorne's requantization of istupakov's ONNX export; CC-BY-4.0):
  `encoder-model.onnx` 902 MB (weights inline, no `.data`),
  `decoder_joint-model.onnx` 17 MB, `vocab.txt`, `config.json`
  (`nemo-conformer-tdt`, 128 features, subsampling 8), `nemo128.onnx`
  (unused by `parakeet-rs`). `CONTENTS.txt` says the files were renamed to
  `parakeet-rs`'s defaults on purpose. Its own `SMOOTHQUANT.md` calls the
  line retired in favour of a MatMulNBits int8 build (macro WER 12.9 % vs
  14.8 %, same speed); same loader, same layout — swap the directory when
  someone downloads it.
- Spike (`/tmp/stt-spike`, throwaway): M3 Max, 8.7 s of Pocket TTS speech
  (the 43 smoke WAV, 24 kHz): load 0.71–0.87 s, transcribe 235–240 ms
  (RTF 0.028), transcript byte-identical to the spoken text incl.
  punctuation. Peak RSS 1.75 GB. AAC/M4A (afconvert, 32 kbit/s) →
  symphonia decode + rubato resample 6 ms → same transcript. MP3 encode
  (LAME, 48 kbit/s, 24 kHz mono) 12 ms → 52 416 B; decoding that MP3 and
  transcribing it again: one word off ("C link" for "See link").
- TDT sequence limit: the crate's `examples/raw.rs` warns of ONNX errors
  past ~8–10 min and recommends ≤ 5 min chunks.

### 2.2 Channels

- Signal (`crates/signal`, presage rev c363731): `IncomingAttachment { bytes,
  mime, file_name, voice_note, duration_ms: None }` (`transport.rs:76-87`);
  `SignalTransport::send_attachment(to, bytes, mime, file_name, caption)`
  sets `voice_note: Some(mime.starts_with("audio/"))` on the
  `AttachmentSpec` (`presage.rs:636`) — **a native voice message send
  exists today**. `Channel::handle_envelope` (`channel.rs:1029`) builds
  `lines` then `save_attachments` (`:1179`) writes `<data_dir>/signal/inbox/
  <ts>-<n>.<ext>` and appends the `[attachment: …]` line; only `image/*`
  and PDF ride as `files`. `handle_agent` (`:1319`): `TextDelta` buffers
  into `Turn.text`, `TurnFinished` → `markdown_to_plain(flatten(text))` →
  `send_chunks` (≤ 2000 chars each) + ✅ reaction; `ToolCall*` are ignored.
  `Turn { in_flight, text, marks, typing }` per session (`:381`).
- Slack (`crates/slack`): `handle_push` maps `Message`/`AppMention` to `Msg
  { …, files: Vec<SlackFile> }` and **drops any message with a `subtype`**
  (`channel.rs:1069`); `save_files` (`:1194`) downloads
  `url_private_download` with the bot token to `<data_dir>/slack/inbox/
  <ts>-<name>`, same `[attachment: …]` line, same image/PDF filter.
  `SlackApi::upload_file(channel, thread_ts, &[UploadFile { name, mime,
  bytes }], comment) -> permalink` is files v2 (`getUploadURLExternal` →
  PUT → `completeUploadExternal`, `api.rs:658-712`). The bubble
  (`bubble.rs`) streams one message per turn; `Worker::apply` sees
  `ToolCallPending/Started/Finished` already (cards).
- Slack audio clips arrive as `files[]` with `mimetype: audio/mp4` and
  `subtype: slack_audio` on the *file* [INFERENCE from Slack's file object
  docs; a `file_share` message subtype on older workspaces — §10 step 3
  verifies against a real clip]. Slack has no API to *post* a clip; an
  uploaded `audio/mpeg` file gets the inline player.
- Signal clients: `audio/mpeg` with the `VoiceMessage` flag renders as a
  voice bubble on Android (MediaPlayer) and iOS (AVAudioPlayer); Ogg/Opus
  does not play on iOS (`Signal-iOS#5771`, open, workaround "convert to
  MP3").

### 2.3 Agent and prompt

- `agent::SendInput { session, text, files }`, `Steer { session, id, text,
  files }` → `loop_::UserInput`; `supervisor::check_files` rejects anything
  but `image/*` ≤ 5 MB and PDF ≤ 32 MB (`supervisor.rs:2457`). No hint or
  source field anywhere; the harness's existing per-message convention is a
  bracketed line in `text` (`[attachment: …]`, `(replying to: "…")`).
- System prompt: `frame::baseline` + `ListPrompts` blocks, rebuilt per turn
  but static per process (`tools/registry.rs:98-113`: "Pure function of
  config … anything dynamic busts the system cache"). A plugin registers
  with `registry.ask(RegisterPrompt { plugin, text })` after
  `rt.lookup_when::<tools::ToolRegistry>(REGISTRY_ACTOR)`.
- `AgentEvent::ToolCallPending { call_id, name }` is the first event of a
  tool batch, after the prose deltas of that assistant message; there is no
  "message complete" event, so it is the block boundary (Slack's bubble
  already keys cards off it).

### 2.4 Desktop

- Composer (`Composer.tsx`): `addBlobs` → `putAttachment(api, blob, ext)`
  (`lib/files.ts`, base64 → `put_attachment(bytes, ext) -> PathBuf`, ext
  whitelist png/jpg/jpeg/gif/webp ≤ 5 MB, pdf ≤ 32 MB, `rpc/src/api.rs:1434`)
  → `[⎘ …]` token → `submit()` → `steer` if running else `send_input`,
  `markSent(session, text, files)`. Send button in a `flex items-end gap-1`
  row after the textarea; `running && session` swaps Send for Stop, and the
  voice-note/dictation buttons stay beside Stop so a note recorded mid-turn
  queues as a steer (tooltip "Voice note (steers the running turn)").
  As built 2026-09-22: earlier the mic row was hidden while running.
- User bubble: `MessageUser.tsx` renders `<Attachments files/>` then the
  text; `Entry.files` exists for own sends only, history `HistoryEntry::User
  { text, turn }` has no files — anything a voice bubble needs to survive
  reload has to be in `text`.
- `lib/speech.ts` (43 §7): module-global single utterance, `start/push/stop/
  remaining`, `installSpeechListeners` plays any `SpeechAudio` for the
  connected harness; `useVoice()` = `voice_status` answered (−32001 →
  plugin disabled). No `AudioContext` reuse; each `start` opens one.
- wry 0.55 auto-grants `requestMediaCapturePermissionForOrigin`
  (`wry_web_view_ui_delegate.rs:136`), so `getUserMedia({audio})` works in
  WKWebView once the OS grants the app the microphone — which needs
  `NSMicrophoneUsageDescription` in the bundle's Info.plist on macOS
  (`crates/desktop/Info.plist`, merged by the Tauri bundler) and iOS
  (`ios/Info.ios.plist`, which has only the camera string today). Without
  it iOS kills the process on the first `getUserMedia`.
- WebKit's `MediaRecorder` emits `audio/mp4` (AAC-LC); Chromium's
  `audio/webm;codecs=opus`; WebKitGTK depends on the distro's GStreamer
  plugins [INFERENCE for the last; §10 step 6]. `MediaRecorder.isTypeSupported`
  picks at runtime; the harness decodes any of them (§0.5).

## 3. Shape

```
Signal / Slack / desktop mic
      │ audio file (m4a, mp4, webm, ogg, mp3, wav) on the harness disk
      ▼
voice::Transcriber ── decode (symphonia / opus) → mono → 16 kHz (rubato) → parakeet-rs
      │ Transcript { text, duration_ms }
      ▼
channel builds `[voice note, N s] <transcript>` ── SendInput / Steer ──▶ agent
                                                                           │ prose … ToolCallPending … prose … TurnFinished
      ┌─────────────── block boundary ────────────────────────────────────┘
      ▼
channel flush(block): text → chat now
                      voice turn? ── voice::Synthesize(md) ─▶ Voice actor: ptts → f32 → LAME → mp3
                                      ▲ awaited after the previous block's clip
                                      └── send_attachment(audio/mpeg, voice note) / upload_file(reply.mp3)
desktop: same boundaries client-side → api.speak(session, block) queued → SpeechAudio events → speech.ts
```

## 4. Plugin `voice` (`crates/voice`), additions

### 4.1 Config

```yaml
voice:
  enabled: true
  voice: peter_yearsley
  # model_dir: ~/.theuth/models/pocket-tts     # Pocket TTS (43)
  # stt_model_dir: ~/.theuth/models/parakeet-tdt  # Parakeet TDT ONNX: encoder-model.onnx, decoder_joint-model.onnx, vocab.txt
  stt_idle_secs: 600                          # unload the STT model after this long unused; 0 keeps it resident
```

`enabled: false` disables all of it: no `Transcriber`, so channels fall back
to today's `[attachment: …]` line (§4.5) and the desktop shows no mic.

### 4.2 `Transcriber` actor (`TRANSCRIBER_ACTOR = "transcriber"`)

```rust
pub struct Transcribe { pub path: PathBuf }
pub struct Transcript { pub text: String, pub duration_ms: u64 }
impl Message<Transcribe> for Transcriber { type Reply = DelegatedReply<Result<Transcript, String>>; }
```

- State: `model: Arc<Mutex<Option<ParakeetTDT>>>` (std mutex, held only
  inside `spawn_blocking`), `last_used: Arc<Mutex<Instant>>`, `dir`,
  `idle: Duration`. The handler reads the file, spawns a blocking task, and
  replies through `ctx.reply_sender()` so the actor never blocks on
  inference (the pattern `loop_.rs:659` uses).
- The blocking task: `stt::decode(bytes, ext) -> (Vec<f32> mono, rate)`
  (§0.5; `ext` from the path — the channels already name inbox files by
  `extension_for(mime)`), `stt::resample(pcm, rate, 16_000)`, then
  `model.lock()`; load if `None` (`ParakeetTDT::from_pretrained(dir, None)`,
  `Err` → "no STT model in <dir>: <e>" — the checkpoint is not on the
  mirror yet, same footing as 43 §11 for the TTS files), transcribe in
  ≤ 180 s pieces, join with a space, `duration_ms = samples * 1000 / rate`.
  Empty transcript → `Ok(Transcript { text: "", .. })`; the channel decides
  what to say (§4.5).
- Idle unload: the load spawns one `tokio::time::interval(60 s)` watchdog
  that drops the model when `last_used.elapsed() > idle` (skipped when
  `idle` is zero). `ponytail:` a fixed interval, not a timer reset per
  transcription — worst case the model lives one minute longer than asked.
- Errors are strings: no `parakeet` type leaves the crate (43 §4.7's rule
  for `ptts`).

### 4.3 `Synthesize` on the `Voice` actor

```rust
pub struct Synthesize { pub text: String }          // markdown
pub struct Clip { pub bytes: Vec<u8>, pub mime: &'static str /* audio/mpeg */, pub duration_ms: u64 }
impl Message<Synthesize> for Voice { type Reply = DelegatedReply<Result<Option<Clip>, String>>; }
```

- `markdown::spoken(text)` empty → `Ok(None)` (a block of only code fences,
  say). Otherwise a blocking task on the loaded `Synth`: the whole PCM of
  `stream_with` (no cancellation — a clip is short and nobody is listening
  yet), `clip::mp3(pcm, 24_000)` via `mp3lame-encoder` (mono, 48 kbit/s,
  `Quality::Good`, `FlushNoGap`), `duration_ms` from the sample count.
- Model not loaded: the request waits in `pending: Vec<(String,
  ReplySender)>`, drained by `Loaded` (the same load `Speak` triggers);
  `Loaded(Err)` fails them all.
- One `tokio::sync::Semaphore(1)` around the blocking task: two channels
  talking at once do not halve each other's speed by fighting for cores;
  the desktop's `Speak` stream is not gated (its latency is what 43 tuned)
  and simply shares the CPU when it overlaps.
- Not a bus event: the clip is bytes for one caller, and 64 KB of base64
  per block on every subscriber's socket would be the 43 §0.5 mistake in
  reverse.

### 4.4 Prompt block and marker

```rust
pub fn note_text(transcript: &str, duration_ms: u64) -> String  // "[voice note, 12 s] <transcript>"
pub fn is_note(text: &str) -> bool                                // starts_with("[voice note")
```

Registered once at start as `RegisterPrompt { plugin: "voice", text }`:

> ## Voice notes
> A user message that starts with `[voice note, N s]` was spoken, not typed;
> what follows is a transcript, so read through recognition slips and do not
> quote them back. Your prose goes back to the user as speech on the same
> channel, block by block: everything you write before a batch of tool
> calls is delivered the moment those calls start, and the rest when the
> turn ends. Write for the ear — short and conversational, a few sentences,
> just enough to carry the point. No markdown, lists, headings, tables, code
> blocks, URLs or file paths; say names the way you would say them aloud. On
> work that takes several steps, say one short sentence about what you are
> about to do before each batch of tool calls, so the user hears progress
> instead of silence. Keep this spoken style while the user keeps sending
> voice notes; go back to your normal style when they type.

Static text, no session state — cache-safe by 07 §4. The transcript is the
model's only evidence a message was spoken; the block is what makes the
prefix mean something.

### 4.5 Channel contract (Signal and Slack alike)

1. **Input.** For each attachment with `voice_note` or an `audio/*` mime:
   save to the inbox as today, then `Transcribe { path }` on the
   `Transcriber` (looked up by name; absent = plugin disabled). `Ok` with
   text → the line is `voice::note_text(&text, duration_ms)` and this
   message is a voice message. `Ok` with empty text → `[voice note, N s]
   (nothing recognized)`. `Err` or no actor → today's `[attachment: … voice
   note)]` line plus `(transcription unavailable)`, logged at `warn`, and
   the message counts as typed. A typed body alongside a voice note keeps
   both lines, in the order Signal presents them.
2. **Voice turn.** `Turn.voice` is set when the input that started or
   steered the turn was a voice message and cleared by a typed one (the
   last input wins, matching the prompt's "until they type").
3. **Blocks.** `Turn.text` is the *unflushed* markdown. `flush(session)`
   runs at `ToolCallPending` when `text.trim()` is non-empty and at
   `TurnFinished`; it takes the buffer, sends the text the way the channel
   sends text today (Signal: `markdown_to_plain(flatten)` + `send_chunks`;
   Slack: nothing extra — the streamed bubble already shows it), and when
   `Turn.voice` is set, schedules the clip.
4. **Clips in order.** `Turn.clip_tail: Option<JoinHandle<()>>`. Scheduling
   a block: `let prev = tail.take(); tail = Some(spawn(async move { let clip
   = voice.ask(Synthesize { text }).await; if let Some(p) = prev { let _ =
   p.await; } if let Ok(Some(c)) = clip { send(c) } }))`. Synthesis of block
   *n+1* starts while block *n*'s clip is uploading; sends stay ordered.
   Signal sends `send_attachment(to, &c.bytes, "audio/mpeg", None, None)`
   (the transport's `voice_note: Some(true)` for `audio/*` makes it a voice
   bubble); Slack `upload_file(channel, thread_ts, &[UploadFile { name:
   "reply.mp3", mime: "audio/mpeg", bytes }], None)`. Failures are logged;
   the text already landed.
5. **Cancelled or errored turns** flush what is buffered (the user hears
   how far it got) and skip nothing else.

### 4.6 Data on disk

`<data_dir>/models/parakeet-tdt/` — `encoder-model.onnx`,
`decoder_joint-model.onnx`, `vocab.txt` (`config.json`, `nemo128.onnx`
optional; `parakeet-rs` ignores them) and `libonnxruntime.{dylib,so}` (§2.1;
Homebrew's or `$ORT_DYLIB_PATH` also serve). Not on the release mirror yet, same
as 43's checkpoint: the user places it or points `voice.stt_model_dir` at
it. The mirror (43 §4.6) adds `models/parakeet-tdt/<rev>/` when it is built;
`ATTRIBUTION.md` there names NVIDIA (CC-BY-4.0) and Olicorne's requantization.

## 5. Signal (`crates/signal`)

- `save_attachments` gains the transcription branch (§4.5.1) and returns
  `voice: bool` alongside lines and files; `handle_envelope` passes it to
  `Turn` when it calls `send_input`/`steer` (the `Turn` is created at
  `TurnStarted` today; the flag is parked on the session entry and read
  there).
- `Turn` gains `voice: bool` and `clip_tail: Option<JoinHandle<()>>`.
- `handle_agent`: `ToolCallPending` → `flush` if prose is buffered;
  `TurnFinished`/`Error` → `flush` then the reactions as today. The typing
  indicator keeps running across blocks (a flush is not the end).
- `Channel` holds `voice: Option<ActorRef<voice::Voice>>` and `stt:
  Option<ActorRef<voice::Transcriber>>`, resolved lazily by name on first
  use (`rt.lookup`), never awaited at start — `voice` is optional.
- Nothing changes for typed turns except the flush at tool boundaries: a
  "Let me check." before tools now arrives as its own message.

## 6. Slack (`crates/slack`)

- `handle_push`: a `subtype` of `file_share` no longer drops the message
  (voice clips on some workspaces arrive that way); other subtypes
  (`bot_message`, `message_changed`, …) still do.
- `save_files`: the transcription branch (§4.5.1) for `audio/*` mimes.
- `Worker::apply`: `Turn` (bubble) gains `voice` and `clip_tail`;
  `ToolCallPending` and `TurnFinished` schedule the clip of the prose
  written since the previous clip (the bubble already has the text; the
  worker keeps a `spoken: usize` cursor into it). Clips go in the thread
  as `reply.mp3` uploads with no comment.
- Manifest: `files:read` and `files:write` are already required (21 §5).

## 7. RPC and desktop

### 7.1 RPC (`crates/rpc`)

- `transcribe(path: PathBuf) -> Transcript` — `Transcribe` on the
  `Transcriber`; `NOT_RUNNING (−32001)` when the plugin is off, an error
  string otherwise. `path` must be under `<data_dir>/attachments` (the
  `get_attachment` rule).
- `put_attachment` accepts `m4a | mp4 | webm | ogg | opus | mp3 | wav`
  ≤ 16 MB (`MAX_AUDIO`). `agent::check_files` is untouched: audio never
  rides as `files`.
- `voice_status` is unchanged: the mic shows whenever the plugin is up, and
  a missing STT checkpoint surfaces as the `transcribe` error the composer
  toasts. Rejected: an `stt: ModelState` field (a fourth state for
  "unloaded but on disk", read by nobody).

### 7.2 Composer

- A microphone `<Button>` beside Send, shown when `useVoice()` and the
  textarea is empty; Send returns as soon as there is text (the ChatGPT
  arrangement). Click to start, click to stop; while recording the button is
  red with the elapsed seconds, `Escape` discards. `MediaRecorder` with the
  first supported of `audio/mp4`, `audio/webm;codecs=opus`, `audio/ogg;
  codecs=opus`, `audio/webm`; `audioBitsPerSecond: 32_000`.
- Stop → blob → `putAttachment(api, blob, extFor(mime))` → `api.transcribe
  (path)` → `noteText(text, secs)` → the existing `submit` path with that
  text (`steer` while running, else `send_input`), `markSent`. Failures
  toast ("no STT model …" verbatim from the harness).
- Session voice mode: `voiceMode: Set<SessionId>` in the sessions store;
  set by a voice send, cleared by a typed `submit`. Not persisted — a fresh
  window starts silent until the next voice note.
- Since 54 (2026-09-20) this is the `AudioLines` button; the round `Mic`
  button beside it is dictation into the textarea (54 §7), not a voice note.

### 7.3 Speaking blocks (`lib/speech.ts`)

- The events reducer already closes the prose before a tool batch into its
  own assistant entry (`closeTail`), so a block *is* an entry: at
  `ToolCallPending` and `TurnFinished`, for a session in voice mode, the
  newest assistant entry since the user's line is enqueued once (a
  `Map<session, lastSpokenEntryId>` in `speech.ts`; presence = voice mode).
  No char cursor, no store fields — nothing but `speech.ts` reads them.
- `enqueue` chains: `queue = queue.then(() => idle()).then(() => api.speak
  (session, text))` where `idle()` resolves when nothing is speaking and
  `remaining()` has elapsed — so `Speak`'s replace-semantics never cut a
  block off. `Speaking.entry` is set to the assistant entry so the badge
  from 43 shows.
- One `AudioContext` for the module, created or resumed inside the mic
  click (a user gesture) and reused by `start` — iOS WKWebView will not
  start one that was created outside a gesture [INFERENCE, §10 step 6].
- Audio session lifecycle (as built 2026-09-22, iOS): one owner in
  `speech.ts` — `unlock(use)` / `release(use)` over the holders `speak`,
  `dictate` (54), `record`. `record()` takes `record` on the tap and gives it
  back in `MediaRecorder.onstop` (upload and Escape-discard alike) and on a
  failed `getUserMedia`; read-aloud's `stop()` gives back `speak`. The last
  release suspends the context and resets `navigator.audioSession.type` to
  `auto` — a running context or a forced `play-and-record`/`playback` type
  kept the Dynamic Island's Now Playing up after the note was sent.

### 7.4 Bubble

`MessageUser`: `parseNote(text)` (`/^\[voice note, (\d+) s\]\s*/`) →
a row with a microphone icon and `N s`, then the transcript in the normal
bubble. Own sends and history render the same, since the marker is the
text. The original recording is not kept for playback (it is in
`<data_dir>/attachments/` if anyone wants it; §11).

### 7.5 Bundle

- `crates/desktop/Info.plist` (new; the bundler merges it):
  `NSMicrophoneUsageDescription` — "theuth records voice notes you send to your
  harness."
- `ios/Info.ios.plist`: the same key.
- CSP: unchanged — recording and Web Audio are not CSP-gated, and no
  `<audio>` element is introduced.

## 8. Workspace

`Cargo.toml [workspace.dependencies]`: `parakeet-rs = { version = "0.3",
default-features = false, features = ["cpu", "load-dynamic"] }` (+ `ort`
with `load-dynamic`, §2.1), `symphonia = { version =
"0.5", default-features = false, features = ["aac", "isomp4", "mp3", "ogg",
"wav", "pcm", "mkv"] }`, `rubato = "0.16"`, `mp3lame-encoder = "0.2"`.
`crates/voice` takes all four plus `tools` (for `RegisterPrompt`);
`crates/signal` and `crates/slack` take `voice`. `crates/rpc` already has
`voice`. Desktop: no new package.

Build: LAME compiles from source through `cc` (no cmake); `ort` is
`load-dynamic`, so nothing is fetched for it at build and the Linux Docker
build is unchanged. `.cargo/config.toml`'s cmake policy pin from 43 still
covers libopus.

## 9. What phase 3 (the live agent) inherits

`Transcriber` (offline) becomes one of two STT paths — the live one is
Nemotron streaming with VAD gating, same crate, same `decode`/`resample`:
built as `voice::stt::Stream` + `vad::Gate` + `mic::Capture` by 54
(dictation) — phase 3 consumes them, it does not build them. `Synthesize`
stays for channels; the live path is 43's `Speak` stream. The marker and
prompt block are already the "the user spoke" convention. Barge-in is
`Stop` + the agent's `cancel`, as 43 §9 said.

## 10. Phases and checks

1. **`voice`: STT + clip.** `stt.rs` (decode, resample, `Transcriber`),
   `clip.rs` (LAME), `Synthesize`, prompt block, config, `note_text`.
   check: `cargo test -p voice` — decode fixtures (a WAV, the spike's
   M4A), `note_text` round-trip; a `Transcribe` on the smoke WAV returns
   the smoke text; `Synthesize("**hi** there")` → MP3 magic bytes,
   `duration_ms` ≈ 1 000.
2. **Signal.** check: a voice note from a phone → `[voice note, N s] …` in
   the session (`/tmp` harness log), narration text arrives as tool calls
   start, its MP3 voice bubble follows, final text + bubble; a typed
   follow-up gets text only.
3. **Slack.** check: an audio clip in a DM → transcript in the session;
   clip per block in the thread; a `file_share`-subtype event is not
   dropped (log a real one).
4. **RPC + desktop.** `transcribe`, `put_attachment` audio, mic button,
   voice mode, bubble. check: `bun run check`, vitest; in the browser
   (`just ts-desktop`): record → transcript bubble → reply spoken block by
   block; typing ends voice mode.
5. **Bundle.** Info.plist keys; the `.app` records after the OS prompt;
   iOS via TestFlight.
6. **Cross-platform.** Linux/WebKitGTK `MediaRecorder` mime and the
   Ogg/Opus decode path; iOS `AudioContext` outside a gesture.

## 11. Status

Built 2026-09-17 (steps 1–4 of §10). Exercised on an M3 Max:

- `cargo test -p voice` (decode, resample, MP3 sync word, marker), `-p
  signal` (57, incl. prose-before-tools flushed as its own message with the
  typing indicator still live), `-p slack` (89, incl. an audio file without
  the plugin → attachment line, not a voice turn); clippy `-D warnings` on
  `voice`, `signal`, `slack`, `rpc`; `bun run check`, vitest 114/114.
- Headless over RPC against a release build with both models resident:
  `put_attachment(m4a)` → `transcribe` cold 938 ms (dylib + model load
  included), warm 226 ms, transcript byte-identical to the spoken text;
  `/etc/passwd` → "not an attachment"; `.exe` → unsupported type.
- In-process (`voice` smoke, deleted): `Synthesize("Hello **there**. This
  is a [clip](http://x) test.")` → 226 ms, 12 384 B of `audio/mpeg`,
  2.08 s; decoding that clip and transcribing it gives the sentence back.
- Prompt block: a `[voice note, 7 s] …` request that needs tools made the
  model write one sentence ("I'll check the theuth cargo workspace for you -
  let me count the crates and find the biggest one by lines of Rust
  code.") before its first `python` call — captured as block 1 at
  `ToolCallPending`. The desktop renders the input as a bubble with the
  microphone row and `7 s`; the sidebar title drops the marker.

Not exercised: a phone's voice note through Signal or a Slack clip (no
linked device or workspace on the smoke daemon — the transcription branch
and the MP3 send are compiler-checked and unit-tested at their fallbacks);
the microphone in a webview (needs a human and a mic: the browser at
`just ts-desktop`, then the `.app` for the TCC prompt); Linux/WebKitGTK
`MediaRecorder` and the Ogg/Opus decode path; iOS `AudioContext` outside a
gesture; the 10-minute idle unload (watched in code, not in a run).

As-built deviations from v0: `ort` is `load-dynamic` (§2.1); the desktop
speaks per assistant *entry*, not per char cursor (§7.3); `ort_lib` also
accepts `$ORT_DYLIB_PATH` and Homebrew's `libonnxruntime`; the checkpoint
directory doubles as the runtime library's home until the mirror ships both.

## 12. Amendments to 00, 21, 25, 43

- 00 §9: `models/parakeet-tdt/` beside `models/pocket-tts/`. §10 crate
  list: `voice` description gains "speech-to-text (Parakeet TDT)". Workspace
  table: `parakeet-rs | 0.3 | Parakeet TDT ONNX inference (45)`, `symphonia |
  0.5 | audio demux/decode for voice notes (45)`, `rubato | 0.16 |
  resampling to 16 kHz (45)`, `mp3lame-encoder | 0.2 | MP3 clips for
  channels (45)`.
- 21 §3: "one streamed message per turn" + "and, for voice turns, one
  `reply.mp3` per prose block in the thread (45)"; the `file_share` subtype
  is accepted.
- 25 §3 parity row `voice`: "read aloud: yes (43); voice notes in and
  spoken replies block by block: yes (45); dictation to text: no".
  §7: `transcribe`.
- 43 §9: phase 2 is this doc; §4.3 `Voice` gains `Synthesize`.
- README config block: `stt_model_dir`, `stt_idle_secs`.

## 13. Rejected

- **Transcribing in the channel crates** (each with its own decoder): one
  model, one decoder, in the crate that owns audio.
- **A `voice` field on the agent's input types**: the marker line is the
  flag and needs no schema, store or compaction changes.
- **Whole-turn clip** (jarvis): silence until the tools finish.
- **Ogg/Opus clips**: Signal-iOS cannot play them (`#5771`).
- **`message_user` as the narration mechanism**: the model narrates in
  prose already; a tool call is a round trip it forgets to make.
- **Harness-side voice mode for the desktop**: the client has the deltas,
  the gesture, and the `speak` RPC; the harness would need to know which
  client is listening.
- **Client-side PCM upload**: ten times the bytes of the recorder's AAC
  through the relay for no gain — the harness decodes AAC anyway for
  Signal.
- **Slack's native transcript**: Slack-only; ours runs everywhere.
- **Streaming STT for notes**: a finished note transcribes in 3 % of its
  length; streaming is the live agent's problem.

## 14. Open

| # | question | default |
|---|---|---|
| 1 | Flush at tool boundaries on Signal for typed turns too | yes — a progress line is a message either way; revert to voice-only if it reads as spam |
| 2 | Slack clip filename | `reply.mp3`; a timestamped name if the thread gets confusing |
| 3 | STT idle unload | 600 s; `0` to keep resident |
| 4 | Note length | pieces of 180 s; silence-aligned splitting when a real long note shows a cut word |
| 5 | Keep the original recording for playback in the desktop bubble | no; it stays in `attachments/` |
| 6 | Hold-to-record on touch | no; tap to start/stop everywhere |
| 7 | MatMulNBits int8 checkpoint instead of the SmoothQuant one at hand | when downloaded; same loader |
