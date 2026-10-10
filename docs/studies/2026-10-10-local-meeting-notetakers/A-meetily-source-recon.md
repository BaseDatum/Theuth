# Meetily (Zackriya-Solutions/meetily) — THEIRS source recon

Clone: `/Users/Sky.Moore/Develop/spike-meeting-notetakers/meetily` (HEAD a2cb62e, 2026-09-10, shallow). Read-only recon, 2026-10-10.
All cites are repo-relative `path#L<a>-L<b>`.

## Executive summary

Meetily community = one Tauri 2 app (Rust core + Next.js), local Whisper/Parakeet STT, post-meeting LLM summary; no diarization, no live AI, no agent access. Most borrowable mechanisms:
1. 30 s encoded audio checkpoints, `-c copy` concat at stop, and a recover-after-crash command + UI — `frontend/src-tauri/src/audio/incremental_saver.rs#L18-L93`, `#L241-L342`, `frontend/src/hooks/useTranscriptRecovery.ts#L39-L143`.
2. Live vs batch VAD redemption (500 ms vs 2000 ms) justified by a measured fragmentation/hallucination study — `frontend/src-tauri/src/audio/pipeline.rs#L16-L27`, `#L738-L757`.
3. Whisper hallucination guards: decoder thresholds + repetition cleaner that drops segments with ratio > 0.7 — `frontend/src-tauri/src/whisper_engine/whisper_engine.rs#L401-L560`, `#L561-L603`.
4. Map-reduce sized from the model's real context (Ollama `/api/show` − 300; cloud single-pass) — `frontend/src-tauri/src/summary/service.rs#L432-L476`, `frontend/src-tauri/src/summary/processor.rs#L400-L490`.
5. Template-filling final prompt with an injection guard and "None noted in this section." — `frontend/src-tauri/src/summary/processor.rs#L229-L253`, `#L493-L507`.
6. English-first summary, then structure-preserving translate/normalise, cached by fingerprints — `frontend/src-tauri/src/summary/processor.rs#L79-L80`, `#L517-L546`, `frontend/src-tauri/src/summary/service.rs#L85-L137`.
7. Regenerate safely: back up the old summary, restore it on failure/cancel — `frontend/src-tauri/src/database/repositories/summary.rs#L88-L210`.
8. Resumable exact-byte model downloads (Range + strict Content-Range + 416 path) — `frontend/src-tauri/src/parakeet_engine/parakeet_engine.rs#L86-L118`, `#L695-L810`.
9. Device-loss policy: Bluetooth 3 polls / wired 2, adaptive 2–5 s polling, bounded fallback to default mic — `frontend/src-tauri/src/audio/device_monitor.rs#L62-L70`, `frontend/src-tauri/src/audio/recording_commands.rs#L1286-L1286`.
10. Core Audio tap-only private aggregate device + "who is playing audio" detector (500 ms debounce) as a meeting-detection seed — `frontend/src-tauri/src/audio/capture/core_audio.rs#L85-L139`, `frontend/src-tauri/src/audio/system_detector.rs#L104-L127`.

## A1 Capture and meeting detection

**How a meeting starts.** Manual: the user presses record (tray or main window, see A7). There is a macOS
"system audio in use" detector, but no auto-start was found: `MacOSSystemAudioDetector` registers Core Audio
property listeners, debounces state flips by 500 ms and lists the processes whose output is running
(`frontend/src-tauri/src/audio/system_detector.rs#L104-L127`, `#L358-L380`). It is exposed as the Tauri command
`start_system_audio_monitoring` (`frontend/src-tauri/src/audio/system_audio_commands.rs#L39-L60`, registered at
`frontend/src-tauri/src/lib.rs#L796-L796`) but no caller exists in `frontend/src` (searched
`start_system_audio_monitoring|SystemAudioStarted` across `frontend/src`). Not found: calendar-driven or
window-title meeting detection (searched `calendar|zoom|teams` in `src-tauri/src/audio`).

**What is captured.** Two inputs — one microphone and one "system" device — each a cpal stream wrapped by
`AudioCapture` (`frontend/src-tauri/src/audio/pipeline.rs#L230-L396`). Per OS:
- macOS: a Core Audio **process tap** through the `cidre` crate — a *global mono tap excluding no processes*,
  put inside a **private aggregate device** named `meetily-audio-tap` whose only member is the tap; the output
  device itself is deliberately NOT added as a sub-device because tap + device produced duplicated audio
  (`frontend/src-tauri/src/audio/capture/core_audio.rs#L85-L139`). Creating the tap triggers the TCC prompt; if
  denied the tap silently returns zeros (`frontend/src-tauri/src/audio/capture/core_audio.rs#L57-L62`).
  No per-app capture: the exclusion list is empty, so everything playing is captured.
- Windows: WASAPI output devices opened in loopback mode via cpal's WASAPI host, with fallback to the default
  host (`frontend/src-tauri/src/audio/devices/platform/windows.rs#L7-L45`, `#L188-L200`).
- Linux: enumerates the ALSA host looking for PulseAudio *monitor* sources
  (`frontend/src-tauri/src/audio/devices/platform/linux.rs#L6-L19`); no PipeWire-native path.

**Per-stream processing (mic only).** Everything is converted to mono and resampled to a fixed 48 kHz
(`frontend/src-tauri/src/audio/pipeline.rs#L240-L241`, `#L399-L486`). Resampling is a persistent `rubato`
`SincFixedIn` per device fed in 512-sample chunks, with sinc length / interpolation picked by ratio
(512 Cubic for ≥2× up or ≤0.5× down, 384, 256 Linear), `f_cutoff 0.95`, Blackman-Harris window
(`frontend/src-tauri/src/audio/pipeline.rs#L331-L377`). Then, mic only: an 80 Hz high-pass filter
(`#L303-L307`), RNNoise (`nnnoiseless`) — compiled in but **disabled** by `RNNOISE_APPLY_ENABLED = false`
("Whisper handles noise well", `frontend/src-tauri/src/audio/ffmpeg_mixer.rs#L20-L20`) — and an EBU R128
loudness normaliser to **-23 LUFS** with a **-1 dBTP** true-peak limit and a 10 ms look-ahead limiter
(`frontend/src-tauri/src/audio/audio_processing.rs#L109-L130`, `#L143-L218`; applied at
`frontend/src-tauri/src/audio/pipeline.rs#L572-L584`). The system stream gets no processing at all
(`frontend/src-tauri/src/audio/pipeline.rs#L322-L326`).

