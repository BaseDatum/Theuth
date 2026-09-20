# 43 — Voice, phase 1: read a reply aloud

Status: design v0 · 2026-09-17 · extends `00-architecture.md` §4 (plugin),
§7 (RPC), §9 (data dir), §10 (crate list), §13 (returned-as); `25-desktop.md`
§3 (parity: `voice | not shown`), §7 (RPC additions); `03-streaming-markdown.md`
§2 (the `markdown` crate grows one function). §12 lists the amendments.

Scope of this doc: one button on an assistant message in the desktop app
speaks that message with Kyutai's Pocket TTS, running in the harness. It is
the first slice of a voice agent (streamed, interruptible, later a microphone);
§9 names the seams phase 1 shapes for that and stops there — phase 2 writes
its own doc.

## 0. Decisions in one screen

1. **TTS runs in the harness as a plugin, crate `voice`; the frontend only
   plays PCM.** 00 §13 lists `voice` among the jarvis subsystems that "return
   as plugins when a doc argues the need"; this is that doc. One model
   instance serves every frontend (desktop, a phone through the relay, a
   later TUI), the markdown-to-speech rule lives once, and the future voice
   agent drives the same actor from inside the process. The desktop stays
   the thin RPC client 25 §0.4 made it. Rejected: TTS in the Tauri Rust
   shell with `cpal` (a model per frontend, no headless/phone path, 25 §0.4
   says the shell never proxies); `window.speechSynthesis` in the webview
   (system voices differ per OS, nothing for headless or the voice agent,
   and the user asked for this model).
2. **Inference library: `ptts` 0.2 (crates.io), the `xn` port of Pocket TTS
   by Laurent Mazare (Kyutai/Gradium).** Pure Rust, CPU, `Synth::stream_with`
   yields mono `f32` PCM per Mimi frame (80 ms) on two background threads,
   and **dropping the `SpeechStream` stops both threads** — interruption is
   a `drop`, not a protocol. Quantized weights (q8…q4) on CPU; the
   `pocket-tts` README lists it first among alternative implementations.
   Rejected: the Python package (`torch` ≥ 200 MB, a Python env to manage, an
   HTTP hop — against 00 §2 one process); `sherpa-onnx`/`PocketTTS.cpp`
   (C++ + ONNX Runtime dylib, an export step that lags the checkpoint);
   `pocket-tts` 0.6 (candle) — viable, same iterator shape
   (`generate_stream`), the fallback if `ptts` stalls; Metal/GPU (Kyutai:
   no speedup at batch 1 on Apple Silicon; xn's Metal feature adds a shader
   build to a binary that already builds BoringSSL and V8).
3. **The checkpoint is mirrored on the release bucket, not fetched from the
   Hub.** `kyutai/pocket-tts` is gated (`"gated": "auto"`; anonymous
   `resolve/main/tts_b6369a24.safetensors` → 401, checked 2026-09-17), so
   a Hub download needs every user to accept terms and hold an `HF_TOKEN`.
   The weights are CC-BY-4.0 — redistribution with attribution is allowed.
   `voice` reads `<data_dir>/models/pocket-tts/` (`voice.model_dir` to point
   elsewhere); the downloader that fills it from
   `<bucket>/models/pocket-tts/<rev>/`, sha256-pinned, is the one piece of
   this doc not yet built (§11): the first pass ran against a checkpoint the
   user downloaded themselves. Rejected: an `HF_TOKEN` in the secrets vault
   (friction for a file we may legally host); bundling the model in the
   `.app` (every update ships it).
4. **Markdown becomes speech text in the `markdown` crate:
   `markdown::spoken(md) -> String`.** A ~60-line `pulldown_cmark` event walk
   next to the renderer that already parses the same dialect (03 §2).
   Emphasis and links keep their text; bare URLs become "link"; code blocks
   become the sentence "Code block omitted."; tables read cell by cell;
   headings and list items end as sentences. Rejected: Signal's frozen
   `signal::text::markdown_to_plain` (line-based, keeps fences and URLs
   verbatim — right for a text channel, wrong for a listener); stripping in
   the webview (a second implementation, and the voice agent needs it
   server-side).
5. **Audio crosses the wire as `SpeechAudio` events carrying one Opus
   packet each: 80 ms mono at 32 kbps, base64 in JSON, ≈ 5 KB/s.** Opus
   takes 24 kHz `f32` straight from `ptts`, an 80 ms packet is exactly one
   Mimi frame so nothing is buffered before encoding, and the 6.5 ms
   lookahead is paid once at the start. On loopback the codec changes
   nothing measurable — first audio is the model (~200 ms) plus one frame
   (80 ms) — but through the relay it is 16× fewer bytes to Noise-encrypt
   on a phone and to pay egress for, and it fixes the codec the microphone
   path will need in the other direction (§9). JSON-RPC over a text
   WebSocket stays the only channel (25 §2.1); base64 is how
   `get_attachment` and `browser_screenshot` already move bytes
   (`rpc/src/api.rs:809, 1407`). Rejected: raw i16 PCM (64 KB/s: free on
   loopback, 512 kbps through the relay); binary WebSocket frames (a
   second frame type in jsonrpsee and a new relay frame kind to shave the
   remaining 1.33× of base64 — 00 §7 "not a second protocol"); a WAV
   artifact via `get_attachment` (first audio after the last sample).
