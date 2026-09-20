# 54 — Dictation: speak into the composer, in the TUI and the desktop

Status: design v0 · 2026-09-20 · extends `00-architecture.md` §4 (plugin
`voice` grows), §7 (RPC), §9 (data dir `models/nemotron-streaming-en/`);
`43-voice.md` §9 (the microphone seam — deviates, §0.2); `45-voice-notes.md`
§7.2 (the composer's mic button changes meaning); `25-desktop.md` §7 (RPC
additions); `02-models-and-menus.md` §4 (a new modal state in the TUI editor).
Proof of design for `meetings/02-live.md` §0.1b–1c: the streaming STT stack
(`voice::stt::Stream`, `voice::vad`, `voice::mic`) is built here first and
53 reuses it unchanged. §11 lists amendments.

Scope: press **Ctrl+D** (or the microphone button) and speak; words appear at
the caret in the composer while you talk — muted while the sentence is still
open, full color when it closes — and never touch text that was already
there. Press again to stop. The model is loaded before you press anything.
Works in the TUI and the desktop app against the same daemon.

Not in this doc: voice *notes* (45 — they stay, behind their own button);
barge-in / the live voice agent (43 §9); dictation from a phone through the
relay (§0.2 names the seam); commands by voice ("send", "new line"); a
dictation model downloader (43 §0.3's mirror is still unbuilt; the user
places the files).

## 0. Decisions in one screen

1. **Dictation lives in `voice`, as a third actor `Dictation`
   (`DICTATION_ACTOR = "dictation"`), beside `Transcriber` (file STT) and
   `Voice` (TTS).** Voice is "speech in, text out, per message" (45); a
   live dictation is one message being typed. 53 argued a 3-hour stateful
   recorder is *not* voice's job and got its own crate; this is not that.
   Rejected: a `dictation` plugin (a config section, a lifecycle and a
   registry name for one mic and one model that voice already owns the
   sibling of); putting it on the `Voice` TTS actor (a mailbox that carries
   80 ms audio frames out should not also carry 20 ms frames in).
2. **The daemon owns the microphone, natively, for every frontend.** A
   `voice::mic` thread opens the default input with `flexaudio` 0.2
   (`StreamConfig { kind: SourceKind::Mic, output: OutputFormat { sample_rate: 48_000, channels: 1 }, .. }`) and
   hands `f32` chunks to the actor. The TUI has no other way to a mic; the
   desktop gets the same path for free; 53 phase 1 already commits the
   daemon to the mic. This **deviates from 43 §9** ("a microphone path adds
   `AudioIn` from the webview, Opus in the other direction"): *remote*
   (relay-linked) clients — a phone through the relay — capture in the
   webview and stream mono i16 PCM (base64, ~100 ms per `dictation_audio`
   call, at the device rate — the tap rides `speech.ts`'s single
   `AudioContext`, since WKWebView runs a second, rate-forced context
   silent — resampled to 16 kHz in the daemon) into the same pipeline;
   `dictation_start(sample_rate)` selects `AudioSource::Remote`, `null`
   keeps the daemon mic for local peers. The `AudioWorklet` module is a
   `blob:` URL, so the Tauri CSP carries `script-src … blob:`. Raw PCM is
   ≈1 Mbit/s at 48 kHz; ponytail: decimate in the worklet, or Opus via
   WebCodecs `AudioEncoder` once the WKWebView floor reaches iOS 26.
   Nothing for the TUI over the wire — it is in-process.
   Permission: macOS attributes the TCC prompt to the responsible process —
   the `.app` when it spawned the daemon (`Info.plist` already carries
   `NSMicrophoneUsageDescription`, 45 §7.5), the terminal app for a `wth`
   launched from a shell [INFERENCE; §10 check 8 verifies both].
3. **STT is the cache-aware streaming transducer from `meetings/02-live.md`
   §0.1b: `nemotron-speech-streaming-en-0.6b` (int8) via
   `parakeet_rs::Nemotron`, wrapped as `voice::stt::Stream`.** 16 kHz mono,
   560 ms windows (8 960 samples), `transcribe_chunk` returns only new,
   punctuated, cased text; encoder cache + decoder state persist across
   windows; `reset()` at turn boundaries. Greedy RNNT emits a token once —
   the text is **append-only**, which is what makes "muted then saturated"
   cheap (§5): a partial is not a guess that gets rewritten, it is text of
   an utterance the VAD has not closed yet. Rejected: Parakeet TDT with
   endpointing or rolling re-decode (02-live §0.1b rejected list); Apple
   `SFSpeechRecognizer` (a Swift surface in the shell, nothing for the TUI
   or headless, one engine everywhere is the standing decision).
4. **VAD is `wavekat-vad` with the TEN-VAD backend (02-live §0.1c), same
   rules:** onset = 10 frames (160 ms) > 0.5 with a 320 ms pre-roll; close
   = 300 ms < 0.5, or 15 s, or stop; the model is fed only between onset
   and close (+ one zero window to flush lookahead); `reset()` after ≥ 2 s
   of silence. Utterances are the unit the UI colors.
5. **Prewarm is a frontend's first RPC after connect, and the model then
   stays resident.** `dictation_prewarm` loads the `Stream` session (≈ 0.9 GB
   int8, seconds) and the VAD in `spawn_blocking`, publishes
   `VoiceEvent::DictationModel { state }`, and answers the current state.
   The TUI calls it at start; the desktop calls it in `loadInfo` after the
   socket opens. Nothing happens for a headless daemon nobody dictates into,
   and a server without a mic never pays the RAM. Once loaded the session is
   `reset()` between dictations and never dropped (`ponytail:` no idle
   unload, same call as 43 §4.3 for TTS; add `voice.dictation_idle_secs` if
   RSS complaints arrive). The mic device is **not** opened at prewarm (the
   OS indicator would be lit all day); opening it costs tens of
   milliseconds at start. Rejected: loading at plugin start unconditionally
   (0.9 GB on every headless daemon); loading on the first Ctrl+D (the
   requirement is "ready when I press it").
6. **Insertion is at the caret, insert-only, and the frontend owns it.**
   The daemon knows nothing about the composer. Each frontend keeps
   `before` (text up to the caret at start), `after` (text from the caret),
   and the list of utterances; the composer value is always `before +
   region + after` where `region` is the dictated text with a separating
   space added *inside the region* when needed (§5.1). `before` and `after`
   are never modified — not a character, not a space. Stop leaves the caret
   at the end of the region. Esc during dictation discards the region and
   restores `before + after` exactly.
7. **While dictating, the editor is read-only and the caret is hidden; the
   dictation button's slot becomes the level meter.** Typing while
   dictating would need the region to move under concurrent edits; nobody
   types and talks at once, and "the edit box is busy" is honest UI.
   Ctrl+D (stop, keep), Esc (stop, discard) and Ctrl+C (TUI quit) are the
   only keys that do anything. Rejected: live editing during dictation
   (offset bookkeeping for a case with no user).
8. **The two buttons are visually and verbally distinct.** The existing
   round microphone button (`Mic`) *becomes* dictation — tooltip
   **Dictation (Ctrl+D)** — and is shown whenever the composer is editable,
   text or no text, because dictation inserts. A new button (`AudioLines`)
   with tooltip **Voice note** keeps 45's behaviour (record → transcribe →
   send as `[voice note, N s]`) and is shown only when the composer is
   empty, as the mic was. Both render as a real `Tooltip`, not an
   `aria-label` alone (the composer has none today).