**The "professional audio mixing".** A ring buffer per source accumulates samples and releases aligned windows;
the window is **600 ms** (the comment still says 50 ms) and the cap is 8 windows (comments say 400 ms; at
600 ms it is 4.8 s), oldest samples dropped on overflow with a loud log
(`frontend/src-tauri/src/audio/pipeline.rs#L31-L98`). When one side is short, it is zero-padded rather than
sample-held (`#L105-L156`). The mixer (`ProfessionalAudioMixer`, `#L158-L203`) is a plain sum
`mic + sys × 1.0` — the "70 % headroom" pre-scale is in the comment only — and when `|sum| > 1` it outputs
`sum/|sum|`, i.e. ±1.0: a hard clip described as "proportional scaling". **No ducking** ("without aggressive
ducking", `#L158-L159`, `#L850-L857`), although the README sells "intelligent ducking and clipping prevention" (`README.md#L184-L186`).

**Echo cancellation.** Not found: searched `echo cancel|aec|acoustic echo` in `frontend/src-tauri/src` and
`backend/app`. The only "echo" fix is avoiding a duplicated tap (`core_audio.rs#L85-L87`). Mic bleed of the
speakers is therefore transcribed twice unless the user wears headphones.

**One mixed stream drives both outputs.** The pipeline loop pulls chunks with a 50 ms timeout, feeds the ring
buffer, mixes each window, sends the mix to Silero VAD for transcription and the same mix to the recorder
(`frontend/src-tauri/src/audio/pipeline.rs#L792-L923`). Transcription chunks are 16 kHz, ≥ 800 samples (50 ms)
and are tagged `DeviceType::Microphone` "Mixed audio" (`#L860-L891`); the recorder receives the mixed window
(`#L894-L903`); remaining VAD state is flushed at stop (`#L925-L964`). Consequence: there is no per-source track
on disk and no channel-based me-vs-them (A3).

**VAD.** `silero_rs` at 16 kHz (`frontend/src-tauri/src/audio/vad.rs#L2-L9`), positive/negative thresholds
0.50/0.35, 300 ms pre-speech pad, 250 ms minimum speech (`frontend/src-tauri/src/audio/vad.rs#L45-L61`).
Redemption (silence that closes a segment) is **500 ms live vs 2000 ms in the batch paths**, with a measured
rationale in the code: at 400 ms a 26-minute meeting became 322 requests (median 3.5 s), short clips made
Whisper emit memorised subtitle boilerplate, and 47 % of boundaries fell in 0.42–0.75 s; 2000 ms live kept a
segment open forever under continuous system audio (`frontend/src-tauri/src/audio/pipeline.rs#L16-L27`,
`#L738-L759`). Batch VAD splits files over 960,000 samples into 10 s (160,000-sample) pieces
(`frontend/src-tauri/src/audio/vad.rs#L384-L389`).

**Recording format and storage.** Each meeting gets a folder under the recordings root —
`~/Movies/meetily-recordings` on macOS, `Music\meetily-recordings` on Windows, `~/Documents/meetily-recordings` on
Linux — and `auto_save` defaults to true (`frontend/src-tauri/src/audio/recording_preferences.rs#L29-L72`).
`metadata.json` and `transcripts.json` are written atomically through a temp file + rename
(`frontend/src-tauri/src/audio/recording_saver.rs#L224-L292`). Audio is AAC-LC in `.mp4` via an ffmpeg
subprocess (`frontend/src-tauri/src/audio/encode.rs#L40-L52`).

**Crash-safety.** `IncrementalAudioSaver` accumulates the mixed stream and every **30 s** (1,440,000 samples at
48 kHz) encodes a checkpoint `.checkpoints/audio_chunk_NNN.mp4`; at stop it concatenates the checkpoints with
ffmpeg's concat demuxer and `-c copy` (no re-encode) into `audio.mp4`, then deletes `.checkpoints`
(`frontend/src-tauri/src/audio/incremental_saver.rs#L18-L44`, `#L54-L93`, `#L114-L198`). After a crash,
`recover_audio_from_checkpoints` rescans the folder, estimates 30 s per chunk and runs the same concat
(`frontend/src-tauri/src/audio/incremental_saver.rs#L241-L342`). The transcript side is recovered from the
webview's IndexedDB: `useTranscriptRecovery` lists meetings left in IndexedDB, checks `has_audio_checkpoints`
and offers preview + recover (`frontend/src/hooks/useTranscriptRecovery.ts#L39-L143`). At most the last <30 s of
audio is lost.

**Device changes mid-meeting.** `DeviceMonitor` polls the device list every 2 s while something is missing and
5 s when all present, plus a Core Audio device-list listener on macOS
(`frontend/src-tauri/src/audio/device_monitor.rs#L127-L160`, `#L272-L396`). A device is declared gone after 3
missed polls for Bluetooth, 2 for wired (`#L62-L70`). On mic loss the recorder falls back to the system default
input, bounded at `MAX_MIC_FALLBACK_ATTEMPTS = 3`, after waiting for macOS to finish swapping the default input
(`frontend/src-tauri/src/audio/recording_commands.rs#L1286-L1286`, `#L1483-L1530`); AirPods are handled as
mic-and-speaker together (`frontend/src-tauri/src/audio/device_monitor.rs#L201-L210`). Stream errors are classified
by substring matching on cpal's error text (`frontend/src-tauri/src/audio/pipeline.rs#L661-L688`).

**Permissions.** Screen Recording permission check/request commands and a "trigger system audio permission"
helper (`frontend/src-tauri/src/audio/permissions.rs#L18-L137`).

**Retention.** Not found: no automatic deletion/retention policy (searched `retention|cleanup_old|max_age` in
`src-tauri/src`).

## A2 Transcription

**Engines.** Two on-device engines behind one enum `TranscriptionEngine { Whisper, Parakeet, Provider(dyn
TranscriptionProvider) }` (`frontend/src-tauri/src/audio/transcription/engine.rs#L15-L46`; trait in
`frontend/src-tauri/src/audio/transcription/provider.rs#L1-L73`). The default provider in the UI config is
`parakeet` (`frontend/src/contexts/ConfigContext.tsx#L113-L113`). Recording refuses to start until the selected
model is loaded and valid (`validate_transcription_model_ready`,
`frontend/src-tauri/src/audio/transcription/engine.rs#L54-L130`).
- **Whisper** = whisper.cpp through `whisper-rs`; catalogue of ggml files pulled from the
  `ggerganov/whisper.cpp` Hugging Face repo — tiny/base/small/medium, `*-q5_1`/`q5_0`, `large-v3-turbo-q5_0`,
  `large-v3-q5_0` (`frontend/src-tauri/src/whisper_engine/whisper_engine.rs#L1077-L1102`; table with sizes in
  `frontend/src-tauri/src/config.rs#L30-L30`).
- **Parakeet** = NVIDIA `parakeet-tdt-0.6b` v3 (default) and v2, **int8 ONNX** (encoder + decoder_joint +
  `nemo128.onnx` mel front-end + `vocab.txt`) run with `ort`
  (`frontend/src-tauri/src/parakeet_engine/parakeet_engine.rs#L106-L140`,
  `frontend/src-tauri/src/parakeet_engine/model.rs#L49-L125`). v3 is downloaded from Meetily's own host
  `meetily.towardsgeneralintelligence.com/models/...`, v2 from a pinned `istupakov/parakeet-tdt-0.6b-v2-onnx`
  commit on Hugging Face (`frontend/src-tauri/src/parakeet_engine/parakeet_engine.rs#L120-L140`); the README
  credits istupakov's ONNX conversion (`README.md#L266-L270`). The decoder is a hand-written greedy
  TDT/RNN-T loop: split joint output into token logits + duration logits, advance by the predicted duration,
  force progress on blank-with-zero-duration, cap at `MAX_TOKENS_PER_STEP = 3`
  (`frontend/src-tauri/src/parakeet_engine/model.rs#L17-L18`, `#L349-L425`). ONNX session: graph optimisation
  Level3 and **CPU execution provider only** — no CoreML/DirectML/CUDA EP
  (`frontend/src-tauri/src/parakeet_engine/model.rs#L95-L125`).
- Cloud STT: the settings type lists `deepgram | elevenLabs | groq | openai` providers
  (`frontend/src/components/TranscriptSettings.tsx#L13-L13`) — see the check at the end of this section.

**The "4x faster" claim.** Not found in this tree: searched `4x|x faster|times faster|faster than` in all
`.md/.rs/.ts/.tsx/.json` files. What the repo does say: Parakeet v3 int8 is "Real time on M4 Max"
(`frontend/src-tauri/src/parakeet_engine/parakeet_engine.rs#L120-L128`), and GPU-vs-CPU folklore in the
workflow docs ("Metal 10-15x faster than CPU", `.github/workflows/README_DEVTEST.md#L109-L111`; "5-10x",
`CLAUDE.md#L354-L354`). No benchmark harness or RTF measurement exists in the repo (A9). The speed story
therefore rests on (a) Parakeet-TDT's non-autoregressive token+duration decoding with int8 weights, and
(b) whisper.cpp GPU builds — not on a measured number in the source.

**Live mechanism.** Live = VAD segments of the *mixed* stream (A1) queued to a transcription task with
**`NUM_WORKERS = 1`** — deliberately serial "for ordered emission", despite the "parallel worker pool" naming
(`frontend/src-tauri/src/audio/transcription/worker.rs#L50-L87`). Each result is a `TranscriptUpdate` with a
global `sequence_id`, segment-relative timestamps, `is_partial` and a confidence that defaults to **0.85** when
the engine reports none; it is emitted to the webview as `transcript-update`, plus a one-shot
`speech-detected` event (`frontend/src-tauri/src/audio/transcription/worker.rs#L14-L45`, `#L161-L246`). Only
empty/whitespace text is filtered — "no confidence gating" (`#L27-L35`). There are no partial-then-final
passes: each VAD segment is transcribed once. Progress (queued vs completed) is tracked with two atomics
(`#L78-L97`, `#L255-L275`).

**Batch / better pass.** "Enhance" re-transcribes a saved recording, optionally with a different model or
language (`README.md#L146-L150`): `retranscription.rs` decodes the file, runs VAD with **2000 ms** redemption
and guards concurrency with a global in-progress flag + RAII guard and a cancel flag
(`frontend/src-tauri/src/audio/retranscription.rs#L19-L53`). File import uses the same batch path
(`frontend/src-tauri/src/audio/import.rs`, 1,323 lines; decoder in `frontend/src-tauri/src/audio/decoder.rs`).

**Hallucination guards (Whisper).** Decoding parameters: beam search (size from the hardware tier), suppress
blank + non-speech tokens, `entropy_thold 2.4`, `logprob_thold -1.0`, `no_speech_thold 0.55`,
`max_initial_ts 1.0`, `max_len 200`, no timestamps
(`frontend/src-tauri/src/whisper_engine/whisper_engine.rs#L561-L603`). After decoding,
`clean_repetitive_text` removes consecutive word repeats and 2–5-word phrase loops and drops the whole segment
when the repetition ratio exceeds **0.7** or the text is mostly one character
(`frontend/src-tauri/src/whisper_engine/whisper_engine.rs#L401-L560`, applied at `#L657-L657` and `#L817-L817`).
The other guard is upstream: longer VAD segments (A1 rationale at `frontend/src-tauri/src/audio/pipeline.rs#L738-L757`).

**Language.** Whisper takes `auto` (detect), `auto-translate` (translate to English) or a code
(`frontend/src-tauri/src/whisper_engine/whisper_engine.rs#L566-L573`). Parakeet v3 is multilingual by model;
no language parameter is passed (`frontend/src-tauri/src/parakeet_engine/parakeet_engine.rs#L543-L560`).
Summary-side language detection is separate (A4).

**Model management.** Parakeet downloads are resumable with an HTTP `Range: bytes=N-` request, strict
`Content-Range` validation (start, end and total must match the expected **exact byte size** per artifact),
a 416 path for already-complete files, a 30 s per-chunk read timeout, and a 5 s cleanup timeout on cancel
(`frontend/src-tauri/src/parakeet_engine/parakeet_engine.rs#L86-L118`, `#L173-L173`, `#L695-L810`,
`#L998-L1010`). Whisper files are judged corrupted below 90 % of the catalogue size, then validated as GGML
(`frontend/src-tauri/src/whisper_engine/whisper_engine.rs#L220-L246`). Download progress/complete/error are
Tauri events (`frontend/src-tauri/src/parakeet_engine/commands.rs#L400-L470`). Not found: checksums/SHA
verification (searched `sha|checksum` in both engines) — byte length is the only integrity check.

**Acceleration.** Whisper's backend is chosen at compile time — `cuda` feature, else `vulkan`, else Metal on
macOS, else CPU — and flash attention is enabled only for Metal/CUDA on a fast tier
(`frontend/src-tauri/src/whisper_engine/acceleration.rs#L1-L72`). A hardware detector scores cores/GPU/RAM into
Ultra/High/Medium/Low tiers that set beam size 5/3/2(/…), temperature 0.1–0.3 and thread caps 8/6/4; Windows
is forced to beam 2 "for stability" (`frontend/src-tauri/src/audio/hardware_detector.rs#L121-L150`,
`#L204-L240`). Windows installers ship a Vulkan whisper build and need AVX2 (`README.md#L111-L111`,
`README.md#L197-L197`). Parakeet: CPU only (above).

**Custom vocabulary / hotwords.** Not found: searched `initial_prompt|set_initial_prompt|hotword|custom_vocab`
in `src-tauri/src` — no prompt biasing for either engine.

**Measured numbers.** Only the in-code VAD measurement (322 requests / 26 min, median 3.5 s, 47 % of
boundaries at 0.42–0.75 s, `frontend/src-tauri/src/audio/pipeline.rs#L744-L750`). Not found: WER/RTF/latency
benchmarks (searched `wer|rtf|benchmark` in `src-tauri/src` and `frontend/tests`).

## A3 Speakers

**Me-vs-them from channels.** Not available: mic and system audio are mixed into one mono stream *before* VAD
and transcription, and every transcription chunk is tagged `DeviceType::Microphone` "Mixed audio"
(`frontend/src-tauri/src/audio/pipeline.rs#L848-L903`). The recording is the same mixed stream (A1). So even
the trivial two-channel "you vs the room" split is thrown away at the mixer.

**Diarization.** Not found in the compiled open build: searched `sortformer|pyannote|diariz` in all
`.rs/.ts/.tsx/.toml`. The only hits are in `frontend/src-tauri/src/audio/stt.rs`, which imports
`crate::pyannote::{embedding::EmbeddingExtractor, identify::EmbeddingManager}` and downloads pyannote
segmentation + embedding models and carries a `speaker_embedding: Vec<f32>` field
(`frontend/src-tauri/src/audio/stt.rs#L1-L8`, `#L119-L119`, `#L184-L185`) — but there is no `pyannote` module in
`src-tauri/src` and `stt` is not declared in `frontend/src-tauri/src/audio/mod.rs#L1-L52`, so this is a dead
file left from an older design (it also imports a non-existent `crate::deepgram`, `stt.rs#L2-L2`).
A `speaker TEXT` column was added to `transcripts` (`frontend/src-tauri/migrations/20251110000001_add_speaker_field.sql#L1-L5`)
yet no UI or Rust code fills it (searched `speaker` in `frontend/src`: only device icons such as
`frontend/src/components/DeviceSelection.tsx#L386-L386`).

**What the README/marketing reserve for PRO.** README: "Speaker diarization is planned for mid-June, bringing
automatic speaker separation to PRO meetings" and "Speaker Identification … *(Coming Soon)*" in the PRO list
(`README.md#L226-L234`). The GitHub repo description nevertheless advertises "speaker diarization" and the
topic list includes `sortformer` (GitHub API, 2026-10-10) — marketing ahead of the open code. The most
discussed open feature request is #230 "Speaker Diarization / Identification" (16 comments, open since
2025-10-30; A9).