6. **One utterance at a time, globally.** `Speak` while speaking stops the
   current utterance first (`SpeechFinished { reason: Stopped }`) and starts
   the new one. A desktop has one speaker; the voice agent's queue (§9) is
   a later variant of the same message, not a new actor.
7. **Stop is local-first.** The Speak button flips and playback halts on
   the click, before `stop_speaking` returns; the harness drops the stream
   within one Mimi frame and the client ignores late `SpeechAudio` for a
   retired id. Nothing waits on a round trip to go quiet.

## 1. Why

A reply the user cannot look at — hands busy, screen off, walking away from
the desk — is a reply not read. Every other surface the harness has (Slack,
Signal, the phone, 24/31) is still text. A voice is also the precondition
for the conversational mode the user wants next: an agent you talk to and
interrupt. Doing the output half first, behind one button, gets the model,
the download, the text rule and the playback path into the tree with a
check on each, before microphones and turn-taking multiply the unknowns.

## 2. Facts relied on (recon 2026-09-17)

### 2.1 Pocket TTS and `ptts`

- Model: ~100 M parameters, 24 kHz mono, 12.5 Hz Mimi frames (80 ms),
  "~200 ms to first audio, ~6× real-time on an M4 Air, 2 CPU cores"
  (`kyutai-labs/pocket-tts` README). English checkpoint at the Hub repo
  root: `tts_b6369a24.safetensors`, `tokenizer.model` (SentencePiece),
  `embeddings/{alba,marius,javert,jean,fantine,cosette,eponine,azelma}.safetensors`;
  newer `languages/<lang>/…` directories exist but ship no `config.json`
  in `ptts`'s format.
- `ptts` (`gradium-ai/xn-ptts`, workspace 0.2.3, MIT/Apache-2.0):
  `Synth::builder(TTSConfig::v202601(0.7), weights).tokenizer_file(..)
  .add_voice(name, path).quant(Quant::Q80).build()?` →
  `Synth::stream_with(&text, &SpeechOptions::default().voice("alba"))?`
  → `SpeechStream: Iterator<Item = Result<Vec<f32>>>`. Text is split on
  sentence boundaries (`split_into_best_sentences`, `max_tokens_per_chunk`),
  so one call handles a whole reply. Two `std::thread`s per stream; the
  flow-LM thread exits when the latent channel closes, the decoder when the
  PCM channel closes, i.e. when the `SpeechStream` is dropped
  (`ptts/src/synth.rs`, `stream_with_rng`, `run_backbone`).
- Weights: safetensors or GGUF (`WEIGHT_CANDIDATES = ["model.safetensors",
  "model.q8.gguf", "tts_b6369a24.safetensors"]`); the `quantize` example
  writes the GGUF. Tokenizer: feature `sp` (SentencePiece, C++ via cmake) or
  `hf` (`tokenizers`, pure Rust) reading `tokenizer.json`;
  `scripts/convert-tokenizer.py` converts. Voices: precomputed embeddings
  via `add_voice_file`; cloning from audio needs the speaker encoder
  (`supports_voice_cloning`), out of scope here.
- `Synth: Send + Sync` [INFERENCE: it spawns threads holding
  `Arc<TTSModel>`; confirmed by the spike, §10 step 1].
- `xn` 0.2.4 on crates.io. crates.io `ptts` 0.2.3 predates the `synth`
  module; the repo's `main` (rev `5d50b52`) has it and depends on the
  published `xn`, so the workspace pins that rev (§11).
- Opus (RFC 6716): input rates 8/12/16/24/48 kHz — 24 kHz needs no
  resample; frame sizes up to 120 ms since libopus 1.2, so one 80 ms Mimi
  frame is one `opus_encode` call and one packet; algorithmic delay 6.5 ms
  (lookahead) once at stream start; a packet decodes standalone. At 32 kbps
  mono an 80 ms packet is ≈ 320 B (≈ 430 B base64), 12.5 packets/s ≈ 5 KB/s
  on the wire against 64 KB/s for base64 i16. Rust: `opus` 0.3 (libopus
  via `audiopus_sys`, cmake — already a prerequisite, 23 §9), `Encoder::
  encode_vec_float(&[f32], max_bytes)`. Webview: `opus-decoder` (npm,
  libopus as wasm, `decodeFrame(Uint8Array) → { channelData, samplesDecoded }`,
  runs on WebKit, WebKitGTK and Chromium alike).

### 2.2 Harness

- Plugin shape, events, commands, config, integration steps:
  `docs/plugins.md` §3–4. Wire-visible event = plugin-owned
  `#[serde(tag = "type")]` enum, forwarded by `rpc/src/bridge.rs`
  `forward!` + one `rt.subscribe` line; every wire type in
  `rpc/src/export.rs`; methods on `trait Api` + `impl ApiServer`
  (`rpc/src/api.rs:87-567, 719+`), `just ts` regenerates
  `bindings/` and `bindings/methods.json`.