9. **Ctrl+D everywhere, literally Ctrl.** TUI: unbound today (recon §2.1);
   iocraft's `TextInput` swallows only Ctrl+A/E. Desktop: `hotkeys.ts`
   maps `mod` to ⌘ on Apple; this binding gets an explicit `ctrl: true`
   so it is Ctrl on every OS as asked, `inEditable: true`, and
   `preventDefault` stops macOS's Emacs-style forward-delete in the
   textarea. Rejected: ⌘D on macOS (the user said Ctrl+D; ⌘D is bookmark
   muscle memory anyway).
10. **One dictation at a time per daemon (one mic); events carry an id.**
    A second `dictation_start` while one runs → `Err("already dictating")`.
    A TUI and a desktop on the same daemon both see the events; each acts
    only on the id it started. Nothing is recorded to disk: dictation audio
    is transient. Hard stop at `voice.dictation_max_secs` (default 300) so a
    forgotten hotkey does not leave the mic open.

## 1. Why

The composer is where the user spends the day, and the mic button today
sends a whole message (45). Dictating *into* the text — before a sentence you
half-typed, in the middle of a paragraph, then editing by hand — is the
missing half of speech input, and it is the smallest surface on which the
live STT stack that 53 needs (streaming model, VAD gating, native mic) can
be built and dogfooded before a meeting depends on it.

## 2. Facts relied on (recon 2026-09-20)

### 2.1 Harness

- `crates/voice/src/lib.rs:216-257` `VoiceEvent { VoiceModel, SpeechStarted, SpeechAudio, SpeechFinished }`, `#[serde(tag = "type")]`, ts-exported; `:260-285` `VoiceConfig { enabled, voice, model_dir, stt_model_dir, stt_idle_secs }`; `:573-623` `VoicePlugin::start` spawns `Transcriber` (`"transcriber"`) and `Voice` (`"voice"`), then `RegisterLifecycle` + `RegisterPrompt`.
- `crates/voice/src/stt.rs:34-73` `ort_lib(dir)` finds `libonnxruntime` and `ort::init_from` is called once (lib comment `Cargo.toml:57`); `:213-249` file → decode → 16 kHz → `ParakeetTDT::transcribe_samples`. No PCM-in entry point; no `Engine`/`Stream` type yet.
- No microphone anywhere in `crates/` (`cpal|flexaudio|coreaudio` → none). `parakeet-rs` 0.3.7 in `Cargo.lock` exposes `Nemotron` (`transcribe_chunk`, `reset`, `mode`).
- `crates/rpc/src/api.rs:761-774` `speak`/`stop_speaking`/`voice_status`/`transcribe`; `:1007-1021` `voice()`/`transcriber()` lookups mapping `NOT_RUNNING` to "voice is disabled"; `bridge.rs:30,111,153` forwards `VoiceEvent`; `export.rs:171-174` exports the voice types.
- Warm-up precedent: `crates/browser/src/lib.rs:99-143` `warmup()` before plugins start (main thread, sync) — the wrong shape here (§0.5), cited so nobody copies it.

### 2.2 TUI (`crates/tui`)