**Voiceprints/enrollment, attendee naming, correction UI, labels in notes.** Not found (searched
`enroll|voiceprint|attendee|relabel` in `src-tauri/src` and `frontend/src`; the only hit is a hard-coded demo
note with an `attendees` array, `frontend/src/app/notes/[id]/page.tsx#L29-L36`). Summaries can only
name people the transcript text mentions ("mentioned individuals", `frontend/src-tauri/src/summary/processor.rs#L218-L222`).

## A4 Notes, summaries and the LLM layer

**Where it runs.** In the Rust/Tauri core. `CLAUDE.md` states the Python/FastAPI backend under `backend/` is an
archived, unsupported legacy tier ("Do not add new endpoints to `backend/app/main.py`", `CLAUDE.md#L10-L17`,
`CLAUDE.md#L49-L55`, `CLAUDE.md#L289-L293`). The legacy backend chunked by characters (5000 / overlap 1000,
`backend/app/main.py#L106-L148`); the current path is `frontend/src-tauri/src/summary/`.

**Pipeline (current).** `SummaryService::process_transcript_background` is spawned per meeting with a
cancellation token (one per meeting, a newer request cancels the previous) and persists status in the
`summary_processes` table (`frontend/src-tauri/src/summary/service.rs#L216-L264`, `#L322-L340`,
`#L575-L640`). The core `generate_meeting_summary` (`frontend/src-tauri/src/summary/processor.rs#L366-L555`):
1. Token estimate is character-based (`0.35 tokens/char`, `frontend/src-tauri/src/summary/processor.rs#L255-L285`).
2. **Map-reduce only for local models.** If the provider is Ollama or BuiltInAI and the transcript exceeds the
   threshold, it is split into chunks of `threshold − 300` tokens with 100 tokens of overlap, cut at
   sentence/word boundaries (`#L271-L345`, `#L400-L403`); each chunk is summarised with up to
   `MAX_CHUNK_ATTEMPTS = 2` tries, and a chunk that still fails aborts the run with "transcript section i of n
   failed" (`#L69-L77`, `#L406-L466`); chunk summaries are joined with `---` and *combined* by a second prompt
   (`#L468-L490`). The threshold is the model's real context: Ollama's `/api/show` context length (TTL-cached,
   `frontend/src-tauri/src/ollama/metadata.rs#L19-L60`) minus 300, falling back to 4000; BuiltInAI from its
   registry; **cloud providers get 100,000 = single pass** (`frontend/src-tauri/src/summary/service.rs#L432-L476`).