- Assistant text: live `AgentEvent::TextDelta { session, turn, text }`
  (`agent/src/lib.rs:831`), stored `HistoryEntry::Assistant { text, turn }`
  (`:748`). No message id; the desktop already holds the full markdown of
  each entry (`store/reduce.ts:127, 268`).
- `markdown` crate: `pulldown-cmark` 0.13 with strikethrough, tables,
  tasklists, footnotes (`markdown/src/lib.rs:84`); `markdown::urls(&str)`
  finds bare URL ranges (`:104`); `Text::committed()` exposes the closed
  blocks of a streaming reply (03 §2).
- Bus: `Guaranteed` + unbounded subscriber mailboxes; payloads cloned once
  per subscriber, so deltas carry `Arc<str>` (00 §3).
- Release bucket: `https://s3.rwx.dev/wth/…` (`update`, 39;
  `scripts/release-s3.sh`); `reqwest` (rustls, stream) is a workspace dep.

### 2.3 Desktop

- Assistant action bar: `MessageAssistant.tsx:116-156` renders
  `<CopyButton>` and `ActionButton`s for Regenerate/Fork once `open` is
  false; `ActionButton({icon, label, onClick, done})` is the 44 px ghost
  button. The raw markdown is the `text` prop.
- Events: one `subscribe_events(null)` per connection, queued and drained
  per `requestAnimationFrame` into `dispatch()` (`App.tsx:80-100`,
  `store/sessions.ts:316`); `HIDDEN_DRAIN_MS = 250` keeps them flowing when
  the window is hidden (`App.tsx:38`). Typed listeners: `on(type, cb)`
  (`sessions.ts:305`). New wire types go in `rpc/events.ts` (`extra.ts`
  until bindings regenerate).
- No audio code anywhere in `crates/desktop`; no `media-src` in the CSP
  (`tauri.conf.json`). The Web Audio API is not CSP-gated and an
  `AudioContext` created inside a click handler satisfies autoplay policy
  [INFERENCE for WKWebView; step 6 checks it].

## 3. Shape

```
 desktop (webview)                         wth --headless
 ┌──────────────────────────┐              ┌──────────────────────────────────────┐
 │ MessageAssistant         │  speak()     │ rpc::Api ──ask──▶ voice::Voice actor │
 │  [🔊] ─────────────────────────────────▶│                   │  markdown::spoken│
 │                          │  SpeechId    │                   │  ensure model    │
 │ lib/speech.ts Player     │◀── events ───│ bridge ◀─publish──┤  spawn_blocking: │
 │  opus-decoder (wasm)     │ SpeechStarted│                   │   ptts stream ──▶│
 │  gapless schedule-ahead  │ SpeechAudio* │                   │   f32→opus→b64   │
 │  stop(): ctx.close()     │ SpeechFinish │                   │  CancellationTok │
 └──────────────────────────┘              └──────────────────────────────────────┘
                                                   <data_dir>/models/pocket-tts/
```

Press → `speak(session, text)` → the actor turns markdown into sentences,
makes sure the model is on disk and in memory, drops whatever is speaking,
and starts a blocking task that pulls PCM from `ptts` and hands each batch
to the actor, which publishes it. The desktop creates an `AudioContext` on
`SpeechStarted`, schedules every `SpeechAudio` back-to-back, and clears the
button when the scheduled audio has drained after `SpeechFinished`.

## 4. Plugin `voice` (`crates/voice`)

### 4.1 Config

```yaml
# voice:                     # read replies aloud (design doc 43): Pocket TTS in-process, audio streamed to the frontend
#   enabled: true            # false: no actor, `speak` answers "not running", the desktop shows no button
#   voice: peter_yearsley    # one of the checkpoint's voices (`voices/<name>.safetensors`); `voice_status` lists them
#   model_dir: ~             # a Pocket TTS checkpoint directory; default <data_dir>/models/pocket-tts
```

Section `voice`, `rt.section::<VoiceConfig>("voice")`, `#[serde(default)]`.
`enabled: false` leaves files under `models/` untouched. No knob for
temperature, seed, quantization or device: `ptts` defaults (0.7, fixed
seed, the mirrored weight format, CPU). `ponytail:` add a key when someone
changes it twice.

### 4.2 Data on disk

`<data_dir>/models/pocket-tts/` — the checkpoint: `tts_b6369a24.safetensors`
(or `model.safetensors` / `model.q8.gguf`), `tokenizer.model` (or
`tokenizer.json`), `voices/<name>.safetensors` — `ptts` speaker embeddings,
not the Hub's `embeddings*/` KV-cache files (§11). With the mirror (§4.6):
`ATTRIBUTION.md`, `REV`; a file whose sha256 does not match the constant in
`voice/src/model.rs` is deleted and re-fetched; a `REV` behind the crate's
constant triggers a re-download; delete the directory to start over.
Nothing else: no cache of synthesized audio.

### 4.3 Commands (`VOICE_ACTOR = "voice"`)