- Composer = iocraft `TextInput` (fork `skymoore/iocraft` `mask-0.9`): `ui.rs:100` `input: State<String>`, `:101` `TextInputHandle` (cursor **byte** offset, `cursor_offset()`/`set_cursor_offset()`), `:590-603` render (border Round Blue, `multiline`, `auto_grow`, `on_change`), `:253-259` `insert_at_cursor` (used by paste, Ctrl+V). External `value` change: pure insertion at the cursor advances it; any other diff sends the cursor to the end (`text_input.rs:670-703`) — so a region that changes must call `set_cursor_offset`.
- Keys: `ui.rs:241-478` `use_terminal_events`, precedence Ctrl+C → Ctrl+O → approval → secret → ask → todos → … → editor. Ctrl+D unbound; kitty `DISAMBIGUATE_ESCAPE_CODES` pushed (`:126-132`) so Ctrl+D arrives as a distinct key.
- Focus: `model.rs:3261` `input_focused()` (false during approval/menus/inspect); `:3275` `modal_menu()`.
- Frame loop `ui.rs:117-233`: 16 ms interval, skips writes unless events pending or `animating()` (`model.rs:3820`); `frame` counter, `spinner()` `:3924` (braille), `tick()` `:3933`. Dim = `Color::DarkGrey` / `Weight::Light`; `MixedText` for per-span color (`ui.rs:1359, 1596`).
- Bus: `lib.rs:54-83` `Bridge` subscribed to 19 event types via `forward!` (`:152-182`) — **not** `VoiceEvent`; `tui/Cargo.toml` has no `voice` dep. Commands: `Command` (`model.rs:283`) → `execute` (`lib.rs:204-758`) → `rpc::api::Server` in-process → `UiEvent` (`model.rs:44`) → `Model::apply` (`:3970`).

### 2.3 Desktop (`crates/desktop/ui`)

- `Composer.tsx:113` controlled `text`; `:123` textarea `ref`; `:488-508` `<Textarea>` (CSS `field-sizing: content`, `max-h-[200px]`); `:140-144` `insert(s)` splices at `selectionStart/End`; `:232-303` `submit(spoken?, interject)`; `:236` voice mode enters on a spoken send.
- Mic today: `:117` `voice = speech.useVoice()`; `:329-352` `record()` (`speech.unlock()`, `getUserMedia`, `MediaRecorder`); `:313-327` `sendNote` → `putAttachment` → `api.transcribe` → `submit(noteText(..))`; `:510-543` right-hand button ternary: Stop / mic (`Mic`, `size-9 rounded-full`, `aria-label` only, shown when `voice && !priv && (recording || !text.trim())`) / Send. Escape while recording discards (`:354-368`).
- `lib/hotkeys.ts:5-12` `Hotkey` union; `:14-20` `Binding { code, mod, shift, event, inEditable }`; `:92-119` one window `keydown` → `CustomEvent`; `mod` = ⌘ on Apple. No `KeyD`.
- `lib/speech.ts:20-34` one `AudioContext` (`unlock()` in a gesture); `:203-208` voice-mode map; `:277-296` `installSpeechListeners` (`on('VoiceModel'|…)`); `:298-308` `useVoice()` = `voice_status` query, `-32001` = plugin off; `:246-250` `useSyncExternalStore` pattern.
- `App.tsx:139-187` `boot`: `makeApi(client)` `:161`; `onState('open')` `:167-175` → `loadInfo(st, api)` on first open, `reload` on reconnect; `attach` subscribes `subscribe_events(null)`.
- `rpc/extra.ts:264-273` hand-written `VoiceEvent` duplicate of `bindings/VoiceEvent.ts`; `events.ts:29,66` uses the hand-written one. `store/sessions.ts:535-567` `on(type, cb)`.
- UI kit: radix `Tooltip` (`components/ui/tooltip.tsx`; provider mounted by `SidebarProvider`; canonical use `sidebar/SearchTrigger.tsx:14-28`); `Button` cva variants; lucide-react 1.47 has `Mic`, `MicVocal`, `MicAudioLines`, `AudioLines`, `Square`; tokens `text-fg-secondary`, `text-fg-tertiary`, `text-fg-accent`. No canvas/level-meter code exists.

### 2.4 Libraries

| crate | version | facts | use |
|---|---|---|---|
| `parakeet-rs` | 0.3.7 (pinned) | `Nemotron::from_pretrained(dir, None)`, `transcribe_chunk(&[f32]) -> String`, `reset()`, `mode()`; int8 export `lokkju/nemotron-speech-streaming-en-0.6b-int8` = `encoder.onnx` 880 MB + `decoder_joint.onnx` + `tokenizer.model`; CPU EP (CoreML unstable per its README) | `voice::stt::Stream` |
| `wavekat-vad` | 0.1.17, `default-features = false, features = ["ten-vad"]` | `VoiceActivityDetector::process(&[i16], 16_000) -> f32`, 256-sample frames, `FrameAdapter`; `ort = "2.0.0-rc.12"` unifies with the workspace `=rc.13`; model fetched at build unless `TEN_VAD_MODEL_PATH` | `voice::vad` |
| `flexaudio` | 0.2 (the only crates.io release; no 0.3 exists — corrected 2026-09-20 while building) | `open(StreamConfig { kind: SourceKind::Mic, output: OutputFormat { sample_rate: 48_000, channels: 1 }, ..Default::default() })` → `Stream::start()`, `poll_chunk()` → 20 ms `AudioChunk { data: Vec<f32>, frames, .. }` interleaved; dedicated thread | `voice::mic` |
| `rubato` | 0.16 (in tree) | `FftFixedIn` 48 → 16 kHz, one stateful instance per dictation | resample |