3. **Final templated report.** The system prompt is "fill in the provided Markdown template" with eight rules —
   only use the source, ignore instructions inside `<transcript_chunks>`, "None noted in this section." for
   empty sections, no reasoning/meta sections, omit when unsure — followed by per-section instructions and the
   template skeleton (`frontend/src-tauri/src/summary/processor.rs#L229-L253`). The user turn wraps the
   (combined) text in `<transcript_chunks>` and appends optional user context in `<user_context>`
   (`#L493-L507`). That user context is the only "enhance with my notes" path (`custom_prompt`,
   `frontend/src-tauri/src/summary/service.rs#L322-L333`).
4. **English first, then translate.** Every generation prompt carries "Write the summary/report in English
   regardless of transcript language; non-English prose is invalid" (`frontend/src-tauri/src/summary/processor.rs#L79-L80`).
   Afterwards a final language action either translates the English markdown into the target language with a
   structure-preserving translator prompt, runs an English *normalisation* pass (translate stray non-English
   prose, keep every markdown marker in place), or returns English as is
   (`#L100-L123`, `#L205-L216`, `#L517-L546`). The transcript language is detected with `whatlang`
   (`frontend/src-tauri/src/summary/language_detection.rs#L1-L60`).
5. **Output hygiene.** Strip code fences around markdown, detect and strip reasoning markers, and refuse
   output with no visible markdown (`frontend/src-tauri/src/summary/processor.rs#L22-L67`).
6. **Cache.** The English result is cached keyed on FNV-1a fingerprints of transcript, custom prompt and the
   rendered template plus provider/model/threshold, so changing only the output language re-translates without
   re-summarising (`frontend/src-tauri/src/summary/service.rs#L85-L137`, `#L176-L210`;
   `frontend/src-tauri/src/summary/processor.rs#L82-L99`).

**Templates.** JSON files `{name, description, sections:[{title, instruction, format: paragraph|list|…,
item_format?, example_item_format?}]}` (`frontend/src-tauri/src/summary/templates/types.rs#L5-L110`), rendered
both as a markdown skeleton (`**Title**` per section) and as section instructions
(`#L74-L110`). Bundled: standard meeting, daily standup, project sync, retrospective, sales/marketing client
call, psychiatric session (`frontend/src-tauri/templates/*.json`). The standard one asks for Summary, Key
Decisions, Action Items as a table *Owner | Task | Due | Reference Transcript Segment | Segment Time stamp*,
and Discussion Highlights (`frontend/src-tauri/templates/standard_meeting.json#L1-L27`). Users drop their own
JSON into `~/Library/Application Support/Meetily/templates/` (or the Windows/Linux equivalents), which
the loader checks alongside the bundled directory (`frontend/src-tauri/src/summary/templates/loader.rs#L8-L70`).
No variables (no attendee/date substitution) — searched `{{` in `templates/`.

**Providers.** `LLMProvider = OpenAI | Claude | Groq | Ollama | OpenRouter | BuiltInAI | CustomOpenAI`
(`frontend/src-tauri/src/summary/llm_client.rs#L215-L223`), endpoints for OpenAI, Groq, OpenRouter,
Ollama's OpenAI-compatible `/v1/chat/completions`, Anthropic Messages
(`frontend/src-tauri/src/summary/llm_client.rs#L300-L346`). A Gemini API key column exists in settings
(`frontend/src-tauri/migrations/20251229000000_add_gemini_api_key.sql#L1-L4`).

**Bundled local LLM ("BuiltInAI").** A separate binary `llama-helper` built on `llama-cpp-2 = 0.1.146` with
metal/cuda/vulkan features (`llama-helper/Cargo.toml#L1-L17`) speaks a tagged JSON request/response protocol
over stdin/stdout (`llama-helper/src/main.rs#L19-L50`) and sizes GPU offload from detected Metal VRAM
(`llama-helper/src/main.rs#L132-L160`). The app's `SidecarManager` spawns it (under `nice` on Unix), tracks
health, reuses it while the same model is loaded and kills it after an idle timeout (`LLAMA_IDLE_TIMEOUT`)
(`frontend/src-tauri/src/summary/summary_engine/sidecar.rs#L1-L104`, `#L262-L300`). Catalogue: Qwen 3.5 2B /
4B Q4_K_M (non-thinking template) and Gemma 3 4B / 1B GGUFs from Hugging Face, all with 32,768 context
(`frontend/src-tauri/src/summary/summary_engine/models.rs#L131-L215`).