```rust
/// Speak `text` (markdown) in `session`'s name. Stops the current utterance
/// first. Replies at once with the id; audio follows as events.
pub struct Speak { pub session: SessionId, pub text: String }   // ask → Result<SpeechId, String>

/// Stop `id`, or whatever is speaking when `None`. Idempotent.
pub struct Stop { pub id: Option<SpeechId> }                    // ask → Result<(), String>

/// Model state, voices, what is speaking.
pub struct Status;                                              // ask → VoiceStatus

pub struct VoiceStatus { pub model: ModelState, pub voice: String, pub voices: Vec<String>, pub speaking: Option<SpeechId> }
pub enum ModelState { Missing, Loading, Ready, Failed { message: String } }   // Downloading { done, total } arrives with the mirror (§4.6)
pub struct SpeechId(pub uuid::Uuid);   // v7, like runtime::InputId
```

`Speak` never blocks on the model: if the model is `Missing`/`Loading`, the
id is returned and the utterance starts when `Ready` (the first caller
triggers `locate` + `load` in `spawn_blocking`, publishing `VoiceModel`
progress; a failure publishes `SpeechFinished { reason: Error }` for the
waiting id and `VoiceModel { Failed }`, and the next `Speak` retries). The
model stays resident once loaded. `ponytail:` no idle unload; a reload on
every press costs more than the resident weights. Add a timer if RSS
complaints arrive (§11 has the numbers).

### 4.4 Events

```rust
#[serde(tag = "type")]
pub enum VoiceEvent {
    /// Model lifecycle; the desktop shows a toast while `Downloading`/`Loading`.
    VoiceModel { state: ModelState },
    /// The first packet is about to arrive. `chars` is the spoken text's length, for a progress estimate.
    SpeechStarted { id: SpeechId, session: SessionId, sample_rate: u32, chars: u32 },
    /// One Opus packet (RFC 6716), mono, 80 ms = 1920 samples at `sample_rate`, base64. `seq` starts at 0 and has no gaps.
    SpeechAudio { id: SpeechId, session: SessionId, seq: u32, opus: Arc<str> },
    /// Synthesis ended. `Done` means every sample has been published — playback may still have seconds to go.
    SpeechFinished { id: SpeechId, session: SessionId, reason: FinishReason },
}
pub enum FinishReason { Done, Stopped, Error(String) }
```

Published `Guaranteed` on the bus like everything else; only `rpc`'s bridge
subscribes today. `session` is there so `subscribe_events(session)` filters
apply (00 §7); phase 1 utterances are global (§0.6), so the desktop's
`subscribe_events(null)` sees them all.

### 4.5 Synthesis task

```rust
// voice/src/speak.rs — one blocking task per utterance
let spoken = markdown::spoken(&text);                 // empty → SpeechFinished { Done } immediately
let stream = synth.stream_with(&spoken, &SpeechOptions::default().voice(&cfg.voice))?;
let mut enc = opus::Encoder::new(24_000, Channels::Mono, Application::Audio)?;
enc.set_bitrate(Bitrate::Bits(32_000))?;
let mut frame = Vec::with_capacity(FRAME);           // FRAME = 1920 samples = 80 ms = one Mimi frame
tx.blocking_send(Started { id, sample_rate: 24_000, chars })?;
let mut seq = 0;
for chunk in stream {                                 // ≤ 80 ms between checks
    if cancel.is_cancelled() { break }                // drop(stream) at scope end stops both ptts threads
    frame.extend_from_slice(&chunk?);                 // Err is terminal → Finished { Error }
    for f in frame.chunks_exact(FRAME) {              // ptts may hand back several frames at once
        tx.blocking_send(Audio { id, seq, opus: base64(enc.encode_vec_float(f, 400)?) })?; seq += 1;
    }
    let rem = frame.len() % FRAME; frame.copy_within(frame.len() - rem.., 0); frame.truncate(rem);
}
if !frame.is_empty() && !cancel.is_cancelled() {     // tail shorter than a frame: pad with silence
    frame.resize(FRAME, 0.0);
    tx.blocking_send(Audio { id, seq, opus: base64(enc.encode_vec_float(&frame, 400)?) })?;
}
```

The actor owns `current: Option<(SpeechId, CancellationToken)>`, forwards
`Started/Audio/Finished` from the `tokio::sync::mpsc` as `VoiceEvent`s, and
on `Stop`/replacement cancels the token and publishes `Finished { Stopped }`
itself (so the client hears the stop within one bus hop, not one Mimi
frame). `encode_vec_float` takes `ptts`'s `f32` as-is; no i16 step. A
stream that yields nothing (model decided the text is silence) still ends
with `Finished { Done }`.

### 4.6 Model files

`voice/src/model.rs` today: `locate(dir) -> Checkpoint` (weights
`tts_b6369a24.safetensors` | `model.safetensors` | `model.q8.gguf`,
`tokenizer.model` | `tokenizer.json`, every `voices/*.safetensors` keyed by
stem; falls back to `dir/languages/english/` when the root has no weights)
and `load(&Checkpoint) -> Synth` — `SynthBuilder` with the checkpoint's
tokenizer file, `xn::set_num_threads(4)`, every voice registered (a voice
that fails to load is logged and skipped, as `ptts`'s own
`register_voices` does). The mirror half — `const MIRROR`, `FILES` with
sha256s, `ensure(data_dir, progress)` (reqwest GET with `stream`, write to
`<name>.part`, verify, rename; `Downloading { done, total }` at most every
250 ms) — is the next step (§10 step 2, §11): until it lands, a missing
directory is `ModelState::Failed` with the path in the message. Attribution:
the mirrored `ATTRIBUTION.md` (Kyutai, CC-BY-4.0, model card URL) and the
same paragraph in the README's crate list.

