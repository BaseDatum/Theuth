# Anarlog (ex-Hyprnote) — ENGINE half recon (fastrepl/anarlog @ ef69625, MIT)

Read-only source recon, 2026-10-10, by recon-anarlog-engine. Techniques only — no code copied; at most a few words of identifiers quoted. Cites are repo-relative `path#L<a>-L<b>` against the shallow clone at HEAD ef69625. Nothing was built or run.

## Executive summary

1. Meeting auto-detect = a non-ignored app holds the mic: CoreAudio per-process `is_running_input` + `DeviceIsRunningSomewhere` listener (1 s poll fallback), 500 ms debounce, deny-list categories, 15 s sustain, 10 min cooldown — crates/detect/src/list/macos.rs#L84-L106, plugins/detect/src/policy.rs#L19-L84, plugins/detect/src/mic_usage_tracker.rs#L8-L9.
2. Accessibility introspection of Zoom/Meet/Teams/Slack-huddle/Discord/Webex windows: platform, Zoom mute, read AND send meeting chat — crates/detect/src/meeting_ax/types.rs#L6-L21, crates/detect/src/meeting_ax.rs#L669-L670, crates/detect/src/meeting_ax.rs#L375-L376.
3. Neural AEC: two-stage ONNX (mask on FFT magnitudes, then time domain; 512/128 blocks; LSTM states), weights embedded, on all OSes — crates/aec/src/onnx/model.rs#L1-L24, crates/aec/src/onnx/mod.rs#L251-L310.
4. AEC is preceded by loopback→mic lag alignment (FFT cross-correlation with acquire/hold/lose hysteresis, ≤ 600 ms) and followed by a linear residual subtractor with a double-talk guard — crates/audio-sync/src/probe.rs#L8-L70, crates/audio-actual/src/capture/stream.rs#L685-L747.
5. Headphones ⇒ AEC off and mic "isolated" ⇒ every mic voice sample auto-enrolls as the owner — crates/listener-core/src/actors/source/stream.rs#L48-L76, plugins/transcription/src/voiceprint.rs#L411-L418.
6. Capture resilience: per-channel 5 s frame timeout, 1/2/4 s retries, silence gap fill; append-mode WAV with 1 s flush and repair-on-reopen — crates/audio-actual/src/capture/recovery.rs#L27-L33, crates/listener-core/src/actors/recorder/disk.rs#L16-L60.
7. Every STT engine behind one Deepgram-style protocol; "local" is a base URL (in-app whisper.cpp on 127.0.0.1:0, Argmax sidecar, `soniqo://local`, `apple-speech://local`) under a restart supervisor — plugins/local-stt/src/server/internal.rs#L52-L75, plugins/local-stt/src/server/supervisor.rs#L19-L28.
8. Live model → batch twin + diarizer after the meeting (Parakeet EOU → Parakeet TDT v3 + pyannote community-1) — crates/transcribe-soniqo/src/model.rs#L100-L163.
9. Speaker naming across meetings: calendar participants as candidates, cosine match ≥ 0.62 with margin ≥ 0.08 (mutual), completion by elimination, same capture domain/model only — crates/voiceprint/src/matching.rs#L3-L163, plugins/transcription/src/voiceprint.rs#L929-L943.
10. Participant-count-bounded diarization per channel, in-person detection (system channel ≤ 3 s speech), and on-device consolidation of cloud speaker labels — crates/listener2-core/src/batch/diarize.rs#L74-L134, crates/listener2-core/src/batch/consolidate.rs#L1-L27.
11. Voiceprint vectors stored as keyring secrets (secret written before the DB row), 45-day candidate TTL — plugins/transcription/src/voiceprint.rs#L15-L15, plugins/transcription/src/voiceprint.rs#L340-L366.
12. Diarization DER harness (AMI + VoxConverse, collar 0.25 s) driven by an autoresearch ratchet program (keep if ΔDER > 0.1 and RTF < 0.15) — crates/pyannote-local/eval/PROGRAM.md#L1-L44.

## A1 Capture and meeting detection