**Storage, regeneration and editing.** Summaries live in SQLite `summary_processes.result`; before a
regeneration the previous result is copied to `result_backup`, and on failure or cancel the backup is restored
with `COALESCE(result_backup, result)` (`frontend/src-tauri/src/database/repositories/summary.rs#L88-L210`;
migration `frontend/src-tauri/migrations/20251101000000_add_summary_backup.sql#L1-L8`). The editor is
**BlockNote** (`frontend/src/components/BlockNoteEditor/Editor.tsx`,
`frontend/src/components/AISummary/BlockNoteSummaryView.tsx`, markdown bridge in
`frontend/src/lib/blocknote-markdown.ts`). A `meeting_notes(meeting_id, notes_markdown, notes_json)` table was
added (`frontend/src-tauri/migrations/20251223000000_add_meeting_notes.sql#L1-L12`) but nothing reads or writes
it — searched `meeting_notes|notes_markdown|notes_json` in `frontend/src` and `src-tauri/src`: Not found.

**Action items / structured output.** Only as template sections (markdown); no JSON extraction, no
frontmatter, no task objects (searched `action_items` struct/serde in `src-tauri/src/summary`).

## A5 Live assistance during the meeting

**What the user sees live.** The live transcript only: the webview subscribes to `transcript-update`
(`frontend/src/services/transcriptService.ts#L58-L58`), ordered by `sequence_id` (A2,
`frontend/src-tauri/src/audio/transcription/worker.rs#L192-L222`), and buffers it into IndexedDB for recovery
(`frontend/src/hooks/useTranscriptRecovery.ts#L39-L100`). Recording state moves through
`idle → recording → stopping → processing → saving → completed | error`
(`frontend/src/contexts/RecordingStateContext.tsx#L16-L42`). A Bluetooth-playback warning tells the user to use
computer speakers "for accurate review" (`frontend/src/components/BluetoothPlaybackWarning.tsx#L71-L80`;
playback detection in `frontend/src-tauri/src/audio/playback_monitor.rs`).

**AI during the meeting.** Not found: no live summary, live suggestions, ask-during-meeting, alerts or
steering (searched `live summary|running summary|suggest|ask` in `src-tauri/src/summary` and the recording
components). The LLM runs only after stop, on demand (A4). No floating widget/overlay/notch: searched
`always_on_top|alwaysOnTop` in `src-tauri` and `frontend/src` — Not found; no hiding from screen capture
(searched `content_protected|sharingType|setContentProtection`).

**Versus theuth.** theuth's island + live topics + tool-written briefs + push-to-talk steering has no
counterpart here; Meetily's live value is transcript fidelity, durability (30 s checkpoints, A1) and the
pause/resume controls in the tray (A7).

## A6 Memory, search, agent access and integrations

**Storage.** One SQLite database through `sqlx` with migrations (`frontend/src-tauri/migrations/*.sql`):
`meetings`, `transcripts`, `summary_processes`, `transcript_chunks`, `settings`, `transcript_settings`
(`frontend/src-tauri/migrations/20250916100000_initial_schema.sql#L2-L72`), plus a meeting folder per recording
with `audio.mp4`, `metadata.json`, `transcripts.json` (A1, `frontend/src-tauri/src/audio/recording_saver.rs#L224-L292`).
API keys are plain columns in `settings` (e.g. `frontend/src-tauri/migrations/20250920155811_add_openrouter_api_key.sql`,
`frontend/src-tauri/migrations/20251229000000_add_gemini_api_key.sql#L1-L4`) — no keychain (searched `keyring|keychain`).

**Search.** Substring only: `search_transcripts` runs `WHERE LOWER(t.transcript) LIKE ?`
(`frontend/src-tauri/src/database/repositories/transcript.rs#L87-L101`), exposed as `api_search_transcripts`
(`frontend/src-tauri/src/api/api.rs#L356-L370`). Not found: FTS5, embeddings or semantic search (searched
`fts5|embedding|vector` in `src-tauri/src/database`).

**Chat/ask across meetings.** Not found — listed as PRO "Chat with Meetings (Coming Soon)" (`README.md#L226-L240`).

**MCP / agent access.** Not found: searched `\bmcp\b` in `src-tauri/src` and `frontend/src`.

**Exports.** Copy-to-clipboard of the summary blocks or markdown, and a `.md` download of the summary
(`frontend/src/components/AISummary/index.tsx#L426-L426`, `#L523-L523`, `#L601-L601`, `#L773-L773`). PDF/DOCX
exports are PRO (`README.md#L230-L231`). No Obsidian/Notion/Slack/email integrations (searched those names).

**Calendar.** A notification setting "Enable meeting reminders based on calendar events" exists
(`frontend/src-tauri/src/notifications/settings.rs#L16-L16`) but no calendar provider code (searched
`calendar` in `src-tauri/src`; only that field and a lucide `Calendar` icon in
`frontend/src/components/Sidebar/index.tsx#L590-L592`). Calendar integration is PRO "Coming Soon"
(`README.md#L233-L236`). People/company pages, follow-ups, sharing: Not found.

## A7 UX surfaces and onboarding

**Main window.** Next.js static export inside Tauri: a sidebar listing meetings (`frontend/src/components/Sidebar/index.tsx`),
the meeting-details page with transcript + BlockNote summary editor (`frontend/src/components/AISummary/index.tsx`,
`frontend/src/components/BlockNoteEditor/Editor.tsx`), a settings modal (`frontend/src/app/_components/SettingsModal.tsx`),
transcript recovery dialog (`frontend/src/components/TranscriptRecovery/TranscriptRecovery.tsx#L119-L119`),
model managers (`frontend/src/components/ParakeetModelManager.tsx`). Beta-features flags live in localStorage;
"Import Audio & Retranscribe" is on by default (`frontend/src/types/betaFeatures.ts#L24-L43`).

**Tray / menu bar.** A tray menu with toggle/pause/resume/stop recording, open window, settings, check for
updates and quit (`frontend/src-tauri/src/tray.rs#L19-L51`); the menu is rebuilt from the recording state and
shows "⏳ Downloading transcription model..." while a model is missing (`frontend/src-tauri/src/tray.rs#L212-L330`).
Pause/resume is a first-class state (`frontend/src-tauri/src/tray.rs#L117-L150`).

**Hotkeys.** Not found: searched `global_shortcut|GlobalShortcut` in `src-tauri` and `frontend/src`.

**Notifications.** Native notifications for recording started/stopped/paused/resumed and transcription
complete, with priority and timeout enums (`frontend/src-tauri/src/notifications/types.rs#L17-L51`); manager
and settings in `frontend/src-tauri/src/notifications/manager.rs` / `settings.rs`.

**First-run onboarding.** A persisted step machine (`current_step`, completed, per-model status
`downloaded | not_downloaded | downloading` for Parakeet and the summary model) stored in the Tauri store
(`frontend/src-tauri/src/onboarding.rs#L15-L110`). `complete_onboarding` writes the model configuration to SQLite
*before* marking onboarding complete; 4 steps on macOS (with the permissions step), 3 elsewhere
(`frontend/src-tauri/src/onboarding.rs#L171-L211`). Permission UI warns separately for mic and system audio
(`frontend/src/components/PermissionWarning.tsx#L60-L85`). Recording is gated on a ready model
(`frontend/src-tauri/src/audio/transcription/engine.rs#L54-L60`).