## 3. Shape

```
 frontend (TUI in-process / desktop over WS)            wth daemon: voice plugin
 ┌──────────────────────────────────────────┐          ┌──────────────────────────────────────────────┐
 │ connect ──dictation_prewarm──────────────────────────▶ Dictation actor: Stream::open + Vad (blocking)│
 │                                          │◀─ DictationModel{Ready} ─┤                              │
 │ Ctrl+D / [Mic] ──dictation_start─────────────────────▶ mic thread ─chunks─▶ task: resample → VAD    │
 │   anchor caret, lock editor, meter on    │◀─ DictationStarted{id} ──┤       onset/pre-roll → Stream │
 │   region = utterances (muted / final)    │◀─ DictationLevel{levels} ┤       .feed(window) → text    │
 │                                          │◀─ DictationText{utt,text,final}                          │
 │ Ctrl+D / [meter] ──dictation_stop(id)────────────────▶ flush window, close, drop mic, reset()       │
 │   caret → end of region, unlock          │◀─ DictationFinished{id, Done}                            │
 └──────────────────────────────────────────┘          └──────────────────────────────────────────────┘
```

## 4. Plugin `voice`, additions

### 4.1 Config

```yaml
# voice:
#   dictation_model_dir: ~       # nemotron-speech-streaming-en ONNX dir; default <data_dir>/models/nemotron-streaming-en
#   dictation_max_secs: 300      # a running dictation stops itself here
```

`enabled: false` (the existing key) removes dictation with the rest of voice:
no actor, `dictation_*` answer "voice is disabled", frontends show no button
and Ctrl+D says so. No prewarm key: prewarm is the frontend's request, not a
daemon policy.

### 4.2 Files

```
crates/voice/src/stt.rs      + pub struct Stream { open(dir), feed(&[f32; 8960]) -> String, reset() }, WINDOW; ort_lib made pub(crate)-shared
crates/voice/src/vad.rs      Gate: TEN-VAD behind FrameAdapter; onset/pre-roll/close/turn rules (pure over frames; unit-tested)
crates/voice/src/mic.rs      Capture::open() -> (Capture, Receiver<Chunk>) ; std::thread + flexaudio Mic; drop = close
crates/voice/src/dictation.rs  Dictation actor, messages, the per-dictation task, level meter
crates/voice/src/lib.rs      DICTATION_ACTOR, DictationId, VoiceEvent variants, VoiceConfig keys, spawn in start
```