### How a meeting starts — detection is "a non-ignored app opened the mic"
- The detector emits only four event kinds: `MicStarted(apps)`, `MicStopped(apps)`, and on macOS `ZoomMuteStateChanged` and `SleepStateChanged` — crates/detect/src/lib.rs#L55-L66. There is no calendar trigger and no window-title trigger in this crate; `Detector::start` wires the mic detector, a Zoom watcher and a sleep detector only — crates/detect/src/lib.rs#L96-L120.
- macOS signal: a CoreAudio property listener on the default input's `DeviceIsRunningSomewhere`, plus a system-object listener for device changes; if registering either listener fails, the detector switches itself to a 1 s polling fallback — crates/detect/src/mic/macos/mod.rs#L15-L24 (`POLL_INTERVAL` 1 s at L21), crates/detect/src/mic/macos/mod.rs#L151-L195.
- Which app: it enumerates CoreAudio process objects and keeps those whose `is_running_input` is true, resolving pid → bundle via `NSRunningApplication` (fallback: sysinfo) — crates/detect/src/list/macos.rs#L84-L106, crates/detect/src/list/macos.rs#L184-L212. Apple's call daemons are relabelled to the visible call app (FaceTime/Phone) — crates/detect/src/list/macos.rs#L108-L151.
- Per-app diffing turns successive snapshots into Started/Stopped lists — crates/detect/src/mic/macos/app.rs#L8-L28, crates/detect/src/mic/macos/app.rs#L30-L67; state changes are debounced 500 ms — crates/detect/src/mic/macos/state.rs#L25-L35.
- Windows and Linux have their own per-app reconcile loops (Windows: a worker thread that seeds and reconciles app lists; Linux: PulseAudio context) — crates/detect/src/mic/windows.rs#L31-L71, crates/detect/src/mic/linux.rs#L1-L36.
- Policy (plugin side): apps are grouped into categories that are ignored by default — Anarlog itself (old `com.hyprnote.*` ids), Dictation (Wispr Flow, superwhisper, VoiceInk, MacWhisper, Voice Memos…), IDE (Warp, Windsurf, VS Code, Cursor's todesktop id), ScreenRecording (Cap, Screen Studio, Loom, OBS, CleanShot, Kap…), AIAssistant, Other — plugins/detect/src/policy.rs#L19-L84; all categories are ignored by default, the user can ignore or include single bundle ids, and user overrides win — plugins/detect/src/policy.rs#L125-L140, plugins/detect/src/policy.rs#L196-L204. Do-not-disturb and "all apps filtered" are the skip reasons — plugins/detect/src/policy.rs#L13-L17. So: unknown app + mic ⇒ "meeting?" notification (a deny-list, not an allow-list of meeting apps).
- Prompt timing (plugin side): an app must hold the mic for **15 s** before the "meeting detected" notification fires; a generation-numbered timer per app prevents a stale timer from claiming a newer one, and a successful claim puts the app in a **10-minute cooldown** — plugins/detect/src/mic_usage_tracker.rs#L8-L9, plugins/detect/src/mic_usage_tracker.rs#L36-L58, plugins/detect/src/mic_usage_tracker.rs#L60-L98.
- Net detection pipeline (my summary of the cites above):
  1. CoreAudio says the input device is running somewhere (listener, or 1 s poll fallback).
  2. Snapshot the processes with running input → bundle ids → diff → `MicStarted([apps])`.
  3. 500 ms debounce in the detector; category/user policy filter in the plugin.
  4. 15 s sustained use → notification; 10 min per-app cooldown afterwards.
- Zoom extra: when Zoom holds the mic, a 1 s poll reads the Zoom window's Accessibility tree and maps the button title "Mute audio"/"Unmute audio" to the mute state — crates/detect/src/zoom.rs#L9-L10, crates/detect/src/zoom.rs#L72-L100.

### Accessibility meeting introspection (macOS, partial Windows/Linux)
- A separate module walks the AX tree (≤ 32 deep, ≤ 4,000 nodes) of native and browser meeting windows — crates/detect/src/meeting_ax.rs#L103-L108; it knows Zoom, Google Meet, Teams, Slack (huddles), Discord, Webex, and native vs web surfaces — crates/detect/src/meeting_ax/types.rs#L6-L21. Browser windows qualify by provider URL/title signal — crates/detect/src/meeting_ax.rs#L1548-L1549.
- It can CAPTURE in-meeting chat messages (incoming/outgoing) and SEND a chat message into the meeting (≤ 2,000 chars, input restored if it owned it) — crates/detect/src/meeting_ax.rs#L669-L670, crates/detect/src/meeting_ax.rs#L375-L376, crates/detect/src/meeting_ax.rs#L362-L363, crates/detect/src/meeting_ax.rs#L1280-L1281, crates/detect/src/meeting_ax/types.rs#L26-L29. Per-OS backends exist (crates/detect/src/meeting_ax/linux.rs, crates/detect/src/meeting_ax/windows.rs) with a fixture test suite (crates/detect/src/meeting_ax/tests/slack_huddle.rs, …/message_parsing.rs, …/chat_scope.rs).

### Sleep and screen signals
- Sleep: `NSWorkspaceWillSleepNotification` / `DidWake` observers become `SleepStateChanged` events — crates/detect/src/sleep.rs#L41-L66, crates/detect/src/lib.rs#L62-L65.
- **Slide capture** (screen, not audio): the detect crate keeps a 64×36 colour signature of the shared window; a frame is "stable" when its mean per-channel difference to the previous is ≤ 0.02 and is "new content" when it differs from every kept frame by ≥ 0.06, or when ≥ 4 % of cells changed by > 32 levels (same-template slides that only change text); only the inner area counts so toolbars/participant strips do not trigger; up to 200 remembered frames stop re-saving a slide you go back to; saved as JPEG q80, ≤ 1,920 px wide — crates/detect/src/screen.rs#L7-L19, crates/detect/src/screen.rs#L25-L47.

### What is captured, per OS
- macOS system audio: a Core Audio **process tap** — a mono global tap that excludes no processes — wrapped in a private aggregate device with tap auto-start; the tap's own ASBD gives the sample rate — crates/audio-actual/src/speaker/macos.rs#L53-L86, crates/audio-actual/src/speaker/macos.rs#L136-L181. (Not ScreenCaptureKit; one mixed system track, not per-app.)
- Windows system audio: WASAPI loopback on the default render endpoint, 44.1 kHz default, endpoint polled every 2 s and a switch needs 2 confirmations — crates/audio-actual/src/speaker/windows.rs#L26-L30, crates/audio-actual/src/speaker/windows.rs#L198-L217.
- Linux system audio: PipeWire with a PulseAudio path, 48 kHz default — crates/audio-actual/src/speaker/linux.rs#L10-L26.
- Mic: cpal-style input with resampled fixed-size chunks; mic and speaker are joined into stereo pairs (`pop_available_pair`) — crates/audio-actual/src/capture/stream.rs#L62-L86, crates/audio-actual/src/capture/stream.rs#L302-L306.

### Echo cancellation — neural AEC + linear residual, sitting on the mic before STT
- Model: two ONNX graphs run in sequence (stage 1 on FFT magnitudes of mic + loopback producing a spectral mask, stage 2 on the masked time signal), with LSTM states of size 128/256/512 selectable by Cargo feature; default `onnx` + `128` — crates/aec/src/onnx/model.rs#L1-L24, crates/aec/Cargo.toml#L7-L11. Block 512 samples, shift 128 ("model already trained with these numbers") — crates/aec/src/onnx/model.rs#L22-L24. This is the DTLN-aec shape (my identification from the two-model mask + LSTM-state layout; the repo does not name it). Weights are embedded with `include_bytes!` (2.0 MB + 5.3 MB for the 128 variant; directory listing of crates/aec/data/models).
- Streaming state: overlap-add circular buffers (keep block−shift samples, append shift) — crates/aec/src/lib.rs#L11-L54; `reset()` zeroes LSTM states and buffers — crates/aec/src/onnx/mod.rs#L98-L106; the per-block loop FFTs mic and loopback, runs stage 1, applies the mask, IFFTs, runs stage 2 — crates/aec/src/onnx/mod.rs#L251-L310.
- Before the neural stage the loopback is **time-aligned** to the mic: an FFT cross-correlation lag probe (max lag 600 ms, min overlap 400 ms) delays whichever signal leads; when the stable lag changes the AEC state is reset — crates/audio-actual/src/capture/stream.rs#L29-L31, crates/audio-actual/src/capture/stream.rs#L373-L512, crates/audio-sync/src/estimator.rs#L24-L47. Tests lock a "negative MacBook-scale lag" — crates/audio-actual/src/capture/stream.rs#L1025-L1026.
- The lag probe itself (crates/audio-sync): FFT cross-correlation over 4,096-sample windows every 16,000 samples (1 s at 16 kHz), min RMS 0.003; it *acquires* a lock when 3 of 4 windows agree within 24 samples at peak ratio ≥ 10 and distinctiveness ≥ 1.15, *holds* at 8 / 1.05, and drops the lock after 3 rejections; outliers beyond 48 samples are ignored; the stable lag is a 5-window history — crates/audio-sync/src/probe.rs#L8-L46, crates/audio-sync/src/probe.rs#L52-L70, crates/audio-sync/src/estimator.rs#L24-L47. The AEC caller widens max lag to 600 ms — crates/audio-actual/src/capture/stream.rs#L421-L431. A drift tracker reports lag trend in ppm — crates/audio-sync/src/drift.rs#L9-L10, crates/audio-sync/src/drift.rs#L53-L53.
- After the neural stage a **linear residual-echo subtractor** removes what still correlates with the loopback: skip if either RMS < 1e-4 or correlation < 0.12; gain = cross/speaker energy clamped ±1.25; if the residual ratio > 0.08 (double talk) a segmented trimmed gain (8 segments, ≥ 3 valid) is used and smoothed with α = 0.12 — crates/audio-actual/src/capture/stream.rs#L32-L39, crates/audio-actual/src/capture/stream.rs#L685-L747, crates/audio-actual/src/capture/stream.rs#L749-L775.
- Frame path, in order (my summary): mic chunk + speaker chunk → stereo joiner pairs them → lag aligner delays the leading side → DTLN-style AEC on (mic, aligned loopback) → linear residual subtraction → frame = {raw mic, raw speaker, aec_mic?} → VAD mask → STT socket and disk recorder.
- Placement: per paired frame, AEC runs only when both channels are live; the frame carries raw mic, raw speaker AND an optional `aec_mic`; downstream prefers the AEC mic — crates/audio-actual/src/capture/stream.rs#L307-L330, crates/listener-core/src/actors/source/pipeline.rs#L1484-L1516. An AEC failure logs and falls back to raw mic — crates/audio-actual/src/capture/stream.rs#L665-L683.
- Auto-off: if every playing output is a headphone, AEC is disabled ("pure cost… can degrade near-end speech") and the mic is flagged *isolated* (= only the local user) unless the mic was swapped away from the headset; env `NO_AEC=1` forces off — crates/listener-core/src/actors/source/stream.rs#L40-L76. The flag can flip mid-session via an output-change hook — crates/audio-actual/src/capture/stream.rs#L175-L177, crates/audio-actual/src/capture/stream.rs#L244-L249.
- Measured effect: no number in the repo. There is a criterion bench (crates/aec/benches/aec_bench.rs) and an Audacity round-trip example (crates/aec/examples/audacity.rs) for listening tests; Not found: any ERLE/PESQ figure (searched crates/aec, crates/audio-actual for "erle", "pesq", "db").

### Device changes, permissions, crash safety, retention
- Device monitor events: default input changed, default output changed (with a headphone verdict), device list changed, volume/mute updates; debounced 1,000 ms by key — crates/device-monitor/src/lib.rs#L15-L30, crates/device-monitor/src/lib.rs#L86-L88, crates/device-monitor/src/debounce.rs#L33-L60.
- A user-ordered device priority list (remember/hide/move-to-top, last seen) picks the preferred device — plugins/audio-priority/src/priority.rs#L110-L211. Bluetooth-HFP avoidance: the capture layer can swap the mic away from a headset mic so the headset stays in A2DP (comment at crates/listener-core/src/actors/source/stream.rs#L78-L80; the `mic_swapped` flag feeds the isolation rule at L42-L45).
- Per-channel recovery: a channel with no frame for 5 s is failed and reopened with backoff 1 s / 2 s / 4 s; the retry budget resets after 30 s healthy; a manual retry is honoured — crates/audio-actual/src/capture/recovery.rs#L27-L33, crates/audio-actual/src/capture/recovery.rs#L49-L50. While a channel is down the healthy one keeps flowing and gaps are filled with silence so the two timelines stay aligned (test: "dual outage keeps sixty seconds of silence before recovered audio") — crates/audio-actual/src/capture/stream.rs#L346-L347, crates/audio-actual/src/capture/stream.rs#L805-L849.
- TCC: preflight and reset of `kTCCServiceAudioCapture` (system audio tap), `kTCCServiceScreenCapture` and `kTCCServiceMicrophone` by `dlopen`ing the private TCC framework (`TCCAccessPreflight`, `TCCAccessReset`) — crates/tcc/swift-lib/src/lib.swift#L7-L83. Onboarding UI with a drag-source guide cursor lives in plugins/permissions/src/assistant/macos/ (views/drag_source.rs, views/guide_cursor.rs).
- Recording on disk: a session directory gets `audio.wav` (stereo) plus `audio_mic.wav`/`audio_spk.wav` mono side files written with hound in *append* mode, flushed every 1,000 ms; an invalid WAV left by a crash is preserved and repaired on reopen; the final artefact is `audio.mp3` (legacy `audio.ogg` decoded) — crates/listener-core/src/actors/recorder/disk.rs#L16-L20, crates/listener-core/src/actors/recorder/disk.rs#L34-L60, crates/listener-core/src/actors/recorder/disk.rs#L60-L82.
- Live transcript persistence is journaled in 250 ms batches with retry 0.5 → 5 s and a bounded backlog (10k words / 1 M chars) — plugins/transcription/src/live_journal.rs#L20-L28. Capture gaps are recorded per session (≤ 128 gaps, 32 sessions) — plugins/transcription/src/capture_gaps.rs#L8-L9.
- Retention: a 60 s sweeper applies `audio_retention` (default `forever`; `save_recordings=false` honoured) and a 6-hourly voiceprint cleanup — plugins/transcription/src/audio_retention.rs#L12-L19, plugins/transcription/src/audio_retention.rs#L69-L70.


## A2 Transcription

### Engines and the catalogue
- Selectable local STT models (one list is the source of truth): Soniqo Parakeet Streaming, Soniqo Parakeet Batch, Apple Speech (default), Argmax Parakeet V2, Argmax Parakeet V3, Argmax Whisper Large V3 — crates/local-stt-core/src/lib.rs#L3-L10. Engine families: `Soniqo`, `AppleSpeech`, `Whispercpp`, `Argmax` — crates/local-stt-core/src/lib.rs#L15-L20. whisper.cpp models still exist in code but are not in `SUPPORTED_MODELS` (legacy).
- Each entry carries display name, size, a **recommended RAM** figure (8 GiB for Parakeet streaming / Apple / Whisper / Argmax Parakeet, 16 GiB for Parakeet batch and Argmax Whisper L-v3, 32 GiB for Qwen3-ASR 1.7B) and a `supports_realtime` flag — crates/local-stt-core/src/lib.rs#L22-L48, crates/local-stt-core/src/lib.rs#L50-L91. A test enforces unique keys and non-empty metadata — crates/local-stt-core/src/lib.rs#L110-L129.
- **Soniqo** = the Swift package `soniqo/speech-swift` 0.0.27 (products AudioCommon, ParakeetASR, ParakeetStreamingASR, OmnilingualASR; `SpeechVAD` imported) bridged with swift-rs — crates/transcribe-soniqo/swift-lib/Package.swift#L15-L27, crates/transcribe-soniqo/swift-lib/src/lib.swift#L1-L8. Models from Hugging Face `aufklarer/*`: Parakeet-EOU-120M CoreML INT8 (120 MB, live), Parakeet-TDT-v3 CoreML INT8 30 s (632 MB, batch), Omnilingual-ASR-CTC-300M, Qwen3-ASR 0.6B MLX-4bit / 1.7B MLX-8bit — crates/transcribe-soniqo/src/model.rs#L100-L140. Only the two Parakeets are selectable; Qwen3 is gated off by `requires_macos_15` making it unavailable everywhere — crates/transcribe-soniqo/src/model.rs#L80-L80, crates/transcribe-soniqo/src/model.rs#L146-L152. Apple Silicon only; Parakeet = 25 European languages — crates/transcribe-soniqo/src/model.rs#L120-L128, crates/transcribe-soniqo/src/model.rs#L165-L171.
- **Argmax (`am`)**: Argmax's Pro SDK models (`argmaxinc/parakeetkit-pro`, `argmaxinc/whisperkit-pro`; Parakeet v2 476 MB, v3 494 MB, Whisper L-v3 626 MB) mirrored as tarballs on `models.anarlog.so` with a CRC-style u32 checksum — crates/am/src/model.rs#L27-L41, crates/am/src/model.rs#L69-L112. They run in an external sidecar binary spawned with `--port`, health via `/status`, `/waitForReady`, `/init` — plugins/local-stt/src/server/external.rs#L114-L130, crates/am/src/client.rs#L28-L71.
- **Apple SpeechAnalyzer** (macOS 26): a `SpeechTranscriber` with `.volatileResults` for live partials; locale matched with `supportedLocale(equivalentTo:)`; assets reserved through `AssetInventory`, evicting the oldest reserved locale when at `maximumReservedLocales`, install progress polled with a timeout — crates/transcribe-speechanalyzer/swift-lib/src/lib.swift#L118-L148, crates/transcribe-speechanalyzer/swift-lib/src/lib.swift#L174-L246; exposed as `apple-speech://local` — crates/transcribe-speechanalyzer/src/lib.rs#L7-L10.
- **whisper.cpp** (in-process): q8_0 GGML models tiny → large-v3-turbo mirrored at `models.anarlog.so/v0/ggerganov/whisper.cpp/…` — crates/whisper-local-model/src/lib.rs#L34-L62; GPU on, flash-attention OFF ("crash on macos") — crates/whisper-local/src/model/actual.rs#L31-L37.

Catalogue at a glance (sizes from code):
| Key | Engine | Size | Live | Notes / cite |
|---|---|---|---|---|
| soniqo-parakeet-streaming | speech-swift CoreML INT8 (Parakeet EOU 120M) | 120 MB | yes | crates/transcribe-soniqo/src/model.rs#L100-L104 |
| soniqo-parakeet-batch | Parakeet TDT v3 CoreML INT8 30 s + community-1 diarizer | 632 MB | no | crates/transcribe-soniqo/src/model.rs#L132-L136 |
| apple-speech | SpeechAnalyzer (macOS 26) | OS assets | yes | crates/transcribe-speechanalyzer/src/lib.rs#L7-L10 |
| am-parakeet-v2 / v3 | Argmax ParakeetKit Pro | 476 / 494 MB | no | crates/am/src/model.rs#L69-L74 |
| am-whisper-large-v3 | Argmax WhisperKit Pro | 626 MB | no | crates/am/src/model.rs#L69-L74 |
| whisper q8_0 tiny…large-v3-turbo | whisper.cpp | per file | no (legacy) | crates/whisper-local-model/src/lib.rs#L34-L62 |
| soniqo-omnilingual / qwen3 | known, not selectable | 300 MB – 1.7 GB | no | crates/transcribe-soniqo/src/model.rs#L66-L80 |

### One protocol, local = a base URL
- Every engine — 20+ cloud adapters (Deepgram, Soniox, AssemblyAI, Gladia, OpenAI, ElevenLabs, Mistral, Fireworks, …) and the local ones — sits behind the same owhisper listen interface; the listener picks an adapter by `AdapterKind` — crates/listener-core/src/actors/listener/adapters.rs#L171-L217.
- Local engines are addressed as URLs: whisper.cpp is an axum `/v1/listen` route bound to `127.0.0.1:0` inside the app — plugins/local-stt/src/server/internal.rs#L52-L75, crates/transcribe-whisper-local/src/service/streaming.rs#L37-L39; Argmax is `http://localhost:<port>/v1` — plugins/local-stt/src/server/external.rs#L128-L130; Soniqo and Apple are pseudo-schemes `soniqo://local` / `apple-speech://local` intercepted before any socket — crates/transcribe-soniqo/src/model.rs#L5-L11, crates/listener-core/src/actors/listener/adapters.rs#L110-L133, crates/listener-core/src/actors/listener/adapters.rs#L238-L255.
- Both local servers live under a ractor supervisor: one child, ≤ 100 restarts per 3 min window, reset after 30 s healthy, transient restart after 0.5 s / 1 s — plugins/local-stt/src/server/supervisor.rs#L19-L28, plugins/local-stt/src/server/supervisor.rs#L79-L103.
- Batch-side "is this local?" is decided from the base URL being loopback — crates/listener2-core/src/batch/diarize.rs#L46-L61, crates/transcribe-soniqo/src/model.rs#L11-L36.

- Wire shape (Deepgram-compatible): words carry start/end/confidence/optional speaker/punctuated form/language; alternatives carry transcript + languages; responses carry a channel and metadata with model info — crates/owhisper-interface/src/stream.rs#L9-L60.
- Listener failures are typed and non-fatal: authentication_failed, upstream_unavailable, connection_timeout, provider_configuration, stream_error put the session into a degraded state while capture/recording continue — crates/listener-core/src/lib.rs#L48-L59, crates/listener-core/src/actors/listener/adapters.rs#L44-L66.

### Live vs batch
- Live: the source pipeline runs an energy VAD **mask** (earshot detector, 6-frame hangover, amplitude floor 0.0005, starts in speech) that zeroes non-speech frames before they reach the recogniser — crates/vad-masking/src/streaming.rs#L4-L19, crates/vad-masking/src/masking.rs#L21-L35, crates/listener-core/src/actors/source/pipeline.rs#L61-L90.
- Local whisper live path chunks speech with a redemption time (default 600 ms) — crates/audio-chunking/src/speech.rs#L16-L22, crates/transcribe-whisper-local/src/service/streaming.rs#L251-L257, crates/transcribe-whisper-local/src/service/streaming.rs#L471-L488; chunk policy drops detections < 200 ms and merges short chunks with gaps ≤ 250 ms — crates/audio-chunking/src/vad/chunk_policy.rs#L14-L16. At most 2 websocket channels (mic, speaker) — crates/transcribe-whisper-local/src/service/streaming.rs#L39-L39.
- Silero VAD v6.2 ONNX is embedded too (512-sample chunks, 64 context, 128 state) — crates/vad/src/silero_onnx/v6.rs#L4-L24.
- After the meeting, `listener2` runs a **batch** pass ("progressive" with a 30 s stream timeout and 10 s segment keep-alive) whose result replaces the live words; a streaming model is swapped for its batch twin (Parakeet EOU → Parakeet TDT v3 + diarizer) — crates/listener2-core/src/batch/progressive/actor.rs#L19-L21, crates/listener2-core/src/batch/progressive/segmented.rs#L14-L14, crates/transcribe-soniqo/src/model.rs#L158-L163.
- Rendering bounds for the live transcript: ≤ 4,096 words / 64 KiB / 30 min window — crates/listener-core/src/live_transcript/segments.rs#L11-L13.

### Hallucination and repeat guards
- whisper.cpp: greedy best_of 1, temperature 0 with +0.2 fallback, `suppress_blank`, `suppress_nst` (non-speech tokens), single segment, no timestamps, language fixed (detection off), the begin token suppressed, and an `initial_prompt` from a dynamic prompt — crates/whisper-local/src/model/actual.rs#L138-L175.
- Soniqo streaming partials are normalised: cumulative-update detection (≥ 4-token prefix), overlap/history prefix trimming (160-token history), collapse of internal repeats (≥ 6 tokens, ≤ 3 extra) — crates/listener-core/src/live_transcript/normalizer.rs#L5-L9, crates/listener-core/src/live_transcript/normalizer.rs#L160-L305.
- The VAD mask itself (zeroed silence) is the main guard against silence hallucination on the live path (my reading of crates/vad-masking/src/masking.rs#L30-L34).

### Vocabulary, languages, downloads, acceleration
- Custom vocabulary: `keywords` become Soniox `context.terms` (live and batch) and Deepgram `keywords`/`keyterm` by model — crates/owhisper-client/src/adapter/soniox/live.rs#L61-L65, crates/owhisper-client/src/adapter/soniox/batch.rs#L67-L68, crates/owhisper-client/src/adapter/deepgram/keywords.rs#L13-L23. Local whisper gets it only through the initial prompt.
- Languages: per-model language support predicates (Parakeet v2 English only, v3 European, Whisper multilingual) — crates/am/src/model.rs#L59-L66, crates/transcribe-soniqo/src/model.rs#L165-L175.
- Download: model-downloader verifies a u32 file checksum off-thread, then renames into place (atomic finalise) — crates/model-downloader/src/download_task/steps.rs#L31-L50, crates/model-downloader/src/download_task/steps.rs#L78-L82; zip/tar extraction — crates/model-downloader/src/archive.rs#L8-L8. Apple assets are OS-managed (no download size) — crates/local-stt-core/src/lib.rs#L61-L70.
- Acceleration: CoreML INT8 (Parakeet), MLX (Qwen3, gated off), Argmax's ANE/CoreML SDK, whisper.cpp Metal (`use_gpu`) — sources above; Windows/Linux have only whisper.cpp + cloud (Soniqo/Apple/Argmax are macOS Apple Silicon only: crates/transcribe-soniqo/src/model.rs#L146-L148).
- Measured latency/RTF/WER: Not found (searched crates/transcribe-*, crates/whisper-local, crates/listener*-core for "rtf", "wer", "latency_ms"); the only RTF gate is the diarization eval's RTF < 0.15 (crates/pyannote-local/eval/PROGRAM.md#L41-L43).

### Managed "Pro (Cloud)" vs local — how the choice is made
- The cloud path is a server crate, `transcribe-proxy`: provider param `anarlog` (or legacy `hyprnote`) turns on managed routing — crates/transcribe-proxy/src/anarlog_routing.rs#L191-L193. The router ranks available providers by language support for the requested languages, live vs batch, Soniox first then Deepgram (snapshot table: en/es/fr/de/hi/ru/pt/ja/it/nl → [Soniox, Deepgram]) — crates/transcribe-proxy/src/anarlog_routing.rs#L31-L35, crates/transcribe-proxy/src/anarlog_routing.rs#L85-L133, crates/transcribe-proxy/src/anarlog_routing.rs#L248-L276. Retries: 2, max delay 5 s, retryable = timeouts/"temporarily"/rate limits — crates/transcribe-proxy/src/anarlog_routing.rs#L6-L7, crates/transcribe-proxy/src/anarlog_routing.rs#L166-L189.
- Stereo is relayed as **one upstream session per channel** (`[mic, spk]`) in split mode, so me/them stay separate even at the cloud vendor — crates/transcribe-proxy/src/routes/streaming/anarlog.rs#L39-L39, crates/transcribe-proxy/src/relay/channel_split/coordinator.rs#L6-L40 (and the dual-Soniox mock test crates/transcribe-proxy/tests/anarlog_dual_soniox_mock.rs). A session gate supports drain-before-deploy — crates/transcribe-proxy/src/session_gate.rs#L28-L77.
- Client side the choice is simply which (base_url, model) the settings hold; local is chosen when the base URL is a local scheme/loopback — crates/listener-core/src/actors/listener/adapters.rs#L238-L255, crates/listener2-core/src/batch/diarize.rs#L46-L61. Who may use `anarlog` (subscription) is enforced server-side (crates/transcribe-proxy/src/supabase.rs); the plan/paywall UI is the SURFACE analyst's.
- Biometrics stay local even on the cloud path: cloud speaker labels are **consolidated on device** with the voiceprint model and the participant count (Soniox/Deepgram have no speaker-count parameter) — crates/listener2-core/src/batch/consolidate.rs#L1-L27.


## A3 Speakers

### Me-vs-them from channels
- The capture is stereo: channel 0 = direct mic (the user), channel 1 = system audio (every remote) — plugins/transcription/src/voiceprint.rs#L17-L19, plugins/transcription/src/voiceprint.rs#L822-L828.
- Batch planner per transcript channel: a call's mic channel is skipped for diarization (user alone); channel 1 is diarized with a speaker budget of participants minus the owner; a downmixed provider channel is scored on the mix with the full count — crates/listener2-core/src/batch/diarize.rs#L74-L113.
- In-person detection: several calendar participants but ≤ 3 s of speech on the system channel ⇒ everybody shared the mic, so the mic channel is diarized with the full participant count (word channel 2 = "shared mic") — crates/listener2-core/src/batch/diarize.rs#L27-L32, crates/listener2-core/src/batch/diarize.rs#L118-L134.
- Headphones ⇒ "isolated mic": every mic-channel voice sample is auto-confirmed as a *self* exemplar (confidence 0.9) without a manual label — plugins/transcription/src/voiceprint.rs#L20-L23, plugins/transcription/src/voiceprint.rs#L411-L418.

- Cloud transcripts get the same treatment after the fact: provider speaker labels are re-embedded locally (`wespeaker-local`) and consolidated — labels that match one known voice merge even under the cap; over the participant cap the most similar pair merges first; two known participants are never merged; ambiguous known matches do not merge; labels too short to embed follow their nearest neighbour in time; a brief talker within the count keeps its label — crates/listener2-core/src/batch/consolidate.rs#L27-L50, crates/listener2-core/src/batch/consolidate.rs#L350-L365, crates/listener2-core/src/batch/consolidate.rs#L465-L581, crates/listener2-core/src/batch/consolidate.rs#L658-L790.

### Thresholds at a glance
| Constant | Value | Cite |
|---|---|---|
| Voiceprint unique score floor | 0.62 | crates/voiceprint/src/matching.rs#L3-L3 |
| Voiceprint margin over runner-up | 0.08 | crates/voiceprint/src/matching.rs#L4-L4 |
| Candidate TTL | 45 days | plugins/transcription/src/voiceprint.rs#L15-L15 |
| Isolated-mic self label confidence | 0.9 | plugins/transcription/src/voiceprint.rs#L22-L22 |
| Span merge gap / min / max / per speaker | 400 ms / 1.5 s / 10 s / 3 | crates/voiceprint/src/lib.rs#L43-L52 |
| Diarizer step / clustering cut / min cluster | 2 s / 0.7045 / 12 s | crates/pyannote-local/src/pipeline.rs#L48-L58 |
| Remote channel "silent" | ≤ 3 s speech | crates/listener2-core/src/batch/diarize.rs#L27-L29 |
| Embedding | 256-d, 80 mel, 16 kHz | crates/embedding/src/onnx/model.rs#L6-L8 |

### Diarization engines
- `pyannote-local` — a Rust re-implementation of the pyannote.audio 3.1 offline recipe: segmentation-3.0 ONNX embedded in the binary (10 s windows at 16 kHz, powerset over 3 local speakers) — crates/pyannote-local/src/segmentation.rs#L6-L20; masked embeddings; centroid-linkage agglomerative clustering on unit-normalised embeddings cut at a fixed threshold, small clusters absorbed — crates/pyannote-local/src/clustering.rs#L1-L10, crates/pyannote-local/src/clustering.rs#L241-L260.
- Defaults: step 2 s ("keeps every frame under ~5 overlapping windows so majority voting suppresses per-window false speech"), min clean embedding 1.0 s, min activity 0.3 s, clustering threshold 0.7045, min cluster 12 s, max 1,800 windows (step grows on long audio), and known-speaker seeding at score ≥ 0.62 — crates/pyannote-local/src/pipeline.rs#L19-L59.
- Embeddings: a WeSpeaker-style ONNX (`feats` → `embs`, 80 mel bins via kaldi-native-fbank, 256-d, 16 kHz) — crates/embedding/src/onnx/model.rs#L1-L8, crates/embedding/src/onnx/mod.rs#L127-L145; the voiceprint layer tags vectors `pyannote` / `wespeaker-embedding-onnx-1` — plugins/transcription/src/voiceprint.rs#L13-L14.
- It only runs when a *local* batch provider returned unlabeled words ("cloud providers diarize server-side") — crates/listener2-core/src/batch/diarize.rs#L1-L3, crates/listener2-core/src/batch/diarize.rs#L46-L61, with progress from 0.95 and a 5 s heartbeat — crates/listener2-core/src/batch/diarize.rs#L21-L35.
- Soniqo batch path pairs Parakeet-TDT with speech-swift's `Community1DiarizationPipeline` (pyannote community-1) downloaded from Hugging Face — crates/transcribe-soniqo/swift-lib/src/lib.swift#L24-L24, crates/transcribe-soniqo/swift-lib/src/lib.swift#L160-L170, crates/transcribe-soniqo/swift-lib/src/lib.swift#L335-L335.
- `pyannote-cloud` is a generated client for the hosted pyannote API (crates/pyannote-cloud/src/lib.rs, crates/pyannote-cloud/build.rs); used server-side, not on the local path.
- Eval: a DER harness (collar 0.25 s) over AMI + VoxConverse with an **autoresearch ratchet program** — one change, measure, keep only if DER improves > 0.1 abs and RTF < 0.15, never touch the test split until the end — crates/pyannote-local/eval/PROGRAM.md#L1-L44, crates/pyannote-local/src/metrics.rs, crates/pyannote-local/src/bin/diarization_eval.rs.

### Voiceprints — enrollment, matching, recognition across meetings
- Span selection: from transcript words per (channel, speaker) merge runs with gaps ≤ 400 ms, cut out same-channel overlaps, keep spans 1.5–10 s (long spans keep their middle), top 3 per speaker by duration quality — crates/voiceprint/src/lib.rs#L36-L52, crates/voiceprint/src/lib.rs#L65-L183.
- Candidates → exemplars: candidate embeddings are stored with a 45-day TTL and **promoted** to exemplars when the user assigns the speaker (or automatically via the isolated-mic rule) — plugins/transcription/src/voiceprint.rs#L15-L15, plugins/transcription/src/voiceprint.rs#L240-L330, plugins/transcription/src/voiceprint.rs#L522-L523.
- Storage: the embedding vector itself is a **keyring secret** (scope + key on the row), moved before the DB row transition "so a crash can leave an orphan secret but never an exemplar without its vector" — plugins/transcription/src/voiceprint.rs#L340-L366.
- Matching: cosine similarity, only within the same capture domain (`direct_mic` vs `system_audio`) and the same model version; best sample per (speaker, person) — crates/voiceprint/src/matching.rs#L19-L40, crates/voiceprint/src/matching.rs#L190-L214, plugins/transcription/src/voiceprint.rs#L583-L590.
- Assignment rule: mutual unique best with score ≥ 0.62 and margin ≥ 0.08 over the runner-up, both from the speaker's side and the person's side; then **completion by elimination** when exactly one speaker and one participant remain open and counts match — crates/voiceprint/src/matching.rs#L3-L4, crates/voiceprint/src/matching.rs#L41-L105, crates/voiceprint/src/matching.rs#L135-L137, crates/voiceprint/src/matching.rs#L143-L163.
- Candidate set = the session's **calendar participants** minus the owner (by user id or any owner email); with fewer than 2 remotes the meeting is named from the participant list alone and voiceprints are not consulted — crates/voiceprint/src/matching.rs#L165-L188, plugins/transcription/src/voiceprint.rs#L929-L943.
- Known voices also seed the local diarizer (`known_speakers_for_session`) so fragmented clusters of a known voice merge — plugins/transcription/src/voiceprint.rs#L543-L556, crates/listener2-core/src/batch/diarize.rs#L36-L40.
- Results are written as speaker-assignment *hints* rather than hard labels — plugins/transcription/src/voiceprint.rs#L1231-L1232. Correction UI and how labels flow into notes: SURFACE analyst.


## A4 Notes, summaries and the LLM layer

Engine side only (prompts/templates/enhance are the SURFACE analyst's). Seen: crates/local-llm-core, crates/local-model, crates/gguf, crates/gbnf, plugins/local-llm, plugins/template.
- The bundled llama.cpp server is **gone**: `LlmServer::start_with_model_path` always returns "Local LLM is not supported on this platform" and the plugin's `start_server` is a no-op — crates/local-llm-core/src/server.rs#L5-L28, plugins/local-llm/src/ext.rs#L99-L104, plugins/local-llm/src/ext.rs#L172-L172. The GGUF catalogue survives for download/compat: "Anarlog LLM" (`hypr-llm.gguf`, from `yujonglee/hypr-llm-sm` Q4_K_M, 1.1 GB, "Experimental model trained by the Char team"), Gemma 3 4B Q4 and Llama 3.2 3B Q4 marked "Deprecated" — crates/local-model/src/lib.rs#L13-L54, crates/local-llm-core/src/model.rs#L3-L57; aarch64 only — crates/local-llm-core/src/model.rs#L3-L11.
- What replaces it: Apple **Foundation Models** (macOS 26) through a Swift bridge — `SystemLanguageModel.default.availability`, one `LanguageModelSession(instructions:)` per request, begin/cancel/generate keyed by request id — plugins/local-llm/src/foundation_models.rs#L31-L112, plugins/local-llm/swift-lib/src/lib.swift#L47-L52, plugins/local-llm/swift-lib/src/lib.swift#L91-L128; and user models found in LM Studio's folder (`list_custom_models`) — crates/local-llm-core/src/lib.rs#L56-L61. (Ollama/LM Studio servers as OpenAI-compatible endpoints are configured on the surface side.)
- `gguf`: reads GGUF metadata directly (magic check, `tokenizer.chat_template`, `general.architecture`, `general.name`) to pick a chat template — crates/gguf/src/lib.rs#L21-L113, crates/gguf/src/template.rs#L2-L9.
- `gbnf`: llama.cpp GBNF grammars for constrained output — `enhance` (a `<thinking>` block of 3–5 lines, then 3–5 `# Header` sections each with 3–5 "- **Bold**: sentence." bullets, or fixed known section names), `title` (capitalised words), `tags`, `email-to-name` — crates/gbnf/src/lib.rs#L1-L25, crates/gbnf/src/lib.rs#L27-L62, crates/gbnf/src/lib.rs#L73-L85; used by plugins/template (crates/gbnf is a dependency of plugins/template/Cargo.toml).
- Summary eval tooling exists in crates/template-eval (src/eval, src/mdgen.rs) — SURFACE analyst for detail.

## A5 Live assistance during the meeting

SURFACE analyst's axis. Engine facts that bear on it: the AX layer can capture in-meeting chat and post a message into the meeting's chat box (Zoom/Meet/Teams/Slack huddle) — crates/detect/src/meeting_ax.rs#L375-L376, crates/detect/src/meeting_ax.rs#L669-L670; Zoom mute state is observable live — crates/detect/src/zoom.rs#L72-L100; the live transcript is bounded at 4,096 words / 30 min for rendering — crates/listener-core/src/live_transcript/segments.rs#L11-L13. Files seen, not analysed: plugins/overlay, crates/overlay-kit, crates/dictation-ui-macos, plugins/dictation.

## A6 Memory, search, agent access and integrations

SURFACE analyst's axis. Files seen: `apps/cli` is the "Rust CLI/TUI and MCP entry point" (AGENTS.md#L1-L12 per the repo's own overview), plugins/mcp + crates/mcp, plugins/tantivy (full-text), crates/db-app (canonical SQLite schema shared desktop/mobile, AGENTS.md#L1-L12), calendar crates (crates/apple-calendar, crates/google-calendar, crates/outlook-calendar, crates/calendar-interface), agent crates crates/claude, crates/codex, crates/opencode, crates/agent-access. Engine link: calendar participants are the candidate set for voiceprint naming — plugins/transcription/src/voiceprint.rs#L929-L943.

## A7 UX surfaces and onboarding

SURFACE analyst's axis. Files seen: the macOS permission assistant with a drag-source view and a guide cursor that walks the user through System Settings — plugins/permissions/src/assistant/macos/views/drag_source.rs, plugins/permissions/src/assistant/macos/views/guide_cursor.rs, plugins/permissions/src/assistant/macos/settings_window.rs, plugins/permissions/src/ext.rs; plugins/tray, plugins/notification (+ crates/notification-macos/-windows/-linux), plugins/windows, plugins/dock. The mic-detect → notification policy is in plugins/detect/src/policy.rs#L113-L190.

## A8 Privacy, local-only guarantees and business model

SURFACE analyst's axis. Engine-side facts: voiceprint vectors are stored as keyring secrets, not DB rows — plugins/transcription/src/voiceprint.rs#L340-L366; cloud speaker labels are consolidated on device because "biometrics stay on device" — crates/listener2-core/src/batch/consolidate.rs#L1-L7; local STT never leaves loopback (127.0.0.1:0 bind) — plugins/local-stt/src/server/internal.rs#L72-L75; the managed path is the server crate crates/transcribe-proxy (Soniox/Deepgram). Licensing: MIT core plus a separate commercial `enterprise/` Cargo workspace (LICENSE.enterprise, LICENSING.md; AGENTS.md says community packages must not depend on it). Telemetry: Sentry plugin + OTEL/Honeycomb conventions (apps/desktop/src-tauri/src/lib.rs#L382-L382, OBSERVABILITY.md#L1-L12), plugins/analytics.

## A9 Engineering, platforms and traction

### Architecture
- Monorepo: pnpm + Cargo; Tauri 2 desktop (React/TS/Vite/Tailwind, Zustand), web, Expo mobile + watchOS with a Rust UniFFI bridge, Rust/Axum API, CLI/TUI/MCP binary, commercial `enterprise/` workspace — AGENTS.md#L1-L12 (repo doc). 153 crates and 47 plugins (directory listing).
- The desktop registers ~60 Tauri plugins; the engine-relevant ones are permissions, mcp, detect, updater2, transcription, audio-priority, local-llm, local-stt, sentry — apps/desktop/src-tauri/src/lib.rs#L309-L382.
- Meeting runtime is a **ractor actor tree**: root → session actor supervising source (capture + AEC + VAD mask), recorder (disk) and listener (STT socket); the session retries a failed recorder/listener on a timer and the listener is spawned in `post_start` so a connection failure degrades instead of killing the session — crates/listener-core/src/actors/mod.rs#L1-L11, crates/listener-core/src/actors/session/supervisor.rs#L44-L52, crates/listener-core/src/actors/session/supervisor.rs#L79-L79, crates/listener-core/src/actors/session/supervisor.rs#L109-L110. A generic restart/backoff crate exists (crates/supervisor/src/retry.rs, crates/supervisor/src/restart.rs).
- Post-meeting batch (`listener2-core`) is a separate actor runtime (batch, progressive, consolidate, diarize) — crates/listener2-core/src/lib.rs#L1-L5.
- Native Apple frameworks are reached through small Swift packages built by `build.rs` + swift-rs (tcc, SpeechAnalyzer, Soniqo, Foundation Models) — crates/tcc/build.rs, crates/transcribe-speechanalyzer/build.rs, crates/transcribe-soniqo/build.rs, plugins/local-llm/swift-lib/src/lib.swift#L1-L3.

### Platforms per feature (from cfg gates)
| Feature | macOS | Windows | Linux |
|---|---|---|---|
| Mic-in-use detection per app | CoreAudio process list — crates/detect/src/list/macos.rs#L84-L106 | yes — crates/detect/src/mic/windows.rs#L90-L147 | PulseAudio — crates/detect/src/mic/linux.rs#L1-L36 |
| Zoom mute / sleep events | yes — crates/detect/src/lib.rs#L58-L65 | no | no |
| AX meeting/chat introspection | yes — crates/detect/src/meeting_ax.rs#L226-L250 | partial — crates/detect/src/meeting_ax/windows.rs | partial — crates/detect/src/meeting_ax/linux.rs |
| System audio | process tap — crates/audio-actual/src/speaker/macos.rs#L53-L86 | WASAPI loopback — crates/audio-actual/src/speaker/windows.rs#L198-L217 | PipeWire/Pulse — crates/audio-actual/src/speaker/linux.rs#L10-L26 |
| Neural AEC | yes (ONNX, all OS) — crates/aec/Cargo.toml#L7-L11 | yes | yes |
| Soniqo Parakeet / Apple Speech / Argmax | Apple Silicon only — crates/transcribe-soniqo/src/model.rs#L146-L148, crates/transcribe-speechanalyzer/swift-lib/src/lib.swift#L118-L118 | no | no |
| whisper.cpp local | yes | yes | yes (legacy list) |
| Bundled GGUF LLM | stubbed off — crates/local-llm-core/src/server.rs#L9-L17 | stubbed | stubbed |

### Tests, evals, diagnostics
- ~706 Rust files contain `#[test]`/`#[tokio::test]` and 152 integration test files under crates/plugins (grep count). Capture has deterministic fault tests (channel failure, dual outage, startup timeout) — crates/audio-actual/src/capture/recovery.rs#L219-L342, crates/audio-actual/src/capture/stream.rs#L805-L849; AX parsing has fixture suites (crates/detect/src/meeting_ax/tests/); audio sync has an e2e test (crates/audio-sync/tests/e2e.rs).
- Black-box desktop E2E (WebdriverIO) incl. a Linux virtual-audio test that "persists separated microphone and system-audio tracks" — e2e/blackbox/tests/linux-virtual-audio.spec.ts#L7-L11; a dedicated CI workflow installs PipeWire for it — .github/workflows/desktop_linux_audio_qa.yaml#L1-L2, .github/workflows/desktop_linux_audio_qa.yaml#L325-L340; a macOS runner action pre-seeds TCC.db — .github/actions/macos_tcc/action.yaml#L1-L44.
- Evals: diarization DER/RTF harness + autoresearch program — crates/pyannote-local/eval/PROGRAM.md#L1-L44; summary/template eval crate crates/template-eval. Not found: an STT WER harness (searched crates/*transcribe*, crates/whisper*, crates/owhisper* for "wer").
- Diagnostics: OTEL→Honeycomb, Sentry tags, `x-request-id` conventions — OBSERVABILITY.md#L1-L12; structured `tracing` event names throughout (e.g. "aec_disabled_headphone_output", crates/listener-core/src/actors/source/stream.rs#L53-L57).
- Updater: plugins/updater2, registered at apps/desktop/src-tauri/src/lib.rs#L344-L344; macOS builds for aarch64 + x86_64 on `macos-26` runners — .github/workflows/desktop_cd.yaml#L152-L161; 30+ workflows incl. desktop_e2e, flathub submit, store publish (.github/workflows/ listing).

### Release cadence and traction (GitHub API, 2026-10-10)
- Last 20 releases span 2026-09-04 → 2026-10-08: stable `desktop_v1.4.19` … `desktop_v1.4.29` (11 stables in 5 weeks, ~2 per week) plus `1.4.24-nightly.1…9` prereleases (2026-09-11 → 09-19) — GitHub MCP `list_releases(fastrepl/anarlog)`.
- Repo: 9,471 stars, 791 forks, 11 open issues+PRs (3 open PRs), created 2024-12-09, pushed 2026-10-10 11:12 UTC, description "Open source Granola AI Alternative" — GitHub search API. Issue search reports 966 issues in total; the item list came back empty unauthenticated and the authenticated MCP search hit a rate limit, so the top-reacted issues are **Not found** (searched `/search/issues?q=repo:fastrepl/anarlog+is:issue&sort=reactions` twice).
- Process: the repo is written for coding agents (AGENTS.md per component, GitButler multi-agent rules, Linear for decisions) — AGENTS.md#L1-L45.

### Not found (searched, absent)
- STT WER / latency / RTF numbers for any engine — searched crates/transcribe-*, crates/whisper*, crates/owhisper*, crates/listener*-core for "wer", "rtf", "latency_ms".
- AEC quality numbers (ERLE, PESQ, dB) — searched crates/aec, crates/audio-actual.
- Calendar-triggered recording start — crates/detect emits only mic/zoom/sleep events (crates/detect/src/lib.rs#L55-L66); calendar crates feed participants, not start triggers (SURFACE analyst to confirm UI behaviour).
- Per-app (process-scoped) system-audio capture — the macOS tap is global with an empty exclusion list (crates/audio-actual/src/speaker/macos.rs#L53-L55).
- A running local LLM server — stubbed (crates/local-llm-core/src/server.rs#L9-L17).
- Top-reacted issues — GitHub search returned a count (966) but no items; MCP search rate-limited.

### Engineering habits worth noting
- Constants carry their reason in a comment next to them (e.g. AEC headphone rule crates/listener-core/src/actors/source/stream.rs#L48-L49, diarizer step crates/pyannote-local/src/pipeline.rs#L48-L51, voiceprint crash ordering plugins/transcription/src/voiceprint.rs#L354-L355).
- Fault behaviour is pinned by tests with paused tokio time (recovery and cooldown tests: crates/audio-actual/src/capture/recovery.rs#L219-L342, plugins/detect/src/mic_usage_tracker.rs#L109-L181).
- Snapshot tests freeze policy tables (routing table per language: crates/transcribe-proxy/src/anarlog_routing.rs#L248-L276; category ↔ bundle id coverage: plugins/detect/src/policy.rs#L219-L245).

## Map to the three questions

theuth facts below are taken from AXES.md's summary of theuth (the OURS analyst verifies them) plus the knowledge base entry on theuth's off-macOS capture (`MicNoAec`, "no echo cancellation; headphones recommended" on Windows/Linux, 2026-09-23). Anarlog facts are cited.

### Q1 — What they offer that theuth does not (engine half)
1. **Automatic meeting detection** from per-app mic use with category deny-lists, a 15 s sustain and a 10 min cooldown — crates/detect/src/list/macos.rs#L84-L106, plugins/detect/src/policy.rs#L19-L84, plugins/detect/src/mic_usage_tracker.rs#L8-L9. AXES lists no auto-start for theuth.
2. **Meeting-app introspection through Accessibility**: platform/surface identification, Zoom mute state, chat capture and chat *send* — crates/detect/src/meeting_ax.rs#L669-L670, crates/detect/src/meeting_ax.rs#L375-L376, crates/detect/src/zoom.rs#L72-L100.
3. **Cross-platform neural AEC with reference alignment and a residual subtractor**, auto-off on headphones — crates/aec/src/onnx/model.rs#L1-L24, crates/audio-actual/src/capture/stream.rs#L685-L747, crates/listener-core/src/actors/source/stream.rs#L48-L76. theuth has no AEC off macOS (knowledge entry above).
4. **Per-channel capture recovery with silence gap fill** and crash-repairable recordings — crates/audio-actual/src/capture/recovery.rs#L27-L33, crates/listener-core/src/actors/recorder/disk.rs#L34-L60.
5. **Many STT engines behind one protocol** (CoreML Parakeet, SpeechAnalyzer, Argmax, whisper.cpp, 20+ clouds) and a managed language-ranked cloud router with per-channel upstream sessions — crates/local-stt-core/src/lib.rs#L3-L10, crates/listener-core/src/actors/listener/adapters.rs#L171-L217, crates/transcribe-proxy/src/anarlog_routing.rs#L85-L133.
6. **Live → batch re-transcription** with the streaming model's batch twin plus a diarizer — crates/transcribe-soniqo/src/model.rs#L158-L163, crates/listener2-core/src/batch/progressive/actor.rs#L19-L21.
7. **Automatic speaker naming across meetings**: calendar-constrained voiceprint matching (mutual unique, margin, elimination), isolated-mic self-enrollment, participant-count-bounded diarization, in-person detection, on-device consolidation of cloud speakers — crates/voiceprint/src/matching.rs#L41-L163, plugins/transcription/src/voiceprint.rs#L411-L418, crates/listener2-core/src/batch/diarize.rs#L74-L134, crates/listener2-core/src/batch/consolidate.rs#L1-L27. theuth enrolls a speaker from a segment manually (AXES).
8. **Evaluation and CI for audio**: DER/RTF harness with an autoresearch ratchet; Linux virtual-audio black-box E2E — crates/pyannote-local/eval/PROGRAM.md#L1-L44, e2e/blackbox/tests/linux-virtual-audio.spec.ts#L7-L11.
9. **Grammar-constrained local summaries** (GBNF) — crates/gbnf/src/lib.rs#L53-L62 — although their in-app llama.cpp runtime is now stubbed off (crates/local-llm-core/src/server.rs#L9-L17).

### Q1 — What theuth has that they do not (engine half; from AXES)
- Live TOPICS and tool-written BRIEFS during the meeting (knowledge first, then web) and push-to-talk steering; Anarlog's engine has nothing equivalent (A5 owner to confirm on the surface side).
- An always-on-top island hidden from screen capture by default.
- Compressed Ogg Opus tracks written live (three per meeting) vs Anarlog's WAV-then-mp3 (crates/listener-core/src/actors/recorder/disk.rs#L16-L18).
- A general agent harness around the meeting: `meeting_read` / `meeting_search` / `meeting_speakers` tools, knowledge harvest, notebooks, subagents, schedules, brief cache across meetings.
- A user correction pass (relabel/merge/split) as a first-class step (Anarlog writes assignment *hints*: plugins/transcription/src/voiceprint.rs#L1231-L1232; its UI is the SURFACE analyst's).

### Q2 — Techniques worth borrowing, with where they land in theuth
| # | Technique (re-implement, never copy) | Anarlog cite | Lands in theuth |
|---|---|---|---|
| 1 | Mic-in-use per app → "meeting?" prompt; deny-list categories; 15 s sustain; 10 min cooldown; user include/ignore | crates/detect/src/list/macos.rs#L84-L106, plugins/detect/src/policy.rs#L125-L204, plugins/detect/src/mic_usage_tracker.rs#L8-L58 | crates/meeting detector + companion/island prompt; Windows via WASAPI session enumeration |
| 2 | Loopback→mic lag alignment by FFT cross-correlation with acquire/hold/lose hysteresis; reset AEC on lag change | crates/audio-sync/src/probe.rs#L8-L70, crates/audio-actual/src/capture/stream.rs#L373-L512 | meeting capture before any AEC; also talk/voice AEC |
| 3 | Neural AEC (two-stage mask + time-domain ONNX, 512/128 blocks) followed by a linear residual subtractor with a double-talk guard | crates/aec/src/onnx/mod.rs#L251-L310, crates/audio-actual/src/capture/stream.rs#L685-L775 | Windows/Linux meeting capture (replaces `MicNoAec`), ORT already in theuth |
| 4 | Headphones ⇒ AEC off and mic "isolated" ⇒ auto self-voiceprint | crates/listener-core/src/actors/source/stream.rs#L40-L76, plugins/transcription/src/voiceprint.rs#L411-L418 | capture settings + speaker profile store |
| 5 | Per-channel failure isolation: 5 s frame timeout, 1/2/4 s retries, 30 s healthy reset, silence gap fill | crates/audio-actual/src/capture/recovery.rs#L27-L33, crates/audio-actual/src/capture/stream.rs#L346-L347 | meeting capture loop (device unplug, AirPods switch) |
| 6 | Append-mode recording with 1 s flush and repair-on-reopen | crates/listener-core/src/actors/recorder/disk.rs#L16-L60 | Ogg Opus writer: page flush cadence + recovery on restart |
| 7 | Voiceprint assignment: mutual unique best ≥ 0.62, margin ≥ 0.08, elimination, same capture domain + model version, calendar participants as the candidate set | crates/voiceprint/src/matching.rs#L41-L214, plugins/transcription/src/voiceprint.rs#L929-L943 | post-meeting diarization → `meeting_speakers`; calendar via theuth's calendar access |
| 8 | Participant-count-bounded diarization per channel; in-person detection when the system channel is silent | crates/listener2-core/src/batch/diarize.rs#L74-L134 | theuth's pyannote-rs/CAM++ pass (speaker count hint) |
| 9 | Voiceprint vectors as vault secrets, candidate TTL 45 days, write secret before the row | plugins/transcription/src/voiceprint.rs#L15-L15, plugins/transcription/src/voiceprint.rs#L340-L366 | theuth secrets vault + speaker table |
| 10 | Live model → batch twin after stop, results replace live words | crates/transcribe-soniqo/src/model.rs#L158-L163, crates/listener2-core/src/batch/progressive/actor.rs#L19-L21 | post-meeting pipeline (re-transcribe Parakeet TDT before diarization) |
| 11 | DER (collar 0.25 s) eval on AMI + VoxConverse, ratchet: keep if ΔDER > 0.1 and RTF < 0.15, test split untouched | crates/pyannote-local/eval/PROGRAM.md#L1-L44 | an autoresearch study (design 140) on theuth's diarizer |
| 12 | One STT protocol; local engines are base URLs under a restart supervisor | plugins/local-stt/src/server/internal.rs#L52-L75, plugins/local-stt/src/server/supervisor.rs#L19-L28 | crates/voice / meeting STT seam; cloud fallback without new code paths |
| 13 | Streaming-partial normaliser: cumulative-prefix detection, repeat collapse | crates/listener-core/src/live_transcript/normalizer.rs#L5-L9, crates/listener-core/src/live_transcript/normalizer.rs#L160-L305 | live transcript feeding live topics (fewer duplicate topics) |
| 14 | CI virtual-audio E2E asserting separated mic/system tracks | e2e/blackbox/tests/linux-virtual-audio.spec.ts#L7-L11, .github/workflows/desktop_linux_audio_qa.yaml#L325-L340 | build-A PipeWire null-sink recipe as a regression test |
| 15 | Zoom mute state from the AX tree (button title) | crates/detect/src/zoom.rs#L72-L100 | island state ("you are muted") / live-topic gating |
| 16 | Slide capture by colour signature with stable/new thresholds, cell-change count, inner-area mask and a remembered-frame set | crates/detect/src/screen.rs#L7-L47 | meeting note attachments; context for live briefs (what is on screen) |
| 17 | Typed degraded STT errors that never stop capture | crates/listener-core/src/lib.rs#L48-L59 | meeting STT seam + island status line |

### Q3 — The highest-leverage changes, in order
1. **Auto-detect meetings** (technique 1): the biggest UX gap; small, well-bounded code (CoreAudio process list + policy + 15 s sustain + cooldown) with a prompt on the island/companion. Windows twin via audio-session enumeration.
2. **Echo cancellation everywhere** (techniques 2–4): align first, then neural AEC + residual subtractor, bypass on headphones. It removes the far-end echo from the "me" track, which today pollutes speakers, topics and briefs on Windows/Linux and on Mac speakers.
3. **Automatic speaker naming** (techniques 7–9): calendar attendees as the candidate set, mutual-unique voiceprint rule with elimination, self-voiceprint from isolated-mic meetings, voiceprints in the vault with a TTL. Turns theuth's manual enroll into recognition across meetings.
4. **Capture resilience** (techniques 5–6): per-channel recovery with gap fill and a flush/repair contract for the Opus writer — protects long meetings from device switches and crashes.
5. **Measure the diarizer** (technique 11): stand up the DER harness and run it as an autoresearch study; it also gives the numbers to choose between pyannote-rs/CAM++ and alternatives.

### Re-implementation sketches for the top three (techniques only)
Auto-detect (Q3 #1):
1. Listen for the default input's running-somewhere property; fall back to a 1 s poll if registration fails (crates/detect/src/mic/macos/mod.rs#L151-L195).
2. On change, list processes with running input and resolve each pid to a bundle id; diff against the last set (crates/detect/src/list/macos.rs#L84-L106, crates/detect/src/mic/macos/app.rs#L8-L28).
3. Drop theuth itself, dictation, IDE, recorder and assistant categories; apply the user's include/ignore overrides (plugins/detect/src/policy.rs#L125-L204).
4. Start a per-app timer; after 15 s still holding the mic, prompt once and cool the app down for 10 min (plugins/detect/src/mic_usage_tracker.rs#L8-L98).

AEC chain (Q3 #2):
1. Pair mic and loopback chunks into frames; if either side is down, pass the healthy side and fill silence (crates/audio-actual/src/capture/stream.rs#L297-L330).
2. Estimate the lag by FFT cross-correlation, lock with hysteresis, delay the leading side, reset the canceller when the lock moves (crates/audio-sync/src/probe.rs#L8-L46, crates/audio-actual/src/capture/stream.rs#L444-L512).
3. Run the two-stage neural canceller on 512-sample blocks with 128-sample hop; keep LSTM state across frames (crates/aec/src/onnx/mod.rs#L203-L310).
4. Subtract the remaining loopback-correlated component with a clamped, smoothed gain; switch to a trimmed segment estimate under double talk (crates/audio-actual/src/capture/stream.rs#L685-L775).
5. Keep raw mic, raw loopback and the cancelled mic side by side; downstream prefers the cancelled one; bypass all of it on headphones (crates/listener-core/src/actors/source/pipeline.rs#L1484-L1516, crates/listener-core/src/actors/source/stream.rs#L48-L76).

Speaker naming (Q3 #3):
1. Pick 1.5–10 s single-speaker spans per (channel, speaker), ≤ 3 each (crates/voiceprint/src/lib.rs#L36-L183).
2. Embed them; store candidates with a TTL; promote to exemplars on user confirmation or when the mic was isolated (plugins/transcription/src/voiceprint.rs#L240-L418).
3. At naming time restrict candidates to the meeting's attendees minus the owner; skip voiceprints when the calendar already names ≤ 1 remote (plugins/transcription/src/voiceprint.rs#L929-L943).
4. Assign only mutual unique best matches above the floor and margin; then complete by elimination (crates/voiceprint/src/matching.rs#L41-L163).
5. Write the result as hints the correction UI can overturn (plugins/transcription/src/voiceprint.rs#L1231-L1232).

## Cites self-check

Method: every cite below was re-opened with `read_file(offset, limit)` over exactly the cited range and checked for the token in brackets; 88 re-opened, 88 held, 0 corrected. Cites in prose that point at whole files or directory listings (e.g. crates/aec/examples/audacity.rs, plugins/permissions/src/assistant/macos/) are labelled as files, not mechanisms.

- `crates/detect/src/list/macos.rs#L84-L106` — held (token: `is_running_input`)
- `crates/detect/src/mic/macos/state.rs#L25-L35` — held (token: `from_millis(500)`)
- `plugins/detect/src/policy.rs#L19-L84` — held (token: `ScreenRecording`)
- `crates/detect/src/meeting_ax.rs#L375-L376` — held (token: `send_meeting_chat_message`)
- `crates/detect/src/meeting_ax.rs#L669-L670` — held (token: `capture_meeting_chat_messages`)
- `crates/aec/src/onnx/model.rs#L22-L24` — held (token: `BLOCK_SHIFT: usize = 128`)
- `crates/audio-actual/src/capture/stream.rs#L29-L39` — held (token: `AEC_MAX_REFERENCE_LAG_MS: u32 = 600`)
- `crates/audio-actual/src/capture/stream.rs#L685-L747` — held (token: `AEC_DOUBLE_TALK_RESIDUAL_RATIO`)
- `crates/listener-core/src/actors/source/stream.rs#L48-L76` — held (token: `headphone_output`)
- `crates/audio-actual/src/capture/recovery.rs#L27-L33` — held (token: `FRAME_TIMEOUT`)
- `crates/listener-core/src/actors/recorder/disk.rs#L16-L20` — held (token: `FLUSH_INTERVAL`)
- `crates/audio-actual/src/speaker/macos.rs#L53-L86` — held (token: `with_mono_global_tap_excluding_processes`)
- `crates/tcc/swift-lib/src/lib.swift#L7-L83` — held (token: `TCCAccessPreflight`)
- `crates/local-stt-core/src/lib.rs#L3-L10` — held (token: `ParakeetStreaming`)
- `crates/transcribe-soniqo/src/model.rs#L100-L108` — held (token: `Parakeet-EOU-120M-CoreML-INT8`)
- `plugins/local-stt/src/server/internal.rs#L52-L75` — held (token: `LOCALHOST`)
- `crates/transcribe-proxy/src/anarlog_routing.rs#L191-L193` — held (token: `"anarlog" \| "hyprnote"`)
- `crates/transcribe-proxy/src/routes/streaming/anarlog.rs#L39-L39` — held (token: `[mic, spk]`)
- `crates/voiceprint/src/matching.rs#L3-L4` — held (token: `0.62`)
- `crates/voiceprint/src/matching.rs#L143-L163` — held (token: `complete_by_elimination`)
- `plugins/transcription/src/voiceprint.rs#L340-L366` — held (token: `a crash can leave an`)
- `plugins/transcription/src/voiceprint.rs#L929-L943` — held (token: `remote_human_ids.len() < 2`)
- `crates/pyannote-local/src/pipeline.rs#L19-L59` — held (token: `0.7045`)
- `crates/listener2-core/src/batch/diarize.rs#L118-L134` — held (token: `REMOTE_SILENCE_MAX_SECONDS`)
- `crates/listener2-core/src/batch/consolidate.rs#L1-L27` — held (token: `biometrics stay on device`)
- `crates/local-llm-core/src/server.rs#L9-L17` — held (token: `not supported on this platform`)
- `crates/whisper-local/src/model/actual.rs#L138-L175` — held (token: `set_suppress_nst`)
- `crates/pyannote-local/eval/PROGRAM.md#L41-L43` — held (token: `0.1 absolute`)
- `crates/detect/src/zoom.rs#L72-L100` — held (token: `Unmute`)
- `e2e/blackbox/tests/linux-virtual-audio.spec.ts#L7-L11` — held (token: `separated microphone`)
- `crates/listener-core/src/actors/session/supervisor.rs#L109-L110` — held (token: `post_start`)
- `crates/detect/src/mic/macos/mod.rs#L151-L195` — held (token: `enable_polling_fallback`)
- `crates/vad-masking/src/streaming.rs#L4-L19` — held (token: `hangover_frames: 6`)
- `crates/audio-chunking/src/speech.rs#L16-L22` — held (token: `from_millis(600)`)
- `crates/listener-core/src/actors/source/pipeline.rs#L1484-L1516` — held (token: `aec_mic`)
- `apps/desktop/src-tauri/src/lib.rs#L309-L382` — held (token: `tauri_plugin_detect::init`)
- `crates/listener-core/src/actors/listener/adapters.rs#L171-L217` — held (token: `Anarlog => AnarlogAdapter`)
- `crates/gbnf/src/lib.rs#L53-L62` — held (token: `thinking ::=`)
- `plugins/local-llm/src/ext.rs#L172-L172` — held (token: `start_server`)
- `crates/listener-core/src/live_transcript/normalizer.rs#L5-L9` — held (token: `SONIQO_HISTORY_TOKEN_LIMIT`)
- `crates/detect/src/meeting_ax/types.rs#L6-L21` — held (token: `GoogleMeet`)
- `crates/aec/Cargo.toml#L7-L11` — held (token: `default = ["onnx", "128"]`)
- `crates/am/src/model.rs#L27-L41` — held (token: `argmaxinc/parakeetkit-pro`)
- `crates/transcribe-speechanalyzer/swift-lib/src/lib.swift#L174-L246` — held (token: `maximumReservedLocales`)
- `plugins/local-stt/src/server/supervisor.rs#L19-L28` — held (token: `max_restarts: 100`)
- `.github/actions/macos_tcc/action.yaml#L1-L44` — held (token: `TCC.db`)
- `.github/workflows/desktop_cd.yaml#L152-L161` — held (token: `macos-26`)
- `crates/local-llm-core/src/lib.rs#L56-L61` — held (token: `lmstudio`)
- `crates/gguf/src/lib.rs#L21-L113` — held (token: `tokenizer.chat_template`)
- `crates/listener2-core/src/lib.rs#L1-L5` — held (token: `mod batch`)
- `crates/voiceprint/src/lib.rs#L36-L52` — held (token: `merge_gap_ms: 400`)
- `plugins/transcription/src/live_journal.rs#L20-L28` — held (token: `BATCH_WINDOW`)
- `plugins/transcription/src/audio_retention.rs#L12-L19` — held (token: `SWEEP_INTERVAL`)
- `crates/device-monitor/src/lib.rs#L86-L88` — held (token: `from_millis(1000)`)
- `crates/owhisper-client/src/adapter/deepgram/keywords.rs#L13-L23` — held (token: `keyterm`)
- `crates/listener-core/src/live_transcript/segments.rs#L11-L13` — held (token: `MAX_RENDERED_WORDS`)
- `crates/vad/src/silero_onnx/v6.rs#L4-L24` — held (token: `silero_v6.2.onnx`)
- `crates/whisper-local-model/src/lib.rs#L34-L62` — held (token: `ggml-large-v3-turbo-q8_0.bin`)
- `crates/whisper-local/src/model/actual.rs#L31-L37` — held (token: `crash on macos`)
- `crates/audio-chunking/src/vad/chunk_policy.rs#L14-L16` — held (token: `MIN_DETECTED_SPEECH_MS: u64 = 200`)
- `crates/listener2-core/src/batch/progressive/actor.rs#L19-L21` — held (token: `BATCH_STREAM_TIMEOUT_SECS: u64 = 30`)
- `crates/pyannote-local/src/segmentation.rs#L6-L20` — held (token: `segmentation-3.0`)
- `crates/embedding/src/onnx/model.rs#L1-L8` — held (token: `EMBEDDING_DIM: usize = 256`)
- `plugins/transcription/src/voiceprint.rs#L13-L23` — held (token: `wespeaker-embedding-onnx-1`)
- `crates/transcribe-soniqo/swift-lib/Package.swift#L15-L27` — held (token: `soniqo/speech-swift`)
- `crates/local-model/src/lib.rs#L13-L54` — held (token: `yujonglee/hypr-llm-sm`)
- `plugins/local-llm/swift-lib/src/lib.swift#L91-L128` — held (token: `LanguageModelSession`)
- `crates/audio-actual/src/speaker/windows.rs#L26-L30` — held (token: `ENDPOINT_SWITCH_CONFIRMATIONS`)
- `crates/listener-core/src/actors/session/supervisor.rs#L44-L52` — held (token: `RetryRecorder`)
- `crates/transcribe-proxy/src/anarlog_routing.rs#L248-L276` — held (token: `[Soniox, Deepgram]`)
- `plugins/detect/src/mic_usage_tracker.rs#L8-L9` — held (token: `DEFAULT_MIC_ACTIVE_THRESHOLD_SECS: u64 = 15`)
- `crates/audio-sync/src/probe.rs#L8-L46` — held (token: `acquire: SyncProbeThresholds::new(10.0, 1.15)`)
- `crates/audio-sync/src/probe.rs#L52-L70` — held (token: `window_samples: 4096`)
- `crates/listener2-core/src/batch/consolidate.rs#L658-L790` — held (token: `cap_merge_never_joins_two_known_participants`)
- `crates/detect/src/screen.rs#L7-L19` — held (token: `NEW_CONTENT_DISTANCE: f32 = 0.06`)
- `crates/detect/src/sleep.rs#L41-L66` — held (token: `NSWorkspaceWillSleepNotification`)
- `crates/listener-core/src/lib.rs#L48-L59` — held (token: `connection_timeout`)
- `crates/owhisper-interface/src/stream.rs#L9-L60` — held (token: `punctuated_word`)
- `crates/audio-actual/src/capture/stream.rs#L421-L431` — held (token: `AEC_MAX_REFERENCE_LAG_MS`)
- `crates/transcribe-soniqo/src/model.rs#L66-L80` — held (token: `SELECTABLE`)
- `crates/audio-actual/src/capture/stream.rs#L297-L330` — held (token: `push_mic`)
- `crates/audio-actual/src/capture/stream.rs#L444-L512` — held (token: `fn align`)
- `crates/aec/src/onnx/mod.rs#L203-L310` — held (token: `process_streaming`)
- `crates/voiceprint/src/lib.rs#L36-L183` — held (token: `fn select_speaker_spans`)
- `plugins/transcription/src/voiceprint.rs#L240-L418` — held (token: `confirm_isolated_mic_candidates`)
- `crates/detect/src/mic/macos/app.rs#L8-L28` — held (token: `fn diff_apps`)
- `plugins/transcription/src/voiceprint.rs#L1231-L1232` — held (token: `write_speaker_assignment_hints`)
- `crates/detect/src/screen.rs#L25-L47` — held (token: `shows_new_content`)

Flagged inferences (not code facts): the DTLN-aec identification of the AEC model (crates/aec/src/onnx/model.rs#L1-L24 names no architecture); the reading that the VAD mask is the main live hallucination guard (crates/vad-masking/src/masking.rs#L30-L34); theuth-side statements in Q1/Q3 come from AXES.md and the knowledge base, not from theuth's source (the OURS analyst verifies them).

GitHub numbers: releases via the authenticated GitHub MCP `list_releases` (20 rows, 2026-09-04 → 2026-10-08); stars/forks/open issues via `search_repositories`; unauthenticated REST `/repos/...` calls returned 403 rate limit, so the brief's exact URLs were substituted by these equivalents.