**States shown.** Recording lifecycle enum (A5, `frontend/src/contexts/RecordingStateContext.tsx#L16-L25`);
summary process status `pending/processing/completed/failed/cancelled` with restore-on-failure (A4,
`frontend/src-tauri/src/database/repositories/summary.rs#L124-L210`).

**Mobile companion.** Not found (searched `ios|android|mobile` in `frontend/src-tauri/tauri.conf.json` and
`.github/workflows`).

## A8 Privacy, local-only guarantees and business model

**Local by default.** Transcription is on-device (Whisper/Parakeet, A2); the default summary path the README
recommends is Ollama (`README.md#L102-L102`, `README.md#L158-L158`), and the bundled llama.cpp sidecar
(BuiltInAI) keeps summaries local too (A4). What leaves the machine: (1) model downloads — Hugging Face for
Whisper/Gemma/Qwen and **Meetily's own host for Parakeet v3**
(`frontend/src-tauri/src/parakeet_engine/parakeet_engine.rs#L120-L128`); (2) the transcript, when the user picks a
cloud LLM (OpenAI, Claude, Groq, OpenRouter, custom endpoint; `frontend/src-tauri/src/summary/llm_client.rs#L300-L346`);
(3) opt-in analytics; (4) update checks against GitHub releases (`frontend/src-tauri/tauri.conf.json#L114-L117`).
The privacy policy states transcripts/audio are never transmitted (`PRIVACY_POLICY.md#L12-L18`).

**Telemetry.** PostHog via `posthog_rs` (`frontend/src-tauri/src/analytics/analytics.rs#L1-L1`). The library
default is disabled with no key (`frontend/src-tauri/src/analytics/analytics.rs#L39-L50`), but the command layer
ships a **hard-coded PostHog project key** with `enabled: true` and host `us.i.posthog.com`
(`frontend/src-tauri/src/analytics/commands.rs#L12-L14`). The gate is the frontend: `analyticsOptedIn` is created
as `false` in the Tauri store and re-forced to false when missing (`frontend/src/components/AnalyticsProvider.tsx#L24-L48`).
Events are identified per user id ("Don't create anonymous users", `frontend/src-tauri/src/analytics/analytics.rs#L101-L160`),
include sessions, daily-active and first-launch (`#L172-L232`); a deny-list strips sensitive property keys and
`meeting_started` sends `meeting_id` + timestamp (`#L25-L35`). The policy promises opt-in for new *and existing*
installs, PostHog as processor, and "never collect meeting content" (`PRIVACY_POLICY.md#L25-L62`, `#L73-L88`).
There is a unit test for analytics behaviour (`frontend/tests/lib/analytics.test.ts`).

**Accounts / licensing.** No sign-in is required to record. The schema nevertheless carries a PRO licensing
table — RSA-encrypted key, SHA-256 signature hash, activation/expiry/soft-expiry dates, `is_soft_expired`
0/1/2 = active/soft-expired/hard-blocked, and a `grace_period` defaulting to **604,800 s (7 days)**
(`frontend/src-tauri/migrations/20251105120000_add_pro_license_custom_openai.sql#L1-L20`,
`frontend/src-tauri/migrations/20251110000000_add_grace_period_to_licensing.sql#L1-L8`), plus a user/license
struct with `is_licensed` in the legacy API layer (`frontend/src-tauri/src/api/api.rs#L196-L205`), which still
targets the archived FastAPI at `http://localhost:5167` (`frontend/src-tauri/src/api/api.rs#L20-L20`). No
license check gates any feature in the community code (searched `is_licensed|licensing` in `src-tauri/src`:
only struct fields) — the paid product is a separate codebase.

**PRO vs community.** README: PRO is "built on a different codebase with superior transcription models"
(`README.md#L212-L222`); held back: enhanced accuracy models, custom summary templates (note: the community
build *does* load user JSON templates, A4), PDF/DOCX/Markdown formatted exports, **auto-detect and join
meetings**, speaker identification (coming soon), chat with meetings (coming soon), calendar (coming soon),
self-hosted team deployment, GDPR audit trails, priority support (`README.md#L226-L240`); a 20 %-off coupon
`LAUNCH20` (`README.md#L222-L224`, `README.md#L47-L47`). The live-path VAD constant is documented as matching "the
established Meetily Pro live policy" (`frontend/src-tauri/src/audio/pipeline.rs#L19-L19`) — the two codebases
share tuning.

**Secrets at rest.** API keys sit in plain SQLite columns (A6); Not found: OS keychain use (searched
`keyring|keychain`).

**Licence.** MIT (`LICENSE.md`; GitHub API `license.spdx_id = MIT`).

## A9 Engineering, platforms and traction

**Architecture.** One Tauri 2 app: Rust core (`frontend/src-tauri/src/lib.rs`, 868 lines, command registry at
`#L700-L800`) + Next.js UI (`frontend/src/`), a separate `llama-helper` binary for local LLMs
(`llama-helper/src/main.rs`), a workspace `Cargo.toml` at the root; the Python backend is archived
(`CLAUDE.md#L10-L17`). The tree carries a lot of dead weight: `lib_old_complex.rs` (2,437 lines),
`audio/core-old.rs`, `recording_commands.rs.backup` (2,068 lines), `recording_saver_old.rs`, an `audio_v2/`
module with its own mixer/limiter/normaliser, the uncompiled `audio/stt.rs` (A3) and a `CLEANUP_PLAN.md`
(`frontend/src-tauri/CLEANUP_PLAN.md`). Several comments contradict the code (600 ms vs "50 ms" window,
`sys × 1.0` vs "70 %", hard clip vs "proportional", `NUM_WORKERS = 1` vs "parallel"; A1/A2).

**Platforms.** macOS (Core Audio tap, Metal + CoreML whisper features enabled by default on macOS,
`frontend/src-tauri/Cargo.toml#L38-L53`, `#L177-L179`), Windows (WASAPI loopback, Vulkan whisper build, AVX2,
`README.md#L111-L111`), Linux build-from-source (PulseAudio monitor sources, `README.md#L197-L200`). Whisper
backends are cargo features (metal, coreml, cuda, vulkan, hipblas, openblas, openmp,
`frontend/src-tauri/Cargo.toml#L38-L53`); GPU auto-detect scripts pick features at dev/build time
(`frontend/scripts/auto-detect-gpu.js`, `frontend/scripts/tauri-auto.js`).

**Tests and evals.** Rust unit tests inside modules (VAD timestamp/flush invariants,
`frontend/src-tauri/src/audio/vad.rs#L500-L660`; resumable-download edge cases,
`frontend/src-tauri/src/parakeet_engine/parakeet_engine.rs#L1240-L1900`; whisper model validation,
`frontend/src-tauri/src/whisper_engine/whisper_engine.rs#L1290-L1470`; chunking progress,
`frontend/src-tauri/src/summary/processor.rs#L667-L667`; summary backup restore,
`frontend/src-tauri/src/database/repositories/summary.rs#L223-L250`), one audio fixture
(`frontend/src-tauri/tests/fixtures/he_aac_48k_5s.m4a`), and ten frontend tests (`frontend/tests/hooks/*`,
`frontend/tests/lib/*`: summary generation, paginated transcripts, BlockNote markdown, onboarding summary
model, language preferences, model readiness, analytics). A `scripts/inject_transcript.py` helper exists for
feeding transcripts. Not found: transcript WER or summary-quality evals (searched `wer|rtf|benchmark|eval`).