Dependencies added to `voice/Cargo.toml`: `flexaudio`, `wavekat-vad` (as
§2.4), `rtrb` not needed (an `std::sync::mpsc` of 20 ms chunks is fine for
one consumer — 53 phase 1's encoder wants rings; dictation does not). Root
`[workspace.dependencies]` entries carry `# 54` comments; 53 phase 1/2 then
find them present.

### 4.3 Commands (`DICTATION_ACTOR = "dictation"`)

```rust
pub struct DictationId(pub uuid::Uuid);                      // v7

/// Load the streaming model and the VAD if not loaded; never opens the mic.
pub struct Prewarm;                                          // ask → ModelState (current; Loading if in flight)
/// Where a dictation's audio comes from.
#[serde(tag = "source", rename_all = "snake_case")]
pub enum AudioSource { Mic, Remote { sample_rate: u32 } }    // Mic = the daemon's own (§0.2); Remote = the client streams PCM with `PushAudio`, mono i16 at `sample_rate` Hz
/// Start. Errors: "already dictating", "voice is disabled", model Missing/Failed, mic open failure (Mic), "sample_rate must be a multiple of 50" (Remote).
pub struct StartDictation { pub source: AudioSource }        // ask → Result<DictationId, String>
/// Mono i16 PCM at the `sample_rate` given at start. Errors: "not dictating" (unknown/finished id), "not a remote dictation".
pub struct PushAudio { pub id: DictationId, pub pcm: Vec<i16> } // ask → Result<(), String>
/// Stop `id` (idempotent; unknown id → Ok). Flushes the open utterance first.
pub struct StopDictation { pub id: DictationId }             // ask → Result<(), String>
/// What is loaded and what is running.
pub struct DictationStatus;                                  // ask → DictationState { model: ModelState, active: Option<DictationId> }
```

`ModelState` is 43's enum, reused (`Missing | Loading | Ready | Failed`).
`Prewarm` from two frontends at once loads once (the actor holds a
`Loading` flag and a waiter list). `StartDictation` while `Loading` waits
for the load rather than failing — the user pressed the key one second
after launch; the reply comes when the mic is open. For `Remote` no
`Capture` is opened: the actor rechunks pushed PCM into 20 ms
(`sample_rate / 50`-sample) `f32` chunks — a partial tail waits for the
next push — and the task's resampler is built for `sample_rate` → 16 kHz
(none at 16 000); everything downstream is the `Mic` path unchanged.

### 4.4 Events (added to `VoiceEvent`)

```rust
/// Streaming model lifecycle (distinct from `VoiceModel`, which is TTS).
DictationModel { state: ModelState },
/// The mic is open and the VAD is listening. Frontends switch to the dictating view on this, not on the RPC reply.
DictationStarted { id: DictationId },
/// RMS per 20 ms of the last 160 ms of mic audio, 0.0–1.0, linear. 8 values, ~6 events/s. Not persisted.
DictationLevel { id: DictationId, levels: Vec<f32> },
/// Text of utterance `utterance` (0-based within this dictation) so far. Append-only per utterance;
/// `final: true` is sent once, with the closed utterance's full text (may be empty → the UI drops it).
DictationText { id: DictationId, utterance: u32, text: String, final: bool },
/// Mic closed. `Done` after a stop, `Stopped` after the max-length stop, `Error` for mic/model failure.
DictationFinished { id: DictationId, reason: FinishReason },
```

Events carry no `session` (a dictation is not in a session);
`subscribe_events(null)` — what the desktop already uses — sees them, and
the TUI is in-process.

### 4.5 The dictation task (`dictation.rs`)

`StartDictation` → `mic::Capture::open()` (48 kHz mono `f32`, its own
`std::thread`, chunks over an mpsc; failure → `Err` to the caller, nothing
published) → a tokio task owning the `Stream` (moved out of the actor for the
duration; `Prewarm`/`Start` while active see `active`), the `vad::Gate`, one
`rubato` resampler, a 160 ms level accumulator, and a `CancellationToken`.
Publish `DictationStarted`. Loop, exactly `meetings/02-live.md` §2 steps 1–5
with these bindings:

- every 20 ms chunk → resample → `i16` frames to the VAD (256 samples), `f32`
  kept in a rolling buffer; RMS per chunk pushed to the level accumulator;
  every 8 chunks → `DictationLevel`.
- onset (10 frames > 0.5) → utterance `n` opens at onset − 320 ms; windows
  of 8 960 samples from there → `spawn_blocking(stream.feed(window))` (one in
  flight; the `Stream` moves into the blocking call and back) → non-empty
  text appended to `utt_text` → `DictationText { n, text: utt_text, final:
  false }`.
- close (300 ms < 0.5 | 15 s | stop requested) → pad + feed the partial
  window, feed one zero window, append → `DictationText { n, text, final:
  true }`; `n += 1`. Feeding stops.
- ≥ 2 s silence → `stream.reset()`, `vad.reset()`.
- `StopDictation` or `dictation_max_secs` → cancel token → the loop closes
  an open utterance as above, drops the mic (thread exits, device closes),
  `stream.reset()`, returns the `Stream` to the actor, publishes
  `DictationFinished { Done | Stopped }`.
- panic in `feed` (`catch_unwind`, as `Transcriber`) or mic thread error →
  `DictationFinished { Error { message } }`, `Stream` dropped and
  `DictationModel { Missing }` so the next `Prewarm` reloads.

The actor's `Stream` is loaded once by `Prewarm` (or lazily by the first
`StartDictation`) in `spawn_blocking` after `ort_lib` + `ort::init_from`,
and the `wavekat_vad::TenVad` is created **after** that init (02-live §0.1c:
one `ort` in the process, initialised once). `Stream::open` failure →
`DictationModel { Failed { message } }`; a missing directory →
`Missing` with the expected path in the message.

## 5. Frontend contract (both UIs implement exactly this)

### 5.1 The region

```
state at start:  before = value[..caret]     after = value[caret..]     utts: Vec<{ text, final }> = []
lead  = if before is non-empty and !before.ends_with(whitespace) { " " } else { "" }
trail = if after is non-empty and !after.starts_with(whitespace | .,;:!?)) { " " } else { "" }
body  = utts.map(text).filter(non-empty).join(" ")
region = if body.is_empty() { "" } else { lead + body + trail }
value  = before + region + after
```

`DictationText { utterance: n, text, final }` sets `utts[n] = { text, final }`
(growing the vector; `final` with empty text removes the entry). Render:
`before` normal, `lead` normal, each utterance `final ? normal : muted`,
`trail` normal, `after` normal — muted = `text-fg-tertiary` (desktop),
`Color::DarkGrey` (TUI). The caret is hidden; the editor accepts no input.

Stop (`Ctrl+D`, the meter button, `DictationFinished` from the daemon for
any reason): value = `before + region + after`, caret = `len(before + lead +
body)` — after the last dictated word, before `trail`, so typing continues
the sentence (§10 check 3) — editor unlocked and focused, all utterances
rendered normal even if a `final` never arrived for the last one (the daemon
flushes before `Finished`; this is the belt). Discard (`Esc`): `dictation_stop(id)`, value
= `before + after`, caret = `len(before)`.

A pure function with this signature exists in both codebases and is the
unit-tested piece: `splice(before, after, utts) -> (value, caret)`
(`crates/tui/src/dictation.rs`, `ui/src/lib/dictation.ts`). Cases that must
hold: empty `before`/`after`; `before` ending in a space (no double space);
`after` starting with `.` (no space before punctuation); mid-word caret
(`hel|lo` + "there" → `hel there lo` — the user put the caret there; we
never merge into their word); multi-byte text before the caret (TUI byte
offsets land on char boundaries because `before` is taken at the caret,
never recomputed); all utterances empty → value unchanged, caret unchanged.

### 5.2 Lifecycle

```
idle ──Ctrl+D / button──▶ starting (button disabled, meter idle) ──DictationStarted{id}──▶ dictating
dictating ──Ctrl+D / button──▶ stopping (dictation_stop; keep rendering) ──DictationFinished──▶ idle
dictating ──Esc──▶ discard → idle           dictating ──Finished{Error}──▶ idle + toast/status line
starting ──Err (already dictating / disabled / model missing)──▶ idle + message
```

