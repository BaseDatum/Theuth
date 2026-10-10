# OURS — theuth meeting stack inventory against AXES (main 2a1238f2, 2026-10-10)

Analyst: recon-ours (read-only). Tree: /Users/Sky.Moore/Develop/theuth on main 2a1238f2. All cites repo-relative.

## Executive summary

theuth's meeting stack is a live *agent* bolted to a local recorder: strongest where the AI works during the call and
where a meeting becomes part of the harness (session, tools, knowledge, notebooks); weakest at the edges a dedicated
note-taker polishes (auto-start, calendar, templates, languages, export, MCP for other agents). Cross-platform capture
is real on main (the 2026-09-21 "macOS-only" docs are stale), but only macOS has AEC.

### 10 distinctive strengths

1. A tool-using agent during the call — knowledge first, then web — writes 2–3 factual bullets, "never what to say" (`crates/meeting/src/tools.rs#L26-L57`).
2. Cross-meeting brief cache with spoken aliases; a no-model alias scan on every utterance puts a known subject up instantly (`crates/meeting/src/cache.rs#L1-L5`).
3. A local decision-oracle gate (mentions + small-talk classifier + cached-brief choice) before any model turn, learning from taps/dismissals (`crates/meeting/src/oracle.rs#L1-L20`).
4. Steering: push-to-talk spoken asks, typed requests, topic taps and deeper dives into the same session (`crates/meeting/src/lib.rs#L799-L805`, `crates/meeting/src/stt.rs#L48-L56`).
5. A non-activating island (webview + native Swift and C#) hidden from screen shares on macOS and Windows (`crates/desktop/src/island/mod.rs#L88-L92`).
6. Live diarization with Nemotron Streaming Sortformer at RTF 0.035, labels reconciled into the post pass (`crates/meeting/src/post/diarize.rs#L112-L120`, `crates/meeting/src/post/mod.rs#L206-L216`).
7. Two-pass local STT: streaming Nemotron at 560 ms live, VAD-gated Parakeet TDT with word times for the record (`crates/voice/src/stt.rs#L1-L7`, `crates/meeting/src/post/transcribe.rs#L1-L7`).
8. Three Opus tracks (mic / system / mix), AEC'd mic on macOS, per-app or system loopback on macOS/Windows/Linux with a stated `Support` level (`crates/meeting/src/capture/encode.rs#L1-L5`, `crates/meeting/src/capture/system.rs#L106-L116`).
9. Notes-first "enhance": the user's live note is a real block document; the summary turn regroups, never rewrites, with before/after snapshots (`crates/meeting/src/note.rs#L16-L35`).
10. A meeting is an agent session: follow-up chat from the summary head, `meeting_read/search/speakers/list` in every session, automatic knowledge harvest, notebook sources (`crates/meeting/src/summary.rs#L159-L181`, `crates/document/src/ingest/mod.rs#L47-L49`).

### 10 biggest gaps

1. Manual start only — no meeting auto-detection, no calendar trigger (only "calendar" hit is the alias scanner's word list, `crates/meeting/src/oracle.rs#L382`).
2. No MCP server for external agents: `t mcp` is a 3-tool shim for theuth's own ACP children (`crates/theuth/src/mcp.rs#L1-L6`).
3. No calendar integration: no attendees, auto-titles or pre-meeting context beyond a typed box (`crates/meeting/src/lib.rs#L633-L635`).
4. No templates per meeting type; one hard-coded summary form, no user prompt (`crates/meeting/src/note.rs#L23-L34`).
5. No local-only mode or disclosure: transcript text goes to the picked model, web search on by default (`crates/meeting/src/lib.rs#L593-L606`, `crates/meeting/src/lib.rs#L639-L641`).
6. Live STT is English-only and meetings use no custom vocabulary (dictionary exists for dictation only) (`crates/voice/src/lib.rs#L64`, `crates/voice/src/dictionary/mod.rs#L1`).
7. No recovery of a recording interrupted by an engine crash — restart resumes only post/summary states (`crates/meeting/src/lib.rs#L3781-L3790`).
8. No AEC on Windows/Linux and no device follow mid-meeting (AirPods, docks) (`crates/meeting/src/capture/system.rs#L22-L23`, `crates/meeting/src/capture/system.rs#L106-L116`).
9. No evals or measured latency: no WER, brief- or summary-quality checks; the e2e pass is a manual list (`docs/design/meetings/README.md#L127-L182`).
10. JSON + SQLite storage with FTS-only search — no markdown/YAML vault, semantic search only via notebooks (`crates/meeting/migrations/0002_segments.sql#L1-L10`, `crates/meeting/src/search.rs#L1-L3`).

## A1 Capture and meeting detection

**(a) What exists — the path through the code**

- **Start is manual only.** The desktop start form is the one entry: source picker (System audio by default, or one app,
  apps playing audio first), title, a dictated context box, the consent checkbox, the session's model, a permissions row;
  `Start` → `meeting_start` → the control center (`crates/desktop/ui/src/components/meeting/StartForm.tsx#L1-L4`). The
  consent text is a constant, verbatim from 53 decision 14 (`crates/desktop/ui/src/components/meeting/StartForm.tsx#L32-L33`);
  `startOptions` sends `process: null` for System audio, `title`, `context`, `consent`, `model`, `endless`
  (`crates/desktop/ui/src/components/meeting/StartForm.tsx#L36-L41`). The actor's command set is
  `ListProcesses / StartMeeting / PauseMeeting / ResumeMeeting / StopMeeting / …` (`crates/meeting/src/lib.rs#L10-L15`).
- **What is captured:** two inputs, the mic and the meeting app's (or the whole system's) output. System side runs
  through `flexaudio` on its own thread: `SourceKind::ProcessLoopback` + `target_pid`, or `SystemLoopback` for pid 0 —
  a Core Audio process tap on macOS, WASAPI (process) loopback on Windows, PipeWire on Linux
  (`crates/meeting/src/capture/system.rs#L1-L7`). The global tap excludes theuth's own process (`exclude_self: true`)
  and follows the *default* output device only — an app routed to a non-default headset is silent to it, flagged as a
  known limitation in code (`crates/meeting/src/capture/system.rs#L16-L23`, `crates/meeting/src/capture/system.rs#L35-L55`).
  `SYSTEM_AUDIO` is pid 0 (`crates/meeting/src/lib.rs#L247`).
- **Echo cancellation:** on macOS the mic opens Apple's VoiceProcessingIO unit (`Aec::Vpio`); if it cannot open, or off
  macOS, the plain input is used and `Degradation::MicNoAec` is raised (`crates/meeting/src/capture/mic.rs#L38-L42`,
  `crates/meeting/src/lib.rs#L184-L190`). The crate doc states the per-OS matrix: Core Audio taps + VPIO on macOS, WASAPI
  loopback on Windows, PipeWire on Linux, no AEC off macOS, "headphones recommended" (`crates/meeting/src/lib.rs#L1-L8`).
- **Platform support is stated to the UI**, not assumed: `Support::{Full, Partial(reason), None(reason)}`
  (`crates/meeting/src/lib.rs#L208-L219`); macOS returns `Full`, Windows/Linux `Partial("no echo cancellation;
  headphones recommended")` when the loopback backend answers, else `None(why)` (`crates/meeting/src/capture/system.rs#L106-L116`).
  Status carries `supported` and the ONNX execution provider (`DmlExecutionProvider | CPUExecutionProvider`)
  (`crates/meeting/src/lib.rs#L706-L711`). **Verdict on the 2026-09-21 untracked docs (58 / recon-desktop-meetings):
  "meetings are macOS-only" is no longer true on main** — capture is compiled per OS (`#[cfg(target_os = "windows")]`
  at `crates/meeting/src/capture/system.rs#L239`, `#[cfg(target_os = "linux")]` + PipeWire registry at
  `crates/meeting/src/capture/system.rs#L343-L358`).
- **Process picker:** flexaudio has no enumeration, so the list is per OS — `kAudioHardwarePropertyProcessObjectList`,
  the default render endpoint's audio sessions, the PipeWire registry (`crates/meeting/src/capture/system.rs#L5-L7`);
  sorted active-first, own pid skipped (`crates/meeting/src/capture/system.rs#L118-L125`).
- **Format and storage:** capture at 48 kHz (`crates/meeting/src/capture/mod.rs#L26`); the encoder resamples to 24 kHz
  once, mixes with a soft limiter, encodes three Opus streams and writes Ogg pages, one page per second, RMS at 10 Hz
  (`crates/meeting/src/capture/encode.rs#L1-L5`; `crates/meeting/src/ogg.rs#L10-L14`). Opus `Application::Voip` at
  32 kbps (`crates/meeting/src/capture/encode.rs#L103-L106`). Files `mic.opus`, `system.opus`, `mix.opus`
  (`crates/meeting/src/store.rs#L42-L57`; opened at `crates/meeting/src/capture/encode.rs#L170-L172`). Layout
  `<data_dir>/meetings/<id>/` with `live.jsonl` and `briefs.jsonl` (`crates/meeting/src/lib.rs#L71-L77`); an index
  `meetings.sqlite` (WAL, migrations) rebuilt from the manifests when missing — "the directory is truth"
  (`crates/meeting/src/store.rs#L238-L265`, `crates/meeting/src/store.rs#L458-L479`). Manifest writes are atomic
  (`.tmp` + rename) (`crates/meeting/src/store.rs#L285-L293`).
- **Mixing/ducking/normalisation:** the mix has a soft limiter (`crates/meeting/src/capture/encode.rs#L4`); nothing
  else (no loudness normalisation or ducking found in `capture/`).
- **Permissions (TCC):** two services, `Microphone` and `AudioCapture` ("System Audio Recording Only")
  (`crates/meeting/src/lib.rs#L221-L229`); mic state read without prompting via `AVCaptureDevice`, non-macOS returns
  `Granted` (`crates/meeting/src/capture/mic.rs#L76-L100`); the tap's TCC prompt fires on first use at open
  (`crates/meeting/src/capture/system.rs#L16-L19`); a denial is `Degradation::PermissionDenied`
  (`crates/meeting/src/lib.rs#L199-L200`). The start form deep-links to System Settings › Privacy
  (`crates/desktop/ui/src/components/meeting/StartForm.tsx#L31`).
- **Guards while recording:** `max_minutes` default 180 (`crates/meeting/src/lib.rs#L584`), `NearMaxLength` ten minutes
  before (`crates/meeting/src/lib.rs#L197-L198`), `DiskLow` under 500 MB (`crates/meeting/src/lib.rs#L81`,
  `crates/meeting/src/lib.rs#L195-L196`). Recording outlives intelligence: STT/model/diariser failures are
  degradations, not stops (`crates/meeting/src/lib.rs#L184-L206`).
- **Device loss mid-meeting:** the tap thread returns on `PermissionDenied | DeviceLost | Error`, and the encoder keeps
  writing silence for that track (`crates/meeting/src/capture/system.rs#L19-L21`, `crates/meeting/src/capture/system.rs#L64-L73`);
  the mic pump exits on a device error (`crates/meeting/src/capture/mic.rs#L53-L74`).
- **Crash/restart:** on plugin start, manifests in `Post(_)`, `Summarizing`, or `Stopping` with `ended_at` are resumed
  or re-queued (`crates/meeting/src/lib.rs#L3781-L3790`); a summary form is re-sent after a restart
  (`crates/meeting/src/lib.rs#L3704-L3708`). Audio on disk is durable to ~1 s (one Ogg page per second,
  `crates/meeting/src/capture/encode.rs#L4-L5`).

**(b) Missing relative to the axis**

- **No auto-detection** of a meeting (mic-in-use, meeting-app window, call start) and **no calendar trigger**: grep for
  `auto.?start|auto.?detect|calendar` in `crates/meeting/src` finds only the alias scanner's month/day word list
  (`crates/meeting/src/oracle.rs#L382`); no EventKit/Google/Outlook calendar code anywhere in `crates/` (searched
  `calendar|EventKit|google.?calendar|outlook.?calendar`).
- **No recovery of a recording interrupted by an engine crash:** the restart filter skips `Recording`/`Paused`/`Starting`
  manifests (`crates/meeting/src/lib.rs#L3786-L3789`) — such a meeting is left in its last state with audio up to the
  crash, no "recover and post-process" path found (searched `recover|orphan|interrupted|crash` in `crates/meeting/src`).
- **No mid-meeting device follow:** a lost device ends that track's capture (silence thereafter), no re-open on the new
  default (AirPods/dock switch) (`crates/meeting/src/capture/system.rs#L64-L73`, `crates/meeting/src/capture/mic.rs#L65-L68`);
  the global tap pins the default device at open (`crates/meeting/src/capture/system.rs#L22-L23`).
- **No AEC on Windows/Linux** (`crates/meeting/src/capture/system.rs#L106-L116`); no software AEC fallback found (grep
  `webrtc|speexdsp|apm::|aec3` in `crates/voice`, `crates/meeting`, `Cargo.toml`: no matches).
- **No retention policy**: nothing prunes meetings (grep `retention|prune|max_age` in `crates/meeting/src` hits only
  in-memory buffers); meetings live until deleted (Sky's decision in `projects/wth-meeting-assistant`).
- No per-app + mic mix "ducking", no loudness normalisation.

**(c) Designs**

- `docs/design/meetings/01-capture.md` (phase 1: VPIO mic + flexaudio tap, three Opus tracks, lifecycle) — built.
- `docs/design/58-cross-platform.md` §3.3/§6.1 (Windows WASAPI, Linux PipeWire, `supported()`) — built for capture
  (cited by `crates/meeting/src/lib.rs#L5-L8`), though the 58 doc itself is untracked on main.
- Auto-start/calendar: explicitly out of scope v1 ("purely manual start"; "calendar briefs" out) in Sky's 2026-09-19
  decisions (`projects/wth-meeting-assistant`) — designed-out, not designed.

## A2 Transcription

**(a) What exists**

- **Two models, two passes.** Live = the cache-aware **Nemotron streaming transducer** (`nemotron-speech-streaming-en-0.6b`
  int8), offline = **Parakeet TDT 0.6B v3** (int8 smoothquant) via `parakeet-rs` (`crates/voice/src/stt.rs#L1-L7`;
  model origins `scripts/fetch-models.sh#L75-L77`; required files `scripts/fetch-models.sh#L48-L56`). Default dirs
  `nemotron-streaming-en` and `parakeet-tdt` (`crates/voice/src/lib.rs#L64-L68`); the meeting's live dir is configurable
  (`crates/meeting/src/lib.rs#L546-L548`).
- **Live path:** one blocking task per track drains its STT ring into a `voice::live::Utterer` and reports text pieces
  with receipt instants mapped onto the recording clock; a model failure ends the task (`SttDown`) and recording
  continues (`crates/meeting/src/stt.rs#L1-L5`). Two Utterers are opened — one per track, "accuracy over compute"
  (`crates/meeting/src/lib.rs#L2059-L2064`). The `Utterer` owns a streaming Nemotron session, a VAD, the utterance gate
  and the resampler and emits rolling partials then a final (`crates/voice/src/live.rs#L1-L5`, `crates/voice/src/live.rs#L20-L32`).
  Streaming window **560 ms** (8,960 samples at 16 kHz = 56 mel frames × 160 hop) (`crates/voice/src/stt.rs#L34-L36`).
- **VAD/gate constants:** 256-sample frames at 16 kHz (16 ms) (`crates/voice/src/vad.rs#L16-L19`); pre-roll 20 frames,
  onset 10, close 19, **max open utterance 15 s**, turn 2 s, threshold 0.5 (`crates/voice/src/vad.rs#L108-L118`).
- **Post pass (transcript of record):** stage 1 decodes each Opus track (48 kHz) → 16 kHz → the live VAD finds
  utterances (≤ 15 s) → `Engine::transcribe(words)` per utterance with padding, word times shifted and clamped; silence is
  never handed to the model because a whole track with silences made TDT drop utterances and stretch word ends
  (`crates/meeting/src/post/transcribe.rs#L1-L7`); 1 s overlap constant (`crates/meeting/src/post/transcribe.rs#L16-L19`).
  TDT's sequence limit handled with 180 s pieces (`crates/voice/src/stt.rs#L29-L31`). The pipeline loads the TDT engine,
  transcribes mic then system, reports progress 0–60 % (`crates/meeting/src/post/mod.rs#L139-L169`).
- **Hallucination guards:** live — a closed utterance must contain a letter or digit ("a lone `.` from a breath or the
  far end's echo is not a line") (`crates/meeting/src/lib.rs#L1732-L1736`); post — VAD-gated utterances only
  (`crates/meeting/src/post/transcribe.rs#L5-L7`). No repetition/loop guard is needed for transducers (none found).
- **Hardware acceleration:** parakeet-rs defaults (4 intra threads, CPU) plus the execution provider
  `models.accelerator` resolves to (`crates/voice/src/stt.rs#L163-L178`); the meeting status reports the diarisation EP
  (DirectML or CPU) (`crates/meeting/src/lib.rs#L708-L711`). A WebGPU concurrency test exists
  (`crates/meeting/tests/webgpu_concurrency.rs`).
- **Model catalogue/download/verify:** `scripts/fetch-models.sh` REQUIRED list + `origin_of` per file
  (`scripts/fetch-models.sh#L31-L32`, `scripts/fetch-models.sh#L48-L56`, `scripts/fetch-models.sh#L75-L77`); parakeet-tdt
  has no recorded hub id (`scripts/fetch-models.sh#L75`). The desktop downloads what `models.json` lists (knowledge
  `facts/theuth-models-publisher-release-models-sh`).
- **Local is enforced by construction:** both STT paths are ONNX models loaded from `<data_dir>/models`; no cloud STT
  client exists in `crates/voice/src/stt.rs` (imports at `crates/voice/src/stt.rs#L14-L23` are parakeet-rs/symphonia only).

**(b) Missing**

- **Language:** the live model is English-only by name and file (`nemotron-streaming-en`, `crates/voice/src/lib.rs#L64`);
  no language detection or language setting in the meeting crate (grep `language|multilingual` in `crates/meeting/src`:
  no matches). Parakeet TDT v3 is multilingual per its public model card (not verified in code), so the post pass may handle
  other languages, but the live transcript, topics and briefs would not.
- **Custom vocabulary / hotwords:** a dictionary exists for dictation (`crates/voice/src/dictionary/mod.rs#L1`, 153 §5),
  but the meeting crate never uses it (grep `dictionary` in `crates/meeting/src`: no matches). No contextual biasing of
  either model.
- **Measured latency/RTF/WER:** none in code or tests; 53's final decision was "no benchmarking or perf checks"
  (`projects/wth-meeting-assistant`). Only anecdotal numbers exist in knowledge (dictation final ≈0.1 s on the Mac,
  2.4–3.2 s on build-B — `facts/theuth-dictation-engine-drops-tail-before-dictation-stop-build-b-2026-10-09`).
- **Model choice/tiers in the UI:** one checkpoint each; smaller/larger tiers were surveyed (`facts/theuth-stt-model-tiers-recon-2026-09-29`)
  but no switch exists in `MeetingConfig` (`crates/meeting/src/lib.rs#L529-L566`).
- No Whisper/whisper.cpp, Moonshine, Apple SpeechAnalyzer or cloud engine (Sky ruled "Parakeet everywhere").

**(c) Designs**

- `docs/design/meetings/02-live.md` (live STT, two tasks, `voice::stt::Engine` extracted) — built.
- `docs/design/meetings/04-post.md` stage 1 (re-transcribe with word times) — built; deviation 13 (VAD-gated) recorded in
  `crates/meeting/src/post/transcribe.rs#L5-L7`.
- `docs/design/153-voice-anywhere.md` §5 dictionary — built for dictation only; meetings not covered.

## A3 Speakers

**(a) What exists**

- **Me-vs-them from channels:** the mic track is always `me`; a system line is the live label's display name
  (`Speaker 2`) when the diariser labelled it, else `them` (`crates/meeting/src/lib.rs#L515-L524`); the live line
  carries `speaker: Option<String>` (`s1`..`s8`) for system utterances only (`crates/meeting/src/lib.rs#L509-L512`).
  The post pass keeps a fixed `ME` label first in the speaker list (`crates/meeting/src/post/mod.rs#L234-L239`).
- **Live diarization (design 07, as built):** one blocking task per meeting drains the system feed's diariser ring into a
  `Streamer` in the `LIVE` geometry and tells the actor speaker spans for the last `KEEP_MS` (120 s); the actor holds
  system utterances back until labelled; pause keeps the speaker cache; stop scores the tail with no look-ahead
  (`crates/meeting/src/diar.rs#L1-L9`, `crates/meeting/src/diar.rs#L36-L38`). An utterance's speaker must cover
  ≥ 30 % of it, VAD pre-roll (320 ms) excluded (`crates/meeting/src/diar.rs#L39-L43`). **Model: NVIDIA Nemotron 3
  Diarization (Streaming Sortformer, ≤ 8 speakers, arrival-ordered)** — `LIVE` = 6.4 s chunk + 1.6 s look-ahead (8 s
  input buffer), **RTF 0.035 on CPU, 97–99 % frame agreement with OFFLINE** (`crates/meeting/src/post/diarize.rs#L112-L120`).
  Switch `meeting.live_diarization` (default true, read at start) (`crates/meeting/src/lib.rs#L556-L558`,
  `crates/meeting/src/lib.rs#L592`). A failure is `Degradation::DiarizationDown` → `them:` lines thereafter
  (`crates/meeting/src/lib.rs#L203-L205`).
- **Post diarization:** stage 2 (60–90 %) runs `diarize::run` over the whole system track and computes a mic
  embedding for `me` (`crates/meeting/src/post/mod.rs#L184-L204`); `OFFLINE` geometry = 27.2 s chunks with 3.2 s
  look-ahead (`crates/meeting/src/post/diarize.rs#L104-L111`). Model files: `nemotron-diarization/model_quantized.onnx`
  + wespeaker CAM++ for voiceprints (`crates/meeting/src/post/diarize.rs#L28-L34`), one embedding per speaker from up
  to 30 s of their turns, ≥ 1 s (`crates/meeting/src/post/diarize.rs#L41-L42`). Frame→turn rules: speech ≥ 0.5,
  gaps < 500 ms merged, turns < 200 ms dropped (`crates/meeting/src/post/diarize.rs#L122-L128`). Without models the
  pipeline still runs and every system segment is Speaker 1 (`crates/meeting/src/post/mod.rs#L171-L183`).
  (The AXES context line "pyannote-rs segmentation + wespeaker" is out of date: pyannote segmentation was replaced
  2026-09-24 because it over-split real meetings into 21/25 "speakers" — `docs/design/meetings/04-post-nemotron-as-built.md#L9-L25`.)
- **Live ↔ post agreement:** stage 3 renames offline labels to the live ones by overlap (cover ≥ 0.5) so "Speaker 2" is
  one voice in the session and in the record (`crates/meeting/src/post/mod.rs#L206-L216`,
  `crates/meeting/src/post/align.rs#L114-L124`).
- **Voiceprints across meetings:** profiles at `<data_dir>/meetings/speakers/` (`crates/meeting/src/speakers.rs#L13`,
  `crates/meeting/src/store.rs#L272-L275`), a running-mean embedding per person, cosine match threshold **0.6**
  (`crates/meeting/src/speakers.rs#L15`, `crates/meeting/src/speakers.rs#L97-L107`); create / add_sample / delete
  (`crates/meeting/src/speakers.rs#L137-L214`). **Recognition is semi-automatic:** after diarization each label gets a
  `proposed` best match (`crates/meeting/src/post/mod.rs#L240-L254`), shown as "looks like X (NN %)" with *Accept*
  (`crates/desktop/ui/src/components/meeting/Review.tsx#L105-L108`) — never applied without the user.
- **Correction UI:** `EditOp::{Relabel, Rename, Merge, Split, FixText, Enroll, Reviewed}`
  (`crates/meeting/src/transcript.rs#L111-L156`) applied via `transcript::apply` (`crates/meeting/src/transcript.rs#L220`).
  Review legend: rename, accept proposed, enroll (`crates/desktop/ui/src/components/meeting/Review.tsx#L70-L108`).
  Per-segment ⋯ menu (156 D11): Play / Stop playback, Split at a word (Review only), Enroll as new speaker…, Add to
  enrolled speaker ▸, acting on the multi-selection (`crates/desktop/ui/src/components/meeting/SegmentMenu.tsx#L1-L2`,
  `crates/desktop/ui/src/components/meeting/SegmentMenu.tsx#L44-L45`). Segment-range enrollment:
  `transcript::speech_spans` / `voiceprint_spans` / `link_enrolled` (`crates/meeting/src/transcript.rs#L334-L402`).
- **Profile ↔ knowledge entity:** `Enroll { entity: Option<String> }` links a person to a knowledge entity id
  (`crates/meeting/src/transcript.rs#L145-L153`).
- **Agent access:** `meeting_speakers` tool (`crates/meeting/src/tools.rs#L377-L395`).

**(b) Missing**

- **Live labels never use profiles:** no `best_match`/`Profiles` in `crates/meeting/src/diar.rs` (grep: no matches) —
  live lines say `Speaker 2`, never `John`, even for an enrolled voice; profiles are only consulted post-meeting
  (`crates/meeting/src/post/mod.rs#L226-L254`) and in edit/enroll handlers (`crates/meeting/src/lib.rs#L3345`,
  `crates/meeting/src/lib.rs#L3430`).
- **No automatic naming from calendar attendees** (no calendar at all — see A1/A6), no names from the meeting app UI.
- **Diarization of the mic track:** none — a room with several people on one mic is all `me`
  (`crates/meeting/src/lib.rs#L519-L520`); 8-speaker cap from the model (`crates/meeting/src/post/diarize.rs#L56`).
- **Proposals are not auto-accepted** at any confidence; no "always label John" setting (by Sky's design: "harness may
  propose a match with confidence for confirmation", `projects/wth-meeting-assistant`).
- No profile export/import (Sky: "delete profile = enough; no export").

**(c) Designs**

- `docs/design/meetings/07-live-diarization.md` — Status **as built** 2026-09-24 (`docs/design/meetings/07-live-diarization.md#L3`).
- `docs/design/meetings/04-post-nemotron-as-built.md` — as built 2026-09-24 (`docs/design/meetings/04-post-nemotron-as-built.md#L3`).
- `docs/design/156-meeting-view.md` D11–D16 (segment menu, `meeting_speaker_enroll`) — landed main 8fc2b7b8
  (`projects/theuth-design-156-meeting-view-2026-10-09`).
- Live profile recognition — not designed (07 scopes live labels as anonymous `Speaker N`).

## A4 Notes, summaries and the LLM layer

**(a) What exists**

- **Who writes the summary:** not a dedicated summariser — one *normal turn in the meeting's own agent session*,
  triggered by the user's Summarize after review; then `summary.md`, the title, `knowledge_harvest` and a compaction
  with the summary as head (`crates/meeting/src/summary.rs#L1-L7`). The model is the meeting session's model, chosen in
  the start form (the chat header's picker, remembered) and stored in the manifest (`crates/meeting/src/lib.rs#L639-L641`;
  `crates/desktop/ui/src/components/meeting/StartForm.tsx#L1-L3`). A 10-minute timeout guards the turn
  (`crates/meeting/src/lib.rs#L1118`, `crates/meeting/src/lib.rs#L3592-L3613`).
- **The prompts (verbatim constants):**
  - legacy, no note: "`[meeting summary]` … Use `meeting_read` (all of it, in pieces) and `meeting_speakers`, then write
    the summary as markdown: a `#` title line (≤ 8 words), then **Topics**, **Decisions** (with who), **Action items**
    (owner — item — due if said), **Open questions**, **Numbers and names said aloud** … under 400 words"
    (`crates/meeting/src/summary.rs#L21-L27`);
  - with a note (the default since 59): `doc_read` the note outline, regroup the user's blocks under headings with
    `doc_move` — "never rewrite or delete a block the user wrote" — then `doc_insert(at="end")` a **Summary**
    (≤ 120 words), **Decisions**, **Action items**, **Open questions**, **Numbers and names said aloud**; reply with the
    title only (`crates/meeting/src/note.rs#L21-L35`). `form_for` picks between them (`crates/meeting/src/summary.rs#L29-L35`).
- **User-typed notes during the meeting + merge ("enhance"):** yes. One `MeetingNote` block document per meeting is
  created at `meeting_start`, shown beside the live transcript, edited by the summary turn through the `doc_*` tools
  (user blocks moved, never rewritten), exported as `meetings/<id>/notes.md`, then `summary.md`
  (`crates/meeting/src/note.rs#L1-L7`). Snapshots `meeting-before-summary` / `meeting-summary` make "the agent
  rearranged my notes badly" one restore (`crates/meeting/src/note.rs#L16-L19`). The desktop mounts the full note view
  embedded in the meeting page (`crates/desktop/ui/src/components/meeting/Note.tsx#L10-L18`). The start form's
  free-text `context` goes to the session too (`crates/meeting/src/lib.rs#L633-L635`).
- **Long-transcript handling:** the summary turn reads the record through `meeting_read` in pieces (cap 400 lines per
  call, `crates/meeting/src/tools.rs#L23-L24`) rather than in context; during the meeting the session compacts at a
  32k-token trigger with a plugin-supplied head, "at no model cost" (`crates/meeting/src/lib.rs#L536-L541`), or uses the
  137 endless memory mode per meeting (`crates/meeting/src/lib.rs#L642-L645`; `crates/meeting/src/endless.rs`).
- **Action items / decisions:** sections in the form (above); not schema-validated JSON — markdown in a block document.
- **Regeneration:** a failed Review accepts Summarize again (`crates/meeting/src/lib.rs#L4206-L4215`); a restart re-sends
  the form (`crates/meeting/src/lib.rs#L3686-L3708`); "Re-run post pipeline" sits in the header row
  (`crates/desktop/ui/src/components/meeting/Summary.tsx#L1-L3`).
- **Knowledge harvest:** after the summary, `agent::Harvest { session }` with one retry
  (`crates/meeting/src/summary.rs#L159-L170`); then `agent::Compact { style: Summary, head: summary }` so the
  follow-up chat starts from the summary (`crates/meeting/src/summary.rs#L172-L181`).
- **Local LLM path:** none meeting-specific, but the harness can run any session on a keyless local adapter —
  `ollama/llama3` resolves to genai's `ollama::llama3` "with no entry" (`crates/models/src/lib.rs#L363`), Ollama is a
  keyless adapter (`crates/agent/src/supervisor.rs#L9257`), and named OpenAI-compatible gateways cover LM Studio /
  llama.cpp servers (`crates/agent/src/lib.rs#L2136-L2146`; `/provider add` in `crates/tui/src/model.rs#L701-L706`).
  So picking a local model in the start form makes live briefs *and* the summary local. A **local decision model**
  (laya / GLiNER-class in candle, `crates/decision/Cargo.toml#L6`) already pre-filters live batches (A5).

**(b) Missing**

- **No templates per meeting type** (1:1, stand-up, sales call, interview): grep `template` in `crates/meeting/src` —
  no matches; one hard-coded form (`crates/meeting/src/note.rs#L23-L34`). No user-editable prompt; no variables;
  no template sharing.
- **No bundled local LLM** and no "local-only" switch for meetings: the session model is whatever the picker holds
  (`crates/meeting/src/lib.rs#L639-L641`); nothing warns that transcript text goes to a cloud model (see A8).
- **No map-reduce summariser:** a 3 h meeting is summarised by one agent turn paging through `meeting_read` — bounded by
  the model's context/patience and the 10-minute timeout (`crates/meeting/src/lib.rs#L1118`); no chunked pre-summaries
  stored for the summary turn (the live head/endless tree exists for the *live* context only).
- **No structured frontmatter** on `summary.md` (title/date/attendees/action items as YAML): the export is the note's
  markdown (`crates/meeting/src/note.rs#L37-L40`).
- **No auto-summarize on stop:** Summarize is a user action after review (by design: "user clicks Summarize").
- No action-item push to tasks/reminders/Jira from the meeting (the agent could, with tools, but no flow).

**(c) Designs**

- `docs/design/meetings/04-post.md` §6 (guided summary, title, harvest, compaction) — built.
- Design 59 §7.4 (meeting note as a block document) — built (`crates/meeting/src/note.rs#L1`).
- `docs/design/81-as-built-MEETING-HEAD.md` / `81-as-built-MEETING-DIET.md` (head + compaction floor for the live
  session) — built; 137 meetings "hybrid NOW" (head kept, tree over older transcript) — built
  (`crates/meeting/src/endless.rs`, `crates/meeting/src/endless_agent.rs`).
- Templates / local-only meeting mode — not designed.

## A5 Live assistance during the meeting

**(a) What exists — the live loop end to end**

1. **Utterance in:** each closed utterance is appended to `live.jsonl`, run through the cache alias scan (05 §3), then the
   batcher (`crates/meeting/src/lib.rs#L1642`). Empty/punctuation-only lines are dropped (`crates/meeting/src/lib.rs#L1732-L1736`).
2. **Brief cache hot path (no model):** `briefs.sqlite` holds `briefs` (truth) + `briefs_fts` (FTS5 mirror); the
   lowercased alias list is held in memory and scanned on every closed utterance before any model turn
   (`crates/meeting/src/cache.rs#L1-L5`, `crates/meeting/src/cache.rs#L18-L21`); a hit puts a topic up with source
   `Cache` (`crates/meeting/src/lib.rs#L470`, scan at `crates/meeting/src/lib.rs#L1139`). Rows carry title, 2–3
   bullets, spoken aliases (≤ 4 words), tags, source, use counts (`crates/meeting/src/cache.rs#L23-L40`). Design target:
   on the island < 1 s after the words in a second meeting (`docs/design/meetings/README.md#L16`).
3. **Batcher:** flush after `batch_secs` (25) with ≥ 1 pending, at `batch_utterances` (6), or at once when a system
   utterance ends in `?`; requests bypass it (`crates/meeting/src/batch.rs#L1-L7`; defaults
   `crates/meeting/src/lib.rs#L586-L587`); `MAX_QUEUED` 3 (`crates/meeting/src/batch.rs#L15`).
4. **Local decision-oracle pre-filter (57):** deterministic `mentions` (capitalised n-grams, ticket ids, acronyms,
   numbers with units, quoted terms) → tier 1a small-talk `Bool` → tier 1b `Choice` over the top-4 FTS cached briefs →
   asymmetric gate (skip only when small talk *and* no mentions; publish a cached brief directly when 1b is sure) →
   tier 2 subject extraction → tier 3 the model turn; modes off / shadow / active (`crates/meeting/src/oracle.rs#L1-L20`,
   constants `crates/meeting/src/oracle.rs#L35-L50`). Taps and dismissals are recorded as training reactions
   (`crates/meeting/src/lib.rs#L2920`, `crates/meeting/src/lib.rs#L2947`).
5. **The model turn:** the meeting session reads `[meeting hh:mm:ss]` batches with `me:` / `Speaker N:` / `them:` and is
   told "notice anything the user might have to ask about … look it up with your tools (knowledge first) … call
   `meeting_topic` with a label of at most four words … two or three short factual bullets. **Never write what the user
   should say**" — plus request / tapped / deeper protocols and the cache-first lookup order (`crates/meeting/src/tools.rs#L26-L57`).
   `meeting_topic` / `meeting_brief` are `ReadOnly`, `DIRECT` (`crates/meeting/src/tools.rs#L107-L110`,
   `crates/meeting/src/tools.rs#L146-L148`). Topic labels ≤ 4 words, 2–3 bullets enforced (`crates/meeting/src/topics.rs#L14-L17`);
   TTL 600 s, max 6 buttons (`crates/meeting/src/lib.rs#L589-L590`).
6. **Live toolset (06):** while recording the session gets an allowlist called directly (no python runtime) —
   `meeting_topic, meeting_brief, brief_search, brief_put, knowledge_search, knowledge_get, meeting_read, meeting_search,
   search_web, read_url` — prompt blocks `meeting, knowledge, web`, `<knowledge>` hints capped at 2 lines
   (`crates/meeting/src/lib.rs#L568-L578`, `crates/meeting/src/lib.rs#L593-L608`).
7. **Context management:** 32k compaction trigger with a model-free plugin head (`crates/meeting/src/lib.rs#L536-L541`;
   `crates/meeting/src/head.rs`), compaction hint (`crates/meeting/src/tools.rs#L59-L63`), or 137 endless memory
   (`crates/meeting/src/lib.rs#L642-L645`; config follow at `crates/meeting/src/lib.rs#L3743-L3750`).
8. **Steering:** push-to-talk — `PushToTalk { held }` publishes `Listening`, and the release tags the closing mic
   utterance as a spoken request (`crates/meeting/src/lib.rs#L799-L805`); mic utterances while held are reported as one
   request on release (`crates/meeting/src/stt.rs#L48-L56`); RPC `meeting_ptt` (`crates/rpc/src/api.rs#L6822`). Also
   typed requests (`meeting_request`), topic taps (`meeting_tap`), deeper dives (`meeting_deeper`), dismiss
   (`meeting_dismiss`), the island cursor (`meeting_cursor`) — the `meeting_*` RPC list in `crates/rpc/src/api.rs#L1723-L1834`.
9. **The island:** a 440 × 44 strip at the notch (meters, rec clock, topic buttons, badges, the eye, ⋮ pause/stop) that
   grows into the brief pane, a collapsed live-transcript dropdown and a request box, in its own non-activating
   `NSPanel` webview (`crates/desktop/ui/src/components/meeting/Island.tsx#L1-L5`). Native companions draw it too:
   SwiftUI `ClosedPill` / `OpenIsland` / `DetachedIsland` (`crates/desktop/companion/Sources/Companion/Island/IslandViews.swift#L78-L168`)
   and the Windows C# `IslandModel` (per-monitor placement memory, `WDA_EXCLUDEFROMCAPTURE` eye default)
   (`crates/desktop/companion-windows/src/Theuth.Companion.Core/IslandModel.cs#L22-L59`).
10. **Hidden from screen capture:** `set_hidden_from_capture` applies `set_content_protected` to both windows —
    `NSWindow.sharingType = .none` on macOS, `WDA_EXCLUDEFROMCAPTURE` on Windows, nothing on Linux
    (`crates/desktop/src/island/macos.rs#L64-L67`, `crates/desktop/src/island/mod.rs#L88-L92`,
    `crates/desktop/src/island/mod.rs#L133-L136`); the island says so when the OS cannot
    (`crates/desktop/ui/src/components/meeting/Island.tsx#L254-L265`).
11. **Pre-meeting prefill:** outside a meeting the user can ask a chat to research subjects and `brief_put` them with
    aliases and a tag for the meeting (`crates/meeting/src/tools.rs#L48-L51`); `/briefs` page
    (`crates/desktop/ui/src/routes/meetings.briefs.tsx`).

**What the competitors are unlikely to have:** an agent with tools *during* the call (knowledge base first, then web),
a cross-meeting brief cache with spoken aliases and a sub-second no-model hot path, a local classifier gate that skips
small talk, push-to-talk asks routed to the same agent, an always-on-top non-activating island hidden from screen
shares on macOS and Windows, and live speaker labels at RTF 0.035.

**(b) Missing**

- **No running summary shown to the user** during the meeting (the head/endless summary feeds the model, not the UI —
  `crates/meeting/src/lib.rs#L536-L541`); no "catch me up on the last 5 minutes" button.
- **No live action-item / decision capture** surfaced while live (only after Summarize).
- **No measured end-to-end latency** (utterance → brief) anywhere in code or tests; targets only in the plan
  (`docs/design/meetings/README.md#L16`, `docs/design/meetings/README.md#L163-L164`).
- **Linux cannot hide the island** from capture (`crates/desktop/src/island/mod.rs#L133-L136`); macOS `sharingType`
  exclusion is contested on macOS 15+ (knowledge `facts/macos-screen-capture-exclusion-sharingtype`).
- **No TTS in the earpiece** (out of scope v1 by Sky's decision).
- No phone/mobile island; recording from phone out for v1.

**(c) Designs**

- `docs/design/meetings/02-live.md` (live STT, batcher, topics/briefs) — built.
- `docs/design/meetings/03-island.md` (island, eye, PTT, placement) — built; 149 island-fix as-builts for macOS and
  Windows (`docs/design/149-as-built-island-fix.md`, `docs/design/149-as-built-island-fix-windows.md`).
- `docs/design/meetings/05-brief-cache.md` — built (`crates/meeting/src/cache.rs#L1-L5`).
- `docs/design/meetings/06-live-toolset.md` — built (`crates/meeting/src/lib.rs#L559-L577`).
- Design 57 oracle pre-filter — built, mode-switched (`crates/meeting/src/oracle.rs#L16-L20`).

## A6 Memory, search, agent access and integrations

**(a) What exists**

- **Storage format (verified on Sky's Mac, formats only):** `~/.theuth/meetings/` holds `meetings.sqlite` (index),
  `briefs.sqlite` (brief cache), `speakers/` (0 profiles today) and one directory per meeting (5 today) with
  `manifest.json`, `mic.opus`, `system.opus`, `mix.opus`, `live.jsonl`, `briefs.jsonl`, `transcript.json`,
  `embeddings.json`, `summary.md` (or `notes.md`). Manifest keys: `consent_at, context, degraded, duration_ms, ended_at,
  failure, gaps, id, memory, note, process, session, session_model, started_at, state, title, tracks`. Transcript keys:
  `diarized, meeting, note, reviewed_at, segments, speakers, summarized_at`; a segment is
  `{id, speaker, t0_ms, t1_ms, text, track, words[{t0_ms, t1_ms, text}]}`; a `live.jsonl` line is
  `{request, t0_ms, t1_ms, text, track}` (+ `speaker` when labelled, `crates/meeting/src/lib.rs#L509-L512`);
  `embeddings.json` maps 8 labels → 512-d vectors (wespeaker). A stray set of root-level `manifest.json/live.jsonl/…`
  from a 2026-09-20 test also sits in `meetings/` (matches knowledge `facts/theuth-meeting-page-and-meeting-audio-seams-2026-10-09`).
  The meeting note itself is a block document in the documents store, owner `MeetingNote(<id>)`
  (`crates/document/src/meta.rs#L157-L158`), exported to `summary.md`/`notes.md` (`crates/meeting/src/note.rs#L1-L7`).
- **Full-text search:** an FTS5 virtual table `segments(meeting, seg, t0, t1, speaker, text)` re-indexed per meeting on
  every edit and rebuilt from `transcript.json` (`crates/meeting/migrations/0002_segments.sql#L1-L10`). The unified
  `search()` category `meetings`: titles nucleo-ranked + transcript segments through the same FTS index, one hit per
  meeting (`crates/meeting/src/search.rs#L1-L3`, caps `crates/meeting/src/search.rs#L16-L18`). Brief cache has its own
  FTS5 mirror (`crates/meeting/src/cache.rs#L1-L5`).
- **Agent tools over meetings (every session):** `meeting_read` (≤ 400 lines per call), `meeting_search`,
  `meeting_speakers`, `meeting_list`, all `ReadOnly` (`crates/meeting/src/tools.rs#L23-L24`,
  `crates/meeting/src/tools.rs#L258-L259`, `crates/meeting/src/tools.rs#L341-L348`, `crates/meeting/src/tools.rs#L395-L396`,
  `crates/meeting/src/tools.rs#L461-L467`); `brief_search / brief_list / brief_put / brief_delete`
  (`crates/meeting/src/tools.rs#L573-L705`). The meeting's own session continues after the meeting as a chat whose first
  line is the summary (`crates/meeting/src/summary.rs#L172-L181`; prompt `crates/meeting/src/tools.rs#L41-L43`).
- **Knowledge harvest:** automatic after the summary turn (`crates/meeting/src/summary.rs#L159-L170`); speaker profiles
  can link a knowledge entity (`crates/meeting/src/transcript.rs#L145-L153`).
- **Notebooks:** `source_add(meeting=<id>)` ingests a meeting's transcript as a notebook source
  (`crates/document/src/ingest/mod.rs#L32-L49`, route `crates/document/src/ingest/routes/mod.rs#L67-L68`), origin key
  `meeting:<id>` (`crates/document/src/meta.rs#L167`); notebook search over sources is lexical + semantic (the `nb_search`
  tool), so a meeting becomes *semantically* searchable once added to a notebook.
- **Exports:** the meeting note gets the regular note action bar — Rename, Attach to chat, Snapshots, Export, Share,
  pop-out (`crates/desktop/ui/src/components/meeting/Note.tsx#L10-L18`; rulings in
  `decisions/theuth-meeting-page-rulings-156-2026-10-09`); `summary.md` on disk is plain markdown.
- **RPC (the programmatic surface):** 36 `meeting_*` / `brief_*` JSON-RPC methods (`crates/rpc/src/api.rs#L1723-L1834`,
  `meeting_oracle_mode` at `crates/rpc/src/api.rs#L2063`, `crates/rpc/src/api.rs#L2103-L2126`) over the engine's WebSocket — what the desktop, TUI and companions use.
- **MCP:** theuth is an MCP *client* (rmcp `client` features, `Cargo.toml#L203`) and serves MCP only as an internal
  stdio shim for its own ACP children: `t mcp --session <id>` exposes three tools — `list_skills`, `load_skill`,
  `send_message` (`crates/theuth/src/mcp.rs#L1-L6`, `crates/theuth/src/main.rs#L171-L178`).

**(b) Missing**

- **No MCP server exposing meetings to external agents** (Claude Code, Codex, any MCP client): the only server is the
  3-tool child shim (`crates/theuth/src/mcp.rs#L1-L6`); meeting tools exist only inside theuth sessions. (Minutes'
  headline feature is ~31 MCP tools over the meeting history.)
- **No semantic search over meetings by default:** `meeting_search` / `search()` are FTS + fuzzy titles
  (`crates/meeting/src/search.rs#L1-L3`, `crates/meeting/migrations/0002_segments.sql#L3`); semantic only via a
  notebook source. No "ask across all meetings" surface beyond a chat using the tools.
- **No calendar integration** of any kind (pre-meeting context, attendees, auto-titles, auto-start): no calendar code in
  `crates/` (searched `calendar|EventKit|google.?calendar|outlook.?calendar`); "calendar briefs" out of scope v1
  (`projects/wth-meeting-assistant`). The `native-apps` skill can script macOS Calendar through AppleScript, but the
  meeting feature does not use it.
- **No markdown+YAML-frontmatter files per meeting** (Obsidian-style vault): transcript is JSON, summary is plain markdown.
- **No exports to Notion/Slack/email from the meeting page**; the agent can post to Slack (Slack MCP tools exist in the
  harness) but there is no flow or button.
- **No people/company pages** derived from meetings (knowledge entities exist; no meeting → person rollup view).

**(c) Designs**

- `docs/design/meetings/04-post.md` §5 (FTS, tools) — built; design 66 §10 (unified search category) — built
  (`crates/meeting/src/search.rs#L1`); design 144 notebooks (`source_add(meeting=…)`) — built; design 75 §9 (`t mcp`
  child shim) — built. MCP server for external agents, semantic meeting index, calendar — not designed.

## A7 UX surfaces and onboarding

**(a) What exists (post-156, main 2a1238f2)**

- **Library › Meetings list:** `/meetings` — recorded meetings with date, title, duration, state, process and *Start
  meeting…*; every device (phones over the relay included) reads meetings, summaries and transcripts; only a desktop
  shell talking to its own daemon records (`crates/desktop/ui/src/components/meeting/MeetingList.tsx#L1-L3`). A Briefs
  sub-tab sits beside the list (`crates/desktop/ui/src/components/meeting/MeetingList.tsx#L72`); rows: click opens,
  hover shows attach · note in new window · delete, right-click/⋯ the same menu
  (`crates/desktop/ui/src/components/meeting/MeetingList.tsx#L118-L126`). `useCanRecord` = local engine + desktop +
  `supported != None` (`crates/desktop/ui/src/components/meeting/MeetingList.tsx#L61-L69`).
- **Start:** `/meetings/new` → `StartForm`, or a hint off the local machine
  (`crates/desktop/ui/src/routes/meetings.new.tsx#L6-L21`); tray *Start meeting…* (`crates/desktop/src/lib.rs#L382-L391`).
  Permissions row with deep links to System Settings panes for Microphone and System audio recording
  (`crates/desktop/ui/src/components/meeting/StartForm.tsx#L152-L154`, `crates/desktop/ui/src/components/meeting/StartForm.tsx#L177`).
- **Meeting page live:** state, elapsed, degradations, pause/resume/stop, the live log (`me:`/`them:`; partials replace
  the open row of their track, finals commit), a typed request box, the brief column with the island's cursor
  mirrored, the session link, and the meeting note beside them (`crates/desktop/ui/src/components/meeting/ControlCenter.tsx#L1-L4`).
- **Meeting page after:** one status strip `Transcribing → Diarizing → Aligning → Review → Summarizing → Done` with the
  current step, a spinner, the failure and its fix (*Re-run post pipeline*, *Retry summary*) inline, the transcript
  (editable in Review, read-only while Summarizing so edits never race the summary turn) and the note beside it
  (`crates/desktop/ui/src/components/meeting/ControlCenter.tsx#L4-L9`). Controls by state in the 44 px row under the
  Library tabs: Review → *Done reviewing*, *Summarize* (disabled until reviewed); Done → *Re-run post pipeline*;
  Failed → the strip's fix; *Delete meeting…* in a trailing ⋯ (`crates/desktop/ui/src/components/meeting/MeetingActions.tsx#L1-L7`).
  Full-height, resizable, collapsible `BriefColumn`, oldest first, cursor ringed, refetched on `TopicAdded`,
  `TopicRemoved`, `Brief` (`crates/desktop/ui/src/components/meeting/BriefColumn.tsx#L1-L5`). Note = the regular
  `NoteView` embedded (`crates/desktop/ui/src/components/meeting/Note.tsx#L10-L18`). Segment playback, ⋯ menu,
  enrollment (A3).
- **States** the engine models: `Idle, Starting, Recording, Paused, Stopping, Post(Transcribing|Diarizing|Aligning),
  Review, Summarizing, Done, Failed` (`docs/design/meetings/README.md#L57-L59`; `stateLabel`
  `crates/desktop/ui/src/components/meeting/MeetingList.tsx#L27-L28`).
- **Island / floating widget:** the webview island (`crates/desktop/ui/src/components/meeting/Island.tsx#L1-L5`) and the
  native companion islands on macOS (SwiftUI) and Windows (C#) (A5). Idle "Always show" pill → click opens Meetings
  (`crates/desktop/companion/Sources/Companion/Island/IslandViews.swift#L78-L124`).
- **Hotkeys (companion defaults):** Meeting push-to-talk ⌥\`, Meeting eye ⌥⇧E, Stop the meeting (hold) ⌃M on the
  island (`crates/desktop/companion/Sources/CompanionCore/KeyBindings.swift#L222-L244`); Tauri global-shortcut plugin in
  the shell (`crates/desktop/src/lib.rs#L259`).
- **Mobile:** phones read meetings over the relay (`crates/desktop/ui/src/components/meeting/MeetingList.tsx#L1-L3`); the
  156 phone layout is one scroll with a collapsible "Briefs · N" block (`decisions/theuth-meeting-page-rulings-156-2026-10-09`).
- **Dev fixtures:** `?sim=meeting-live|meeting-review|meeting-done` and `/island?fixture=meeting`
  (`projects/theuth-design-156-meeting-view-2026-10-09`).

**(b) Missing**

- **No first-run onboarding for meetings** (permission walkthrough, test recording, model download check): only the
  permissions row in the start form (searched `onboard|first.run|welcome` in the meeting UI: none); Sky ruled
  "onboarding flow out of scope" (`projects/wth-meeting-assistant`).
- **Stale platform gating in the shell:** the tray *Start meeting…* item is added only on macOS with the comment "the
  recorder is macOS only (Core Audio taps)" (`crates/desktop/src/lib.rs#L382-L391`) and the off-machine hint says "the
  Mac running the node" (`crates/desktop/ui/src/routes/meetings.new.tsx#L19`), while the engine and `useCanRecord`
  allow Windows/Linux (`crates/meeting/src/capture/system.rs#L106-L116`,
  `crates/desktop/ui/src/components/meeting/MeetingList.tsx#L61-L69`).
- **No OS notification** when post-processing finishes or a summary is ready (toasts only inside the page; no
  `notif` hits in `crates/meeting/src`).
- **No "meeting detected — record?" prompt** (no detection, A1).
- No meeting notes in Library › Notes (open item from 156, `decisions/theuth-meeting-page-rulings-156-2026-10-09`);
  no rename from the note title.

**(c) Designs**

- `docs/design/meetings/03-island.md` (island, control center, start form, tray) — built; `docs/design/156-meeting-view.md`
  + `156-as-built-{PAGE,TRANSCRIPT,ENGINE}.md` — landed main 8fc2b7b8; design 149 companion islands (macOS + Windows)
  — built (`docs/design/149-as-built-island-fix.md`). Onboarding — designed-out for v1.

## A8 Privacy, local-only guarantees and business model

**(a) What exists — what leaves the machine, and when**

- **Audio never leaves:** capture, encode, live STT (Nemotron), post STT (Parakeet TDT), live and post diarization
  (Nemotron 3 Diarization + wespeaker) all run on local ONNX models from `<data_dir>/models`
  (`crates/voice/src/stt.rs#L1-L7`, `crates/meeting/src/post/diarize.rs#L28-L34`, `scripts/fetch-models.sh#L48-L56`);
  audio files stay under `<data_dir>/meetings/<id>/` (`crates/meeting/src/lib.rs#L1-L5`). No cloud STT client exists
  (`crates/voice/src/stt.rs#L14-L23`).
- **Transcript text goes to the meeting session's model** — live batches every ~25 s / 6 utterances
  (`crates/meeting/src/lib.rs#L586-L587`) and the summary turn (`crates/meeting/src/summary.rs#L1-L7`) — i.e. to whatever
  provider the user picked at start (`crates/meeting/src/lib.rs#L639-L641`); local if that is an Ollama /
  OpenAI-compatible local endpoint (`crates/models/src/lib.rs#L363`, `crates/agent/src/supervisor.rs#L9257`).
  Before a batch reaches it, the local decision oracle may **skip** small-talk batches entirely
  (`crates/meeting/src/oracle.rs#L10-L12`), which also reduces what is sent.
- **Search queries leave** when the live allowlist's `search_web` / `read_url` run (`crates/meeting/src/lib.rs#L593-L606`):
  subjects heard in the meeting become web queries to the configured search providers.
- **Harvest:** `agent::Harvest { session }` harvests the meeting session's frame into knowledge after the summary
  (`crates/agent/src/lib.rs#L827-L836`, `crates/meeting/src/summary.rs#L159-L170`) — another model call over meeting
  text; `MeetingConfig` has no harvest or model key to steer it (`crates/meeting/src/lib.rs#L529-L566`).
- **Phones over the relay** read meetings, summaries and transcripts (`crates/desktop/ui/src/components/meeting/MeetingList.tsx#L1-L3`).
- **Hidden from screen shares:** island and main window `set_content_protected` (macOS `sharingType = .none`,
  Windows `WDA_EXCLUDEFROMCAPTURE`) (`crates/desktop/src/island/macos.rs#L64-L67`, `crates/desktop/src/island/mod.rs#L88-L92`).
- **Consent:** the start is refused without `consent` (`crates/meeting/src/lib.rs#L621-L638`); the manifest records
  `consent_at` (verified key on disk, A6).
- **Telemetry:** none found — grep for `posthog|sentry|telemetry|analytics|mixpanel` in `crates/` finds only a
  tracking-parameter stripper (`crates/web/src/url.rs#L13`) and an explicit opt-out for the OpenUI helper
  (`crates/app/src/helper.rs#L380`).
- **Accounts / paid tiers:** none — a personal harness; no sign-in for meetings, nothing held back. No LICENSE file at
  the repo root and no `license` key in `Cargo.toml` (checked `ls LICENSE*`, `grep license Cargo.toml`).

**(b) Missing**

- **No "local-only meeting" mode** that pins the session model to a local endpoint and turns off `search_web`/`read_url`
  for the meeting; the allowlist is config-global (`crates/meeting/src/lib.rs#L559-L561`), the model is per meeting.
- **No disclosure in the start form or island** of where transcript text is going (provider/model name, web on/off) —
  the model picker is there (`crates/desktop/ui/src/components/meeting/StartForm.tsx#L1-L3`) but no privacy line.
- **No redaction** (PII, numbers) before text leaves; no per-meeting "don't harvest" switch (harvest is unconditional,
  `crates/meeting/src/summary.rs#L159-L170`).
- **No encryption at rest** for meeting audio/transcripts (plain files; the vault covers secrets only).
- **No retention/auto-delete** (A1).

**(c) Designs**

- 53 decisions: "local-only (no audio leaves the machine, only text to the model)"; eye toggle default hidden
  (`projects/wth-meeting-assistant`) — built. Local-only mode / redaction / encryption at rest — not designed.

## A9 Engineering, platforms and traction

**(a) What exists**

- **Architecture:** one Rust crate `meeting` (~15.8k lines in `crates/meeting/src`, `wc -l` at main 2a1238f2: lib.rs 4,415,
  oracle.rs 1,441, post/diarize.rs 925, transcript.rs 762, tools.rs 734, store.rs 703, topics.rs 672, cache.rs 639, …) as a
  runtime plugin with a kameo actor (`crates/meeting/src/lib.rs#L3756-L3812`), sharing `crates/voice` (STT/VAD/live)
  with dictation (`crates/voice/src/live.rs#L1-L5`); RPC in `crates/rpc/src/api.rs#L1723-L1834`; Tauri/React UI
  (`crates/desktop/ui/src/components/meeting/*`, 2,154 lines of `.tsx`); native islands in Swift and C# (A5). Capture
  via `flexaudio` (`crates/meeting/src/capture/system.rs#L1-L7`); ONNX Runtime with a selectable execution provider
  (`crates/voice/src/stt.rs#L163-L178`).
- **Platforms per feature:**
  | feature | macOS | Windows | Linux |
  |---|---|---|---|
  | system/app capture | Core Audio process tap (`crates/meeting/src/capture/system.rs#L129`) | WASAPI (process) loopback (`crates/meeting/src/capture/system.rs#L239`) | PipeWire (`crates/meeting/src/capture/system.rs#L343-L358`) |
  | mic AEC | VPIO (`crates/meeting/src/capture/mic.rs#L38-L42`) | none → `MicNoAec` | none → `MicNoAec` |
  | `supported()` | `Full` | `Partial` | `Partial` / `None` (`crates/meeting/src/capture/system.rs#L106-L116`) |
  | island hidden from capture | `sharingType=.none` | `WDA_EXCLUDEFROMCAPTURE` | no-op (`crates/desktop/src/island/mod.rs#L133-L136`) |
  | tray *Start meeting…* | yes | **no** (`crates/desktop/src/lib.rs#L382-L391`) | **no** |
  | ONNX execution provider | per `models.accelerator` (`crates/voice/src/stt.rs#L163-L178`) | status reports `DmlExecutionProvider \| CPUExecutionProvider` (`crates/meeting/src/lib.rs#L708-L711`) | CPU |
  Windows capture was verified opening and enumerating, Linux captured real audio in an LXC (knowledge
  `facts/theuth-meeting-capture-verified-windows-linux-2026-09-23`); a build-B engine died during a model-less meeting
  with WebGPU live diarization (knowledge `facts/build-b-engine-2-14-dies-in-model-less-meeting-webgpu-diarization-2026-10-09`),
  followed by the 149 GPULOCK fix (`git log -- crates/meeting`: a1e6e122, 79fb799f).
- **Tests:** 76 `#[test]`/`#[tokio::test]` in `crates/meeting/src` (grep count) incl. restart recovery
  (`crates/meeting/src/lib.rs#L4190-L4200`) and summarize-again (`crates/meeting/src/lib.rs#L4206-L4224`); integration
  tests `crates/meeting/tests/{note,oracle,webgpu_concurrency}.rs`; examples `capture_probe.rs`, `live_diar_bench.rs`,
  `live_diar_smoke.rs` (`crates/meeting/examples/`); 5 UI test files (`ControlCenter`, `IslandTranscript`,
  `MeetingActions`, `MeetingList`, `StartForm` `.test.ts`); companion fixtures `meeting_events_149.json`,
  `meeting_status_149*.json` (`crates/desktop/companion/Tests/TheuthKitTests/Fixtures/`).
- **Benchmarks that exist:** the live diarization bench numbers baked into the constant's doc (RTF 0.035, 97–99 % frame
  agreement, `crates/meeting/src/post/diarize.rs#L112-L114`); the oracle's phrasing bench chose the small-talk statement
  (`crates/meeting/src/oracle.rs#L47-L48`).
- **Diagnostics:** degradations surfaced to UI (`crates/meeting/src/lib.rs#L184-L206`), `tracing` logs to the engine log.
- **Updater:** `crates/update` (shared app updater; nothing meeting-specific).
- **Cadence:** 71 commits touch `crates/meeting` since 2026-09-19 (first commit of the crate), last 2026-10-09 22:33
  (59ec7e43, 156 ENGINE) — `git log --oneline -- crates/meeting | wc -l`.

**(b) Missing**

- **No transcript or summary evals:** no WER fixture, no brief-quality or summary-quality eval (grep `wer|word error|eval`
  in `crates/meeting/{src,tests,examples}`: no matches); 53's "no benchmarking" decision (`projects/wth-meeting-assistant`).
- **No golden-audio end-to-end test** (start → live briefs → post → summary) in CI; the README's integration pass is a
  manual dogfood list (`docs/design/meetings/README.md#L127-L182`). No CI at all on Sky's Mac (knowledge
  `facts/sky-mac-build-toolchain-state-2026-10-07`: `.github` absent).
- **Stale docs:** `docs/design/58-cross-platform.md` and `cross-platform/recon-desktop-meetings.md` are untracked
  (`git status`: `?? docs/design/58-cross-platform.md`) and their "macOS-only" claim is outdated for capture; the 53
  doc stops at §4.7 (`docs/design/meetings/README.md#L24-L29`).
- **Repo traction:** private project — no stars/issues/PR community to compare.

**(c) Designs**

- README §4 integration pass — written as a manual checklist, not automated (`docs/design/meetings/README.md#L127-L182`).
- Evals — not designed.

## Map to the three questions

**Q1 — what theuth has that they likely do not / what they offer that theuth lacks (OURS view).**
- Has: the live agent loop (A5 items 1–11), cache-first briefs across meetings, the oracle gate, PTT asks, the
  capture-hidden island, live Sortformer labels reconciled with the post pass, meeting-as-session with harness tools,
  harvest and notebooks (strengths 1–10 above). Also a reviewed-correction pipeline with voiceprint proposals
  (`crates/meeting/src/post/mod.rs#L240-L254`) and enrollment from selected segments (`crates/meeting/src/transcript.rs#L334-L402`).
- Lacks: gaps 1–10 above, plus live voiceprint naming (`crates/meeting/src/diar.rs#L1-L9` uses no profiles), OS
  notifications on "summary ready", first-run onboarding, retention, encryption at rest, map-reduce summaries, and stale
  macOS-only gating in the tray (`crates/desktop/src/lib.rs#L382-L391`).

**Q2 — where a borrowed technique would land in theuth (the seams).**
- Auto-detect / calendar start → a sensor or schedule (`when:` routines) calling `meeting_start` with `context`/`title`
  (`crates/meeting/src/lib.rs#L621-L646`); attendee names → `SpeakerLabel.proposed` (`crates/meeting/src/post/mod.rs#L240-L254`).
- MCP for external agents → extend `t mcp` (`crates/theuth/src/mcp.rs#L1-L6`) or a new stdio server proxying the
  existing `meeting_*`/`brief_*` RPCs (`crates/rpc/src/api.rs#L1723-L1834`, `crates/rpc/src/api.rs#L2103-L2126`).
- Templates → `form_for` (`crates/meeting/src/summary.rs#L29-L35`) choosing a user template instead of the constant
  (`crates/meeting/src/note.rs#L23-L34`), picked in the start form (`crates/desktop/ui/src/components/meeting/StartForm.tsx#L36-L41`).
- Local-only mode → `MeetingConfig.live_tools` per meeting (`crates/meeting/src/lib.rs#L559-L577`) + a local
  `session_model` (`crates/meeting/src/lib.rs#L639-L641`) via the Ollama adapter (`crates/models/src/lib.rs#L363`).
- Vocabulary → feed `voice::dictionary` (`crates/voice/src/dictionary/mod.rs#L1`) into the live and post text paths
  (`crates/meeting/src/stt.rs#L1-L5`, `crates/meeting/src/post/transcribe.rs#L1-L7`); languages → a multilingual live tier
  beside `nemotron-streaming-en` (`crates/voice/src/lib.rs#L64-L68`).
- Crash recovery → the plugin-start filter (`crates/meeting/src/lib.rs#L3781-L3790`): treat `Recording/Paused` with
  tracks on disk as stopped and run post.
- Markdown vault / frontmatter → the `summary.md` export (`crates/meeting/src/note.rs#L37-L40`) and the FTS rebuild
  (`crates/meeting/migrations/0002_segments.sql#L1-L2`).
- AEC off macOS → `capture/mic.rs` open path (`crates/meeting/src/capture/mic.rs#L38-L42`); device follow → the tap
  thread's `DeviceLost` branch (`crates/meeting/src/capture/system.rs#L64-L73`).

**Q3 — highest-leverage changes, from what exists today (input for the orchestrator's ranking).**
1. Meetings over MCP for external agents (cheap: the RPCs and tools exist; only a server is missing).
2. Auto-start + calendar context (detection sensor + attendees → title/context/speaker proposals).
3. Templates per meeting type on the existing note-first summary turn.
4. Local-only meeting mode with an on-screen disclosure line (model + web on/off).
5. Crash-safe recording recovery and device follow — recording "always takes precedence over intelligence" is Sky's rule.

## Cites self-check

Mechanical pass over every cite in this report: 348 `path#L` cites across 64 files — every file exists on main 2a1238f2 and every range lies inside the file (`wc -l`), 0 out of bounds. Then 35 cites re-opened with `read_file` and checked for the keyword the claim rests on (e.g. `Partial`, `MicNoAec`, `RTF 0.035`, `never rewrite`, `send_message`, `macOS only`, `WDA_EXCLUDEFROMCAPTURE`): 35 held, 0 corrected.

- `crates/meeting/src/capture/system.rs#L106-L116` — held · `crates/meeting/src/capture/mic.rs#L38-L42` — held
- `crates/meeting/src/lib.rs#L3781-L3790` — held · `crates/voice/src/stt.rs#L34-L36` — held
- `crates/voice/src/vad.rs#L108-L118` — held · `crates/meeting/src/post/diarize.rs#L112-L120` — held
- `crates/meeting/src/speakers.rs#L15` — held · `crates/meeting/src/post/mod.rs#L240-L254` — held
- `crates/meeting/src/note.rs#L21-L35` — held · `crates/meeting/src/summary.rs#L21-L27` — held
- `crates/meeting/src/lib.rs#L593-L608` — held · `crates/meeting/src/oracle.rs#L1-L20` — held
- `crates/meeting/src/tools.rs#L26-L57` — held · `crates/theuth/src/mcp.rs#L1-L6` — held
- `crates/meeting/migrations/0002_segments.sql#L1-L10` — held · `crates/desktop/src/lib.rs#L382-L391` — held
- `crates/desktop/src/island/mod.rs#L133-L136` — held · `crates/models/src/lib.rs#L363` — held
- `crates/agent/src/supervisor.rs#L9257` — held · `crates/meeting/src/lib.rs#L1732-L1736` — held
- `crates/desktop/companion/Sources/CompanionCore/KeyBindings.swift#L222-L244` — held · `crates/meeting/src/cache.rs#L1-L5` — held
- `crates/meeting/src/lib.rs#L586-L587` — held · `crates/document/src/ingest/mod.rs#L32-L49` — held
- `crates/desktop/ui/src/components/meeting/MeetingList.tsx#L61-L69` — held · `crates/meeting/src/lib.rs#L1118` — held
- `crates/desktop/src/island/mod.rs#L88-L92` — held · `crates/agent/src/lib.rs#L2136-L2146` — held
- `crates/meeting/src/stt.rs#L48-L56` — held · `crates/meeting/src/lib.rs#L4206-L4224` — held
- `crates/desktop/companion-windows/src/Theuth.Companion.Core/IslandModel.cs#L22-L59` — held · `crates/meeting/src/transcript.rs#L111-L156` — held
- `crates/rpc/src/api.rs#L1723-L1834` — held · `crates/meeting/src/lib.rs#L639-L641` — held
- `crates/desktop/ui/src/components/meeting/StartForm.tsx#L32-L33` — held

Not code-cited (knowledge or data, marked as such in the text): Sky's 2026-09-19 decisions (`projects/wth-meeting-assistant`), the Windows/Linux capture verification and build-B crash facts, the on-disk formats (read with a key-only script, no transcript or summary text read or quoted).