**CI / release / updater.** Eleven workflow files: per-OS builds, a dev-test build, a PR check and a release
workflow (`.github/workflows/*.yml`, overview in `.github/workflows/WORKFLOWS_OVERVIEW.md`), plus a guard that
GGML is built portable (`.github/force-portable-ggml.cmake`, `.github/verify-portable-ggml.cjs`). The Tauri
updater is signed (minisign pubkey) and polls `releases/latest/download/latest.json` on the old repo name
`meeting-minutes` (`frontend/src-tauri/tauri.conf.json#L91-L117`); manifest generator in
`scripts/generate-update-manifest-github.js`.

**Diagnostics/logging.** An async logger and batched audio metrics to keep logging off the audio thread
(`frontend/src-tauri/src/audio/async_logger.rs`, `frontend/src-tauri/src/audio/batch_processor.rs`,
`frontend/src-tauri/LOGGING_OPTIMIZATIONS.md`); periodic RMS / buffer-health logs in the pipeline
(`frontend/src-tauri/src/audio/pipeline.rs#L62-L71`, `#L489-L521`); whisper.cpp/Metal stderr suppression
(`frontend/src-tauri/src/whisper_engine/whisper_engine.rs#L118-L121`).

**Traction (GitHub API via the GitHub MCP, 2026-10-10 ~12:25).** 31,576 stars, 3,454 forks, 361 open
issues+PRs, created 2024-12-26, last push 2026-09-15; topics include `parakeet`, `sortformer`, `whisper-cpp`.
Description: "Privacy first, AI meeting assistant with 4x faster Parakeet/Whisper live transcription, speaker
diarization, and Ollama summarization built on Rust …" — so the "4x faster" claim lives in the repo
description, not in the code. Releases (12 in total): v0.0.1 2025-02-01 … v0.0.5 2025-08-12, 0.1.1 2025-10-23,
v0.2.0 2025-12-31, v0.2.1 2026-02-11, v0.3.0 2026-03-03, v0.4.0 2026-06-05, **v0.4.1 2026-09-12** — roughly one
release per 1–3 months. Issue search by reactions was rate-limited (HTTP 403 on both the REST search and the MCP
search); fallback = issues ordered by comment count: #110 Windows CMake backend error (22, closed), #254 macOS
app does not request microphone permission (17, closed), **#230 Speaker Diarization / Identification (16,
open)**, #95 Ollama summary error (15, closed), #228 application crash (13, open), #233 multi-language
summaries (12, closed), #148 brew SHA-256 mismatch (12, closed), **#456 local Whisper not using GPU (11,
open)**, **#387 automatic meeting detection and recording (11, open)**, #25 no mic input / long conversations
fail (11, open), #108 access to saved audio (9, open).

## Map to the three questions