Both frontends call `dictation_prewarm` once per connection and render the
button disabled with the reason while `Missing`/`Loading`/`Failed`
(`DictationModel` events update it). Ctrl+D while disabled shows the reason;
the TUI in its status line, the desktop as a toast.

## 6. TUI

- `tui/Cargo.toml` gains `voice` (types); `lib.rs` `Bridge` subscribes to
  `VoiceEvent` (`forward!(VoiceEvent => Voice)`), `UiEvent::Voice`;
  `Model` gains `dictation: Option<Dictating { id, before, after, utts,
  levels: VecDeque<f32> (last 24), phase }>` and `dictation_model:
  ModelState`; `apply` handles the five variants (ids not ours ignored).
- `Command::DictationPrewarm | DictationStart | DictationStop(id)` →
  `execute` arms calling `api.dictation_prewarm/start/stop`. Prewarm is sent
  once from `run_commands` startup (beside the existing first commands).
- Keys (`ui.rs` `use_terminal_events`): a `dictation` arm **after Ctrl+C
  and before the approval modal**: while `m.dictation.is_some()` — Ctrl+D →
  `DictationStop`; Esc → discard + `DictationStop`; everything else
  swallowed. Ctrl+D while idle and `input_focused()` → snapshot `before`/
  `after` from `input`/`handle.cursor_offset()` (byte offset, already a char
  boundary) → `DictationStart`. `input_focused()` returns `false` while
  dictating (the `TextInput` shows no cursor).
- Render (`ui.rs:590-603`): while dictating, the same bordered box renders
  `MixedText` per §5.1 instead of `TextInput` (Round border, colour
  `Color::Blue` → `Color::Cyan` while dictating so the box itself says
  "listening"); on stop, `input.set(value)` then
  `handle.write().set_cursor_offset(caret)` in the same tick (iocraft would
  otherwise send the cursor to the end).
- Animation: the header line above the editor (`ui.rs:587`, `m.header()`)
  gains, while dictating, `● dictating  ▁▃▆█▆▃▁▂▄…  00:12` — a 24-cell
  meter of `▁▂▃▄▅▆▇█` glyphs from `levels` (newest right, log-scaled,
  colour Cyan; the `●` blinks on `frame / 30`), the elapsed time, and
  ` (Ctrl+D stop · Esc discard)` in DarkGrey. `animating()` gains
  `dictation.is_some()` so the 16 ms loop redraws. `/dictate` in
  `COMMANDS` toggles the same (menus, 02).
- Status line reasons: "dictation: model loading…", "dictation: model
  missing (~/.wth/models/nemotron-streaming-en)", "voice is disabled".

## 7. Desktop

- `rpc/api.ts` `Api` + `METHODS`: `dictation_prewarm`, `dictation_start`,
  `dictation_audio`, `dictation_stop`, `dictation_status`. `rpc/events.ts` switches `VoiceEvent`
  to `@/rpc/bindings/VoiceEvent` (the hand-written duplicate in `extra.ts`
  is deleted — its header asks for exactly that) after `just ts-desktop`.
- `lib/dictation.ts`: a module store (`useSyncExternalStore`, as
  `speech.ts:246`) with `{ model: ModelState, active: { id, before, after,
  utts, levels } | null, phase }`, `splice()` (§5.1), `installDictationListeners()`
  (`on('DictationModel'|'DictationStarted'|'DictationLevel'|'DictationText'|'DictationFinished')`),
  `start(api, before, after)`, `stop(api)`, `discard(api)`. `App.tsx`
  mounts the listeners beside `installSpeechListeners` and calls
  `api.dictation_prewarm()` in `loadInfo` (first open and every reconnect;
  idempotent server-side).
- `lib/hotkeys.ts`: `Binding` gains `ctrl?: boolean` (literal Ctrl,
  bypassing the `mod` mapping); `{ code: 'KeyD', ctrl: true, event:
  'wth:dictate', inEditable: true }`; `Hotkey` union gains `'wth:dictate'`.
  The handler's "bail on the other modifier" rule is skipped for bindings
  with `ctrl: true`.