## 5. `markdown::spoken`

```rust
/// Markdown as text to be read aloud: what a listener needs, nothing they cannot hear.
pub fn spoken(md: &str) -> String
```

Rules, as one `pulldown_cmark::Parser::new_ext` walk with the crate's
existing option set (`markdown/src/lib.rs:84`):

| event | spoken |
|---|---|
| `Text`, inline `Code` | verbatim (identifiers are worth hearing) |
| `Emphasis`/`Strong`/`Strikethrough` | text only |
| `Link` | link text only; a bare `http(s)://` run in `Text` (via `markdown::urls`) → `link` |
| `Image` | alt text |
| `SoftBreak`, `HardBreak` | one space |
| end of `Paragraph`, `Heading`, `Item`, `TableRow`, `BlockQuote` | terminal punctuation if missing, then `\n` |
| `TableCell` | cell text, cells joined by `, ` |
| fenced or indented `CodeBlock` | the sentence `Code block omitted.` — once per block |
| `Html`, `Rule`, `InlineMath`/`DisplayMath`, `FootnoteReference`, `TaskListMarker`, `MetadataBlock` | nothing |

Whitespace collapsed to single spaces inside a sentence; leading/trailing
trimmed. Empty result ⇒ nothing to say (the desktop toasts "Nothing to
read"). Mermaid and `json-render` fences (30) are code blocks and fall under
the same rule. Reasoning text is never spoken.

Check: one `#[test]` with a fixture covering every row above and the
expected string. This is the one piece of phase 1 where a plausible bug
(a URL read letter by letter, a fence read aloud) fails a test.

## 6. RPC (`crates/rpc`)

Three methods on `trait Api`, one `ask` each (plugins.md §4.2):

| method | params | result |
|---|---|---|
| `speak` | `session: SessionId, text: String` | `SpeechId` |
| `stop_speaking` | `id: Option<SpeechId>` | `()` |
| `voice_status` | — | `VoiceStatus` |

Actor missing (plugin disabled) → `NOT_RUNNING` (−32001), as for every
optional plugin. `VoiceEvent` joins `forward!` and gets its `rt.subscribe`
line in `bridge.rs:79-134`; `VoiceEvent`, `VoiceStatus`, `ModelState`,
`FinishReason`, `SpeechId` join `export.rs`; `rpc`'s `ts` feature forwards
to `voice/ts`. `just ts` adds the three names to `bindings/methods.json`
(the vitest in `rpc/api.test.ts` enforces the mirror in `ui/src/rpc/api.ts`).

## 7. Desktop (`crates/desktop/ui`)

### 7.1 `src/lib/speech.ts` — the player

```ts
import { OpusDecoder } from 'opus-decoder'                    // libopus as wasm; decodeFrame(packet) → Float32Array[]
let ctx: AudioContext | null = null, dec: OpusDecoder | null = null, next = 0, id: string | null = null
export async function start(speech: string, sampleRate: number) {
  stop(); ctx = new AudioContext({ sampleRate }); next = 0; id = speech
  dec = new OpusDecoder({ channels: 1, sampleRate }); await dec.ready   // ~ms; wasm is cached after the first utterance
}
export function push(speech: string, b64: string) {
  if (!ctx || !dec || speech !== id) return                   // late packet for a retired utterance
  const { channelData } = dec.decodeFrame(bytes(b64))         // bytes(): the atob loop files.ts already uses
  const buf = ctx.createBuffer(1, channelData[0].length, ctx.sampleRate)
  buf.copyToChannel(channelData[0], 0)
  const src = ctx.createBufferSource(); src.buffer = buf; src.connect(ctx.destination)
  const at = Math.max(ctx.currentTime + 0.02, next); src.start(at); next = at + buf.duration
}
export function remaining() { return ctx ? Math.max(0, next - ctx.currentTime) : 0 }
export function stop() { ctx?.close(); dec?.free(); ctx = dec = null; id = null }
```

Every packet is exactly one Mimi frame of audio, so `decodeFrame` output is
1920 samples and scheduling is a fixed step. Schedule-ahead on
`AudioBufferSourceNode`s is gapless as long as packets arrive before
`next` — synthesis at 4–6× real time keeps `next` ahead after the first
frame, and the 250 ms hidden-window drain (`App.tsx:38`) only batches what
is already ahead. `ponytail:` no `AudioWorklet` ring buffer;
that comes with the microphone (echo cancellation wants one graph).

### 7.2 State and events

A module-level `speaking: { id, entry } | null` behind
`useSyncExternalStore` (no new Zustand slice for one field). Listeners
registered once at boot next to `useInvalidateOn` (`lib/rpc.ts`):

- `on('SpeechStarted', e)` → `start(e.id, e.sample_rate)`
- `on('SpeechAudio', e)` → `push(e.id, e.opus)`
- `on('SpeechFinished', e)` → `Done`: clear `speaking` after `remaining()`
  seconds (audio outlives synthesis); `Stopped`/`Error`: `stop()`, clear
  now; `Error` toasts the message.
- `on('VoiceModel', e)` → `Downloading`/`Loading`: one `sonner` toast with
  progress, updated in place; `Ready`: dismiss; `Failed`: error toast.

### 7.3 The button

In `MessageAssistant.tsx`'s action bar, before Copy:

```tsx
{voice && <ActionButton icon={speaking?.entry === id ? Square : Volume2}
  label={speaking?.entry === id ? 'Stop' : 'Read aloud'}
  onClick={() => speaking?.entry === id
    ? (speech.stop(), setSpeaking(null), void api.stop_speaking(speaking.id))
    : api.speak(session, text).then(sid => setSpeaking({ id: sid, entry: id }))} />}
```

`voice` is the `voice_status` react-query result, fetched once per
connection; a `NOT_RUNNING` error hides the button everywhere. Pressing
Read aloud on a second message while one speaks just calls `speak` — the
harness stops the first (§0.6) and the `Stopped` event clears its button.
Streaming entries (`open`) have no action bar, so a reply is read once it
is complete; reading while it streams is phase 2.

## 8. Not in phase 1

No TUI command (the TUI has no audio device path; an `afplay` subprocess
would be a second player). No voice picker in Settings (config key only).
No voice cloning, no speed control, no word highlighting
(`pocket-tts-timestamped` exists if wanted). No reading of reasoning, tool
output or peer messages. No Slack/Signal audio. No microphone, no STT, no
barge-in. Each is a line in a later doc, not a stub here.

## 9. Seams phase 1 leaves for the voice agent

Named so phase 1 does not paint over them; none is built now. Phase 2 is
`45-voice-notes.md` (2026-09-17): voice notes in on every channel,
`Synthesize` → MP3 clips per prose block out, the desktop's microphone; it
took the "streaming text has a natural spoken unit" seam (the block boundary
is `ToolCallPending`) and left the queue flag to the client (`speech.ts
enqueue`). The live, interruptible agent is still phase 3.

- **`Speak` is the primitive; a queue is a flag on it.** Phase 2 adds
  `append: bool` (or a `Queue` message) so sentences are spoken in order as
  they arrive, and `Stop` clears the queue. The actor already has the
  single-slot state machine and the cancel token this grows from.
- **Streaming text has a natural spoken unit.** `markdown::Text::committed()`
  (03 §2) yields closed blocks of a streaming reply; `spoken()` on each
  committed block, appended to the queue, is sentence-level streaming TTS
  without a second parser. The voice agent subscribes to `TextDelta` and
  feeds a `markdown::Text` per turn, exactly as Slack's bubble does
  (`slack/src/bubble.rs:112`).
- **Interruption is a `Stop` plus what the agent already has.** Barge-in
  = `Stop` (drop the stream) + `Cancel`/`Steer` on the agent (00 §5). No
  new agent message.
- **Ids and events are already per utterance.** `SpeechId` on every event
  lets a client discard late audio and lets phase 2 report "spoke up to
  sentence n" for the transcript.
- **The wire format is an Opus packet stream.** The microphone seam is
  split (54, 2026-09-20): local frontends — the TUI and the desktop on the
  harness host — use the daemon's own mic (`voice::mic`, 54 §4.5), no audio
  crosses the wire; `AudioIn` remains the remote-client seam, in the other
  direction with the same shape and the same codec (`MediaRecorder` or a
  wasm encoder in the webview, libopus decode in the harness); nothing is
  chosen twice.

## 10. Phases and checks

Each ends with a runnable check, not a test suite.

1. **Spike (throwaway, `crates/voice/examples/say.rs`, deleted after).**
   `cargo add ptts --features hf`; download the Hub checkpoint with a
   personal `HF_TOKEN`; convert the tokenizer with `scripts/convert-tokenizer.py`;
   quantize to q8 with `ptts`'s `quantize` example; synthesize one
   paragraph with f32 and q8; `cargo add opus` and encode the result in
   80 ms packets at 32 kbps. Record: build time added (ptts, libopus),
   load time, RSS, time to first chunk, real-time factor, whether q8 is
   audibly worse on this M3 Max, encoder time per 80 ms frame, and Opus
   bytes/s. Pick the mirrored format. check: `out.wav` for both
   plays; numbers in §11.
2. **Mirror.** Upload `tts_b6369a24.safetensors` (or its q8 GGUF),
   `tokenizer.model`, `voices/*.safetensors` made with `create_voice`,
   `ATTRIBUTION.md` under `models/pocket-tts/<rev>/` with
   `scripts/release-s3.sh`'s credentials; sha256s and `<rev>` into
   `voice/src/model.rs` with the downloader from §4.6. check: `curl -sI`
   each URL is 200; `shasum -a 256` matches the constants. *Not done in
   the first pass — §11.*
3. **`markdown::spoken`.** Function + fixture test. check:
   `cargo test -p markdown spoken`.
4. **`voice` crate.** `VoicePlugin` (`enabled` → actor or `Ok(())` with a
   log), `model.rs`, `speak.rs` with the Opus encoder, events, `main.rs`
   vector line, `DEFAULT_CONFIG` section, `[workspace.dependencies]`
   entries for `voice`, `ptts`, `opus`. check: headless
   (`WTH_RPC__LISTEN=127.0.0.1:7791 wth --headless --data-dir /tmp/wth-v`)
   on a fresh data dir: a ws script calls `speak` →
   `VoiceModel{Downloading…}` → `Ready` → `SpeechStarted` → `SpeechAudio`
   × n → `SpeechFinished{Done}`; the script decodes the packets with
   `opusdec` (or the Rust decoder) to a WAV and it is the paragraph, and
   `seq` is gapless. `stop_speaking` mid-stream → `Finished{Stopped}` and
   no further `SpeechAudio` for that id. Two `speak`s back to back → first
   `Stopped`, second `Done`. `enabled: false` → `speak` is −32001.
5. **RPC.** Methods, `forward!`, `export.rs`, `just ts`. check:
   `bindings/methods.json` gains `speak`, `stop_speaking`, `voice_status`;
   `cargo check -p rpc --features ts`; the desktop vitest passes.
6. **Desktop.** `lib/speech.ts` (decoder + player), listeners, button,
   `voice_status` gate. check in the webview: first press shows the
   download toast, then audio; warm press → audio within about a second;
   second press stops instantly; pressing another message switches; the
   window hidden keeps playing; the button hides with `voice.enabled:
   false`. Measured: bytes per second of `SpeechAudio` on the wire (expect
   ≈ 5 KB/s incl. JSON) and a Linux webview decoding the same stream.
7. **Docs.** §12 amendments, README config bullet and crate line with
   attribution, §11 filled in.

## 11. Status (as-built 2026-09-17)

Implemented in one pass; every check below ran against
`~/.wth/models/pocket-tts/` on an M3 Max. What differs from the design
above is recorded here rather than rewritten into it.

- **Model files: `model_dir` only, no mirror yet.** The mirror (§0.3, §4.6,
  step 2) needs the bucket credentials and a decision on which voices to
  host; today `voice` reads `<data_dir>/models/pocket-tts/` (or
  `voice.model_dir`) as-is and `ModelState` has no `Downloading` variant.
  The checkpoint in use came from the user's own Hub download:
  `tts_b6369a24.safetensors` (f32, 235 MB), `tokenizer.model` (SentencePiece,
  so `ptts` builds with `sp`, not `hf`; cmake was already a prerequisite).
- **Voice files must be `ptts` embeddings.** The Hub's `embeddings*/`
  files are Kyutai KV-cache states (`transformer.layers.N.self_attn/cache`,
  rank 5); `ptts` loads them without complaint and fails at the first
  `stream_with` (`cat: tensor 1 has rank 5`), so `model::locate` reads only
  `voices/*.safetensors`. `voices/peter_yearsley.safetensors` (307 KB, `emb`
  `[1,75,1024]`) was made with `ptts`'s `create_voice` example from the
  public `kyutai/tts-voices/voice-zero/peter_yearsley.wav` (5.94 s; too short
  for `add_voice_from_pcm`'s 10 s floor, fine for `create_voice`). That is the
  file a mirror ships.
- **`ptts` is git-pinned.** crates.io 0.2.3 predates the `synth` module;
  `Cargo.toml` pins `gradium-ai/xn-ptts` rev `5d50b52` (2026-09-16), `xn`
  0.2.4 from crates.io, no git `gemm`. Drop the pin at 0.2.4.
- **`opus` 0.3.1 needs `CMAKE_POLICY_VERSION_MINIMUM=3.5`** under cmake 4
  (`audiopus_sys`'s bundled libopus); `.cargo/config.toml` sets it.
- **Measured (release, 4 xn threads via `xn::set_num_threads(4)` in
  `model::load`; 14 threads was slower and hotter):** load 70 ms (mmap);
  first synthesis pays the page faults. 306 chars → 18.96 s audio in 1.51 s
  (RTF 0.08), TTFA 82–85 ms; peak RSS 650–940 MB f32. Opus: 318 B mean per
  80 ms packet, 3.98 KB/s raw, ≈ 5.1 KB/s on the wire as JSON, encoder
  0.72 ms/frame, wasm decoder 55 µs/frame. Release build added 55 s.
- **Headless check (step 4), fresh data dir:** `voice_status` cold →
  `missing`; `speak` → `VoiceModel{loading}` → `{ready}` → `SpeechStarted`
  at 467 ms (load included) → 108 `SpeechAudio`, `seq` gapless →
  `SpeechFinished{done}` at 1.4 s for 8.64 s of audio; the packets decoded
  through `opus-decoder` (the webview's decoder) to a WAV that reads the
  fixture as `spoken()` renders it — heard by the user. `stop_speaking`
  after `seq` 3 → `{stopped}`, zero late packets. Two `speak`s back to back
  → `stopped` then `done`. `WTH_VOICE__ENABLED=false` → `voice_status` is
  −32001 `voice is disabled`.
- **Gates:** `cargo check`/`clippy -D warnings`/`test` for `voice`, `rpc`,
  `wth`, `markdown`; `just ts` (bindings + `methods.json` gained the three
  methods); `bun run check` and 112/112 vitest in `desktop/ui`.
- **Desktop (step 6):** button renders first in the action bar; the click →
  audio path is being exercised by the user in browser dev mode
  (`VITE_WTH_URL`) against the headless daemon. Not exercised: the Tauri
  `.app`, a hidden window, Linux/WebKitGTK, a phone through the relay.
- **Deviations kept:** no "Nothing to read" toast (an empty `spoken()`
  yields `SpeechFinished{done}` at once and the button clears); the
  synthesis task reports to the actor with kameo `tell(..).blocking_send()`
  instead of an mpsc; a `Speak` while the model loads replaces the waiting
  utterance (its `Stopped` is published immediately) rather than queueing.
- **Leftovers:** none in-tree (the spike example and its dev-deps were
  removed).

## 12. Amendments to 00 and 25 (same commit series)

- 00 §9: data dir gains `models/pocket-tts/` (mirrored Pocket TTS
  checkpoint; delete to re-download — 43 §4.2).
- 00 §10: `voice/  plugin: read replies aloud — Pocket TTS via ptts, model
  mirror, speak/stop, SpeechAudio events (43)`; dependency chain
  `{signal, slack, nodes, schedule, artifact, update, voice} ← rpc`
  (`voice` depends on `runtime`, `markdown`; `rpc` names its types).
  Workspace table: `ptts | 0.2 | Pocket TTS inference, CPU, streaming
  (43)`; `opus | 0.3 | libopus encoder for `SpeechAudio` (43)`. Desktop
  `package.json`: `opus-decoder` (libopus as wasm).
- 00 §13 returned-as table: `| voice | \`voice\` crate: Pocket TTS in
  process, audio streamed to frontends | plugin | 43 (jarvis's
  system-voice fallback, per-message caching and TUI playback not ported) |`.
- 25 §3: the `voice` cell of the "not shown" row → `read aloud: yes (43),
  one button per assistant message; dictation: no`.
- 25 §7: three RPC methods (§6).
- 03 §2: the crate gains `spoken()`; the doc's "TUI renderer" framing
  becomes "renderer plus one text exporter".

## 13. Rejected

- **`window.speechSynthesis`.** Native and free, but a different voice on
  every OS, nothing for headless or a phone through the relay, and not the
  voice a voice agent can build on. The user named the model.
- **Python `pocket-tts serve` as a subprocess.** `torch` and a Python
  environment per install, an HTTP hop, a second process to babysit;
  00 §2 is one process.
- **`sherpa-onnx` / `PocketTTS.cpp`.** ONNX Runtime dylib and a C++ build
  for a model whose Rust port exists; the export lags each checkpoint.
- **`pocket-tts` 0.6 (candle).** Same shape, larger dependency surface
  (`candle-*`, `hf-hub`, `tokenizers`, `rubato`, `axum` in the workspace);
  `ptts` is maintained next to the model by its authors' orbit. The
  fallback if `ptts` stalls: `voice/src/model.rs` and `speak.rs` are the
  only files that know which.
- **Metal.** Kyutai measured no speedup at batch 1 on Apple Silicon; the
  feature adds a shader build and a second code path.
- **TTS in the Tauri shell (`cpal`/`rodio`).** 25 §0.4: the shell never
  proxies; one model per frontend; nothing for the phone.
- **WAV artifact + `get_attachment`.** Not streaming.
- **Binary WebSocket frames.** After Opus the base64 tax is ≈ 1.3 KB/s. Removing
  it costs a second frame type in jsonrpsee (text JSON-RPC only), a new
  relay frame kind in 31 §4.2 and a dispatch path beside `raw_json_request`
  — 00 §7's "not a second protocol" for a 1.33× that the codec's 16× makes
  irrelevant.
- **Raw PCM (i16) on the wire.** 64 KB/s is free on loopback and a 512 kbps
  stream through the relay, Noise-encrypted at both ends, on a phone.
- **WebCodecs `AudioDecoder`.** The native rung, but Tauri on Linux is
  WebKitGTK and its WebCodecs audio support is unconfirmed [INFERENCE];
  one decoder that runs on all three webviews beats two paths.
- **`HF_TOKEN` in the vault instead of a mirror.** Every user accepting
  Hub terms for a CC-BY file we may host.
- **Stripping markdown in the webview.** A second rule set; the voice
  agent needs the server-side one anyway.
- **A `Speaker` trait over `ptts`/candle.** One implementation.

## 14. Open

| # | question | default |
|---|---|---|
| 1 | q8 GGUF or f32 safetensors on the mirror | f32 is what ships today (235 MB, 650–940 MB peak RSS); quantize with `ptts`'s `quantize` example when the mirror is built |
| 2 | Default voice | `peter_yearsley` (the one voice in the checkpoint at hand); `alba` when the mirror ships Kyutai's set |
| 3 | Speak a reply while it still streams | no; phase 2 (§9) |
| 4 | Read reasoning when the user shows it | no |
| 5 | Voices mirrored | four; all eight if the total stays under a few MB |
| 6 | Opus bitrate | 32 kbps, `Application::Audio`; drop to 24 kbps VoIP only if a relay measurement asks |
| 7 | WebCodecs `AudioDecoder` instead of the wasm decoder | when it is confirmed on WebKitGTK too; one file changes |