(theuth's side is taken from the AXES context paragraph; the OURS analyst verifies it.)

### Q1 What they offer that theuth does not — and the reverse

Meetily has, theuth (per AXES) does not mention:
- Summary **templates per meeting type** as user-droppable JSON (six bundled incl. standup, retro, sales call;
  `frontend/src-tauri/src/summary/templates/loader.rs#L8-L70`, `frontend/src-tauri/templates/standard_meeting.json#L1-L27`).
- **Provider choice for summaries** incl. Ollama with context-sized chunking and a **bundled llama.cpp sidecar**
  with a model catalogue (`frontend/src-tauri/src/summary/llm_client.rs#L215-L223`,
  `frontend/src-tauri/src/summary/summary_engine/models.rs#L131-L215`).
- **Summary language** independent of transcript language (`frontend/src-tauri/src/summary/processor.rs#L517-L546`).
- **Import an audio file / re-transcribe** a recording with another model or language
  (`frontend/src-tauri/src/audio/retranscription.rs#L19-L53`, `README.md#L146-L150`).
- **Interrupted-recording recovery** UI (`frontend/src/hooks/useTranscriptRecovery.ts#L39-L143`).
- **Tray pause/resume** recording (`frontend/src-tauri/src/tray.rs#L37-L51`).
- Engine choice Whisper vs Parakeet with in-app model manager and resumable downloads (A2).

theuth has, Meetily does not: per-source tracks (Meetily mixes before STT, `frontend/src-tauri/src/audio/pipeline.rs#L848-L903`);
diarization + correction + enrollment (A3: none); live topics/briefs/island/steering (A5: none); capture hiding
(A5: none); meeting chat, `meeting_read/search/speakers` tools, MCP (A6: none — search is SQL `LIKE`,
`frontend/src-tauri/src/database/repositories/transcript.rs#L87-L101`); knowledge harvest and a brief cache.

### Q2 What we can learn — techniques, cite, where it lands in theuth

| # | Technique | Cite | Lands in theuth |
|---|---|---|---|
| 1 | Checkpoint journal + recover-on-launch flow (audio + transcript) | `frontend/src-tauri/src/audio/incremental_saver.rs#L241-L342` | crates/meeting recorder + desktop meeting page start-up check |
| 2 | Separate live/batch VAD redemption; measure segments per meeting and median length | `frontend/src-tauri/src/audio/pipeline.rs#L738-L757` | crates/meeting live STT vs post-meeting pass |
| 3 | Repetition cleaner + Whisper no-speech/entropy/logprob thresholds | `frontend/src-tauri/src/whisper_engine/whisper_engine.rs#L401-L603` | any Whisper path in theuth's STT; also dictation |
| 4 | Context-sized map-reduce with per-chunk retry and fail-loud | `frontend/src-tauri/src/summary/processor.rs#L400-L490` | meeting summary for long meetings on small/local models |
| 5 | Template = sections {title, instruction, format, item_format}; prompt fills a skeleton | `frontend/src-tauri/src/summary/templates/types.rs#L5-L110` | meeting note generation; also notebooks/briefs |
| 6 | Prompt-injection guard on transcript text + "None noted" rule | `frontend/src-tauri/src/summary/processor.rs#L229-L253` | every post-meeting LLM call over transcript text |
| 7 | English-first then translate, cache keyed by fingerprints | `frontend/src-tauri/src/summary/service.rs#L85-L137` | meeting summary language setting |
| 8 | Backup/restore on regenerate | `frontend/src-tauri/src/database/repositories/summary.rs#L88-L210` | meeting note regenerate (block doc revisions) |
| 9 | Exact-size resumable downloads with Content-Range validation | `frontend/src-tauri/src/parakeet_engine/parakeet_engine.rs#L695-L810` | theuth model fetcher (STT/diarization ONNX models) |
| 10 | BT-aware disconnect thresholds, bounded default-mic fallback, AirPods mic+speaker coupling | `frontend/src-tauri/src/audio/device_monitor.rs#L62-L70`, `#L201-L210` | capture layer device-change handling |
| 11 | Output-running process listing + debounced listener → "meeting started?" prompt | `frontend/src-tauri/src/audio/system_detector.rs#L104-L127`, `#L358-L380` | Swift companion or crates/meeting detection |
| 12 | Hardware tier → beam/threads/temperature | `frontend/src-tauri/src/audio/hardware_detector.rs#L204-L240` | STT config defaults per machine |
| 13 | Action-item table with a transcript-reference + timestamp column | `frontend/src-tauri/templates/standard_meeting.json#L15-L20` | meeting note → link back to segment playback |

Avoid: mixing sources before STT (loses channel identity); hard-clip "soft scaling"
(`frontend/src-tauri/src/audio/pipeline.rs#L190-L196`); plain-text API keys in SQLite; an analytics key in source
behind a UI-only gate (`frontend/src-tauri/src/analytics/commands.rs#L12-L14`).

### Q3 How to make theuth better — 3–5 highest-leverage changes, in order

1. **Interrupted-meeting recovery as a product feature**: on launch, find meetings without a clean stop, show a
   preview, finalise tracks and transcript, resume post-processing (their durability idea; crash reports are
   among their most-discussed open issues, #228). Land: crates/meeting + desktop meeting page.
2. **STT segment policy + hallucination guards, with a measurement**: count requests and median clip length
   per meeting, split live vs final-pass redemption, add the repetition cleaner (A2). Cheap, measurable, and it
   improves every downstream topic, brief and summary.
3. **Templated, long-meeting-safe summary**: section templates (bundled + user), context-sized map-reduce for
   local models, injection guard, a summary-language setting with a translation cache, backup on regenerate (A4).
4. **Device-change resilience** in capture (BT thresholds, bounded fallback, mic+speaker coupling) (A1).
5. **Meeting-detection prompt** from "a process is playing audio" + mic-in-use, debounced — their most-requested
   open feature (#387) and PRO-only for them (`README.md#L226-L240`).

## Cites self-check

Re-opened 44 cites in one pass (each checked for the quoted constant/keyword inside the cited range):

- `frontend/src-tauri/src/audio/pipeline.rs#L38-L60` — held (`window_ms = 600.0`)
- `frontend/src-tauri/src/audio/pipeline.rs#L158-L203` — held (`sys * 1.0`)
- `frontend/src-tauri/src/audio/pipeline.rs#L158-L203` — held (`sum / sum_abs`)
- `frontend/src-tauri/src/audio/ffmpeg_mixer.rs#L20-L20` — held (`RNNOISE_APPLY_ENABLED: bool = false`)
- `frontend/src-tauri/src/audio/audio_processing.rs#L143-L218` — held (`TARGET_LUFS: f64 = -23.0`)
- `frontend/src-tauri/src/audio/capture/core_audio.rs#L85-L139` — held (`with_mono_global_tap_excluding_processes`)
- `frontend/src-tauri/src/audio/capture/core_audio.rs#L85-L139` — held (`meetily-audio-tap`)
- `frontend/src-tauri/src/audio/incremental_saver.rs#L18-L44` — held (`sample_rate as usize * 30`)
- `frontend/src-tauri/src/audio/incremental_saver.rs#L114-L198` — held (`"-c", "copy"`)
- `frontend/src-tauri/src/audio/device_monitor.rs#L62-L70` — held (`3 // 3 polling cycles`)
- `frontend/src-tauri/src/audio/recording_commands.rs#L1286-L1286` — held (`MAX_MIC_FALLBACK_ATTEMPTS: u32 = 3`)
- `frontend/src-tauri/src/audio/transcription/worker.rs#L50-L87` — held (`NUM_WORKERS: usize = 1`)
- `frontend/src-tauri/src/audio/transcription/worker.rs#L161-L246` — held (`unwrap_or(0.85)`)
- `frontend/src-tauri/src/parakeet_engine/parakeet_engine.rs#L120-L140` — held (`towardsgeneralintelligence`)
- `frontend/src-tauri/src/parakeet_engine/model.rs#L95-L125` — held (`CPUExecutionProvider`)
- `frontend/src-tauri/src/whisper_engine/whisper_engine.rs#L561-L603` — held (`set_no_speech_thold(0.55)`)
- `frontend/src-tauri/src/whisper_engine/whisper_engine.rs#L401-L560` — held (`> 0.7`)
- `frontend/src-tauri/src/summary/processor.rs#L400-L403` — held (`token_threshold - 300, 100`)
- `frontend/src-tauri/src/summary/service.rs#L432-L476` — held (`100000`)
- `frontend/src-tauri/src/summary/processor.rs#L229-L253` — held (`None noted in this section`)
- `frontend/src-tauri/src/database/repositories/summary.rs#L88-L210` — held (`COALESCE(result_backup, result)`)
- `frontend/src-tauri/src/analytics/commands.rs#L12-L14` — held (`phc_`)
- `frontend/src/components/AnalyticsProvider.tsx#L24-L48` — held (`store.set('analyticsOptedIn', false)`)
- `frontend/src-tauri/src/audio/vad.rs#L45-L61` — held (`min_speech_time = Duration::from_millis(250)`)
- `frontend/src-tauri/src/audio/pipeline.rs#L738-L759` — held (`322 requests`)
- `frontend/src-tauri/src/summary/summary_engine/models.rs#L131-L215` — held (`Qwen3.5-2B-Q4_K_M.gguf`)
- `frontend/src-tauri/src/summary/summary_engine/sidecar.rs#L262-L300` — held (`Command::new("nice")`)
- `frontend/src-tauri/src/database/repositories/transcript.rs#L87-L101` — held (`LIKE ?`)
- `README.md#L226-L234` — held (`Speaker diarization is planned`)
- `frontend/src-tauri/tauri.conf.json#L114-L117` — held (`latest.json`)
- `frontend/src-tauri/migrations/20251110000000_add_grace_period_to_licensing.sql#L1-L8` — held (`604800`)
- `frontend/src-tauri/src/audio/hardware_detector.rs#L204-L240` — held (`beam_size: 5`)
- `frontend/src-tauri/src/parakeet_engine/parakeet_engine.rs#L695-L810` — held (`bytes={range_start}-`)
- `frontend/src-tauri/src/audio/system_detector.rs#L104-L127` — held (`from_millis(500)`)
- `frontend/src-tauri/src/audio/recording_preferences.rs#L29-L72` — held (`meetily-recordings`)
- `frontend/src-tauri/src/audio/stt.rs#L1-L8` — held (`pyannote`)
- `frontend/src-tauri/src/audio/transcription/engine.rs#L15-L46` — held (`Provider(Arc<dyn TranscriptionProvider>)`)
- `frontend/src-tauri/src/summary/processor.rs#L79-L80` — held (`regardless of transcript language`)
- `frontend/src-tauri/src/summary/service.rs#L85-L137` — held (`FNV`)
- `frontend/src-tauri/src/summary/llm_client.rs#L215-L223` — held (`BuiltInAI`)
- `frontend/src-tauri/src/audio/devices/platform/windows.rs#L188-L200` — held (`loopback`)
- `frontend/src-tauri/src/audio/pipeline.rs#L19-L19` — held (`Meetily Pro live policy`)
- `frontend/src-tauri/src/audio/transcription/worker.rs#L27-L35` — held (`no confidence gating`)
- `frontend/src-tauri/src/parakeet_engine/model.rs#L17-L18` — held (`MAX_TOKENS_PER_STEP: usize = 3`)

Corrected: none after re-open. Soft spots: whole-function cites (e.g. `frontend/src-tauri/src/summary/service.rs#L322-L340`)
span the signature rather than every statement; GitHub reaction ranking could not be fetched (HTTP 403 rate limit on
REST and MCP search) — comment-count ordering used instead (A9).