- `Composer.tsx`:
  - `wth:dictate` listener (gated on `focused`, like `wth:focus-composer`):
    idle → `start(api, text.slice(0, ta.selectionStart), text.slice(ta.selectionStart))`
    (a selection is collapsed to its start; nothing is replaced); dictating
    → `stop`. Escape while dictating → `discard` (the existing Escape
    listener pattern, `:354-368`). `onKeyDown` returns early while dictating.
  - While `active`: the `<Textarea>` is replaced by a `<div>` with the
    textarea's classes plus `whitespace-pre-wrap break-words` and
    `aria-live="polite"`, rendering §5.1's spans (`text-fg-tertiary` for
    open utterances, a 150 ms `transition-colors` so the saturation is
    visible, not a flicker). On stop: `setText(value)`, then in an effect
    `ta.setSelectionRange(caret, caret); ta.focus()`. Rejected: a mirrored
    highlight layer behind a transparent textarea (scroll/wrap sync bugs for
    a read-only moment); `contenteditable` (rewrites the composer).
  - Buttons (`:510-543`): the right-hand slot becomes `[voice note?] [dictation | meter] [send | stop]`:
    - **Dictation**: `<Tooltip><TooltipTrigger asChild><Button variant="ghost" className="size-9 rounded-full" aria-label="Dictation"><Mic/></Button></TooltipTrigger><TooltipContent>Dictation (Ctrl+D)</TooltipContent></Tooltip>`; shown when `voice && !priv && !running`; disabled with the `ModelState` reason in the tooltip while not `Ready`.
    - **Meter** (replaces the dictation button while `active`): `<canvas>` 96 × 36 px inside the same round-ended button shape (`rounded-full bg-fg-accent/10`), click = stop, tooltip "Stop dictation (Ctrl+D)". Drawing (`components/chat/LevelMeter.tsx`): a ring of the last 24 `levels` values (24 × 20 ms = 480 ms of audio scrolling right-to-left, newest at the right), one bar per 3 px with 1 px gap, height = `4 + 28 · clamp(log10(1 + 9·level), 0, 1)`, mirrored around the vertical centre with `roundRect` caps, accent colour, alpha fading toward the left; a `requestAnimationFrame` loop interpolates between packets (levels arrive 6×/s in bursts of 8, the meter renders 60×/s) and idles to a flat 4 px line after 400 ms without a packet; `devicePixelRatio`-scaled. Colour tokens from CSS variables via `getComputedStyle` so themes apply.
    - **Voice note**: `<Button variant="ghost" …><AudioLines/></Button>` with tooltip "Voice note", shown when `voice && !priv && !text.trim() && !active`; the existing `record()`/`stopRecording()`/`sendNote()` move behind it unchanged (red `Square` + seconds while recording, as today).
    - Send / Stop as today. While `active`, Send is disabled.
- Touch (`touch()`): the hotkey is desktop-only; the buttons work as-is.
  Relay clients (the phone): the button works — `lib/dictation.ts`
  `start(api, before, after, capture)` with `capture = true` opens the
  phone's mic with `getUserMedia` + an `AudioWorklet` and streams ~100 ms
  batches of i16 LE base64 with `dictation_audio`; `Composer` passes
  `capture = useHarnesses.getState().active !== LOCAL`, so the harness-host
  desktop keeps `dictation_start(null)` and the daemon mic. A dropped
  connection stops the dictation server-side (§8). 25 §3 parity table row
  `dictation on the phone` reads yes.

## 8. RPC (`crates/rpc/src/api.rs`)

| method | params | returns |
|---|---|---|
| `dictation_prewarm` | — | `ModelState` |
| `dictation_start` | `sample_rate: Option<u32>` | `DictationId` — `null` = harness mic (local peers only); `Some(r)` = `AudioSource::Remote { sample_rate: r }` (any peer) |
| `dictation_audio` | `DictationId`, `pcm: String` (base64 LE i16 mono) | `()` |
| `dictation_stop` | `DictationId` | `()` |
| `dictation_status` | — | `DictationState` |

Through `fn dictation() -> RpcResult<ActorRef<voice::Dictation>>` beside
`voice()` (`NOT_RUNNING` → "voice is disabled"). `dictation_start` is
declared `with_extensions`; only `dictation_start(null)` from a relay/link
peer (the `link::RpcPeer` the listener puts into the request,
`docs/plugins.md` §4.2) answers `failed("microphone is on the harness host;
pass sample_rate to stream audio")` — prewarm, audio, stop and status are
open to any peer. A remote-source dictation is stopped when its connection
closes (`RpcPeer::closed()`), so a dropped phone never holds the
one-dictation slot. `VoiceEvent` is already forwarded; `export.rs` gains
`DictationId`, `DictationState`.

## 9. Data on disk

`<data_dir>/models/nemotron-streaming-en/` — `encoder.onnx`,
`decoder_joint.onnx`, `tokenizer.model` (int8; the fp32 export adds
`encoder.onnx.data`), placed by the user; `voice.dictation_model_dir` to
point elsewhere. Nothing else: no audio, no transcripts — dictated text
lives only in the composer until the user sends it.

## 10. Checks

Each step ends with a runnable check (00 §12).

1. **`voice::stt::Stream` + `vad::Gate` + `mic::Capture`.** check: `cargo
   test -p voice` — `Stream` fed the smoke WAV in 8 960-sample windows plus
   one zero window returns the smoke sentence punctuated; `Gate` over a
   synthetic frame sequence (silence, 5 frames speech, silence, 12 frames
   speech, 20 frames silence, …) yields exactly one onset at the 10th
   frame with the 320 ms pre-roll, one close at +300 ms, and a turn reset
   at +2 s; a `mic` example (throwaway `examples/mic.rs`, deleted) prints
   RMS from the built-in mic for 3 s.
2. **`Dictation` actor + RPC.** check, headless on a scratch data dir with
   the model placed: `dictation_prewarm` → `Loading` then
   `DictationModel{Ready}` within a few seconds, RSS +≈0.9 GB, no mic
   indicator; `dictation_start` → `DictationStarted`, the macOS mic
   indicator lights; speak two sentences with a pause → `DictationText{0,
   …, false}` deltas while speaking, `{0, …, true}` ≈ 0.9 s after the pause,
   then utterance 1; `DictationLevel` at ~6/s with 8 values tracking speech;
   `dictation_stop` → `DictationFinished{Done}`, indicator off; a second
   `dictation_start` during the first → "already dictating"; 5 minutes
   idle-open → `Finished{Stopped}`; model dir absent → `Missing` with the
   path; `voice.enabled: false` → "voice is disabled".
3. **TUI.** check, on a real pty with the built-in mic: launch → status
   shows nothing until Ready (log line); type `hello|world` with the
   caret between; Ctrl+D → box turns cyan, header shows `● dictating`
   and the meter moves with your voice; say "this is a test." → grey
   text between `hello` and `world` with single spaces; pause → it turns
   white; say another sentence; Ctrl+D → `hello this is a test. Another
   sentence world` with the caret after `sentence` (before the space that
   precedes `world`); type — insertion continues there. Esc during
   dictation → `hello|world` restored exactly. Ctrl+D with a menu open or an
   approval pending does nothing. `/dictate` toggles the same.
4. **Desktop.** check, `bun run check`, vitest (`splice` cases from §5.1,
   the store's transitions); in the app: after connect the dictation button
   is enabled within seconds (tooltip shows the state before); hover shows
   **Dictation (Ctrl+D)** and **Voice note** on the two buttons; the voice
   note button disappears when text is present, the dictation button stays;
   Ctrl+D with the caret mid-text → the textarea becomes the read-only
   view, the button slot shows the waveform moving with your voice, muted
   text appears and saturates per sentence; Ctrl+D → textarea back, caret
   at the end of the dictated region, `before`/`after` byte-identical
   (compare against a copy); Esc → restored; the voice-note button still
   records and sends `[voice note, N s]` as before; a reconnect (kill and
   restart the daemon) re-prewarms and the button re-enables.
5. **Two frontends.** check: TUI and desktop on one daemon; start
   dictation in one → the other's button reads "dictating elsewhere"
   (`dictation_status.active` set) and Ctrl+D there says so; text lands
   only in the one that started.
6. **Permission.** check: fresh TCC state → first `dictation_start` from
   the `.app` shows the system mic prompt attributed to wth; denial →
   `Finished{Error}` + toast, second attempt shows the settings hint (the
   Computer pane's opener, 27); from a terminal-launched `wth` the prompt
   is attributed to the terminal (record which).
7. **Phone over the relay.** check: link a phone (31) and open a chat →
   the dictation button is shown and enabled once the harness reports
   `Ready`; tap → the webview's mic prompt; speak → muted then saturated
   text lands in the phone's composer exactly as on the desktop, the meter
   moves; tap again → `Finished{Done}`. Kill the phone's connection mid-
   dictation (airplane mode) → the daemon ends it (`Finished{Done}`,
   `dictation_status.active` clears) and a desktop on the same daemon can
   start its own.

## 11. Amendments (same commit series)

- `00-architecture.md` §9: `models/nemotron-streaming-en/`; §10 `voice`
  description gains "live dictation (Nemotron streaming)"; §7 four RPC
  methods.
- `43-voice.md` §9: the microphone seam is split — local frontends use the
  daemon's mic (this doc); remote clients stream PCM with `dictation_audio`.
- `45-voice-notes.md` §7.2: the round `Mic` button is dictation; voice
  notes are the `AudioLines` button; §9 (phase-3 inherits) points here for
  `Stream`.
- `25-desktop.md` §3 parity table: `dictation on the phone | yes`; §7 RPC
  list.
- `meetings/README.md` §3 config: `live_model_dir` becomes "defaults to
  `voice.dictation_model_dir`" (one model directory for both);
  `meetings/02-live.md` §1: `Stream`, `vad::Gate` and the `ort` init order
  already exist from 54 — phase 2 consumes, does not build;
  `meetings/01-capture.md` §4.2: the mic fallback path is `voice::mic`.
- `crates/wth/src/main.rs` `DEFAULT_CONFIG` voice section: the two keys;
  repo `README.md` config bullet.

## 12. Rejected

- **Webview-side STT** (`SpeechRecognition` API, wasm Whisper): nothing
  for the TUI, two engines, WebKit's recognizer is Apple's server model.
- **Dictation as a session input mode** (send each utterance as `Steer`):
  that is voice notes / the voice agent; the requirement is text in the
  box the user can edit before sending.
- **Persisting dictation audio** for a later correction pass: nothing in
  the requirement, and a transient mic path has no privacy surface to
  explain.
- **Auto-stop on silence**: a pause to think would end the dictation;
  the user toggles, `dictation_max_secs` is the safety net.
- **Word-level muted/saturated** (colouring the last N tokens grey): the
  model's tokens are final when emitted; the only honest "not yet certain"
  unit is the VAD's open utterance.

## 13. Status

Implemented 2026-09-20: `voice::{stt::Stream, vad, mic, dictation}` (§4),
the four RPC methods + `RpcPeer::remote()` (§8), TUI (§6), desktop (§7),
§11 amendments. `flexaudio` is pinned at 0.2 (0.3 does not exist on
crates.io). `splice` puts the caret after the last dictated word, before
`trail` (§5.1, §10 check 3).

Checks run: §10.1 `Gate` unit tests, `Stream`/`Vad` compile; mic capture
smoke (960-sample chunks, clean drop). §10.2 headless actor path without a
model: `Prewarm` → `Loading` → `Missing` with the path, parked `Start`
fails with it, `Stop` of an unknown id is `Ok`; relay peer refused. §10.3
`/dictate` on a real pty → `dictation: model missing (…)` line; the rest of
§10.3–10.6 (real model, mic, both frontends, TCC) still needs a machine
with `libonnxruntime` and the Nemotron int8 export placed.

Remote clients added 2026-09-20: `AudioSource::Remote`, `dictation_audio`,
webview capture (branch `remote-dictation`).
