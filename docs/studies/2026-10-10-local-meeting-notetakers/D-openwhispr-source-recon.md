# Report — THEIRS: OpenWhispr source recon (OpenWhispr/openwhispr @ 5f9a88b, MIT)

Read-only recon of `/Users/Sky.Moore/Develop/spike-meeting-notetakers/openwhispr` (shallow clone, HEAD 5f9a88b, 2026-10-10). Cites are repo-relative `path#L<a>-L<b>`. Techniques only — no code is copied.

## Executive summary

OpenWhispr (Electron, MIT, 9.2k★) grew from dictation into meetings + notes + an agent: strong audio plumbing, passive AI during meetings. Most worth borrowing:
1. Process-attributed mic-in-use on macOS (Core Audio process objects, `IsRunningInput`, CoreSpeech ignored) as the meeting trigger — `resources/macos-mic-listener.swift#L106-L160`.
2. Signal ranking: mic activity triggers, running app only corroborates, last app exit ends; silence/ownership auto-end windows — `src/helpers/meetingDetectionEngine.js#L88-L135`, `src/helpers/meetingAutoEndController.js#L2-L19`.
3. WebRTC AEC3 sidecar using system audio as the far-end reference — `native/meeting-aec-helper/src/aec_processor.cc#L31-L46`.
4. Correlation-based echo-leak classifier that suppresses mic segments that are only bleed — `src/helpers/meetingEchoLeakDetector.js#L7-L36`.
5. Cross-platform system audio: CATap + private aggregate (macOS), process loopback EXCLUDE-self (Windows), PipeWire portal (Linux) — `resources/macos-audio-tap.swift#L48-L131`, `resources/windows-system-audio-helper.c#L249-L280`.
6. Voiceprint match = threshold 0.65 AND margin 0.03 over the runner-up; profiles EMA 0.3/0.7 — `src/helpers/liveSpeakerIdentifier.js#L695-L747`, `src/helpers/database.js#L5940-L5960`.
7. Calendar attendees restrict voiceprint candidates; 1:1 meetings auto-enroll the other person — `src/helpers/ipcHandlers.js#L7900-L7945`.
8. Diarization cluster threshold ramps 0.55 → 0.7 with meeting length; phantom/collapse guards — `src/helpers/diarizationPolicy.js#L11-L44`.
9. Hybrid FTS5 + Qdrant search fused with RRF K=60, score floor 0.3, FTS fallback — `src/helpers/ipcHandlers.js#L2281-L2341`.
10. Notes prompts: source-fidelity rule, speaker-labels-only identities, summary-only inputs for follow-up actions, sectioned templates — `src/helpers/builtinActions.js#L366-L386`, `src/helpers/templatePrompts.js#L29-L126`.
11. Token-guarded loopback bridge + CLI + SKILL.md so external agents use the app — `src/helpers/cliBridge.js#L10-L195`, `agent-skills/openwhispr-cli/SKILL.md#L1-L40`.


## A1 Capture and meeting detection

**Start triggers.** Three paths converge in one engine, `MeetingDetectionEngine` (`src/helpers/meetingDetectionEngine.js#L87-L136`):
(1) *device activity* — a native mic listener reports sustained input use and the engine raises a "meeting detected" notification (`#L117-L119`);
(2) *calendar* — an event within `IMMINENT_THRESHOLD_MS = 5 min` is treated as imminent (`src/helpers/meetingDetectionEngine.js#L15-L18`), with join-from-calendar (`joinCalendarMeeting`, `#L605-L657`) and manual start (`startManualMeeting`, `#L537-L584`);
(3) *process presence* is deliberately **context only** — "a running app never prompts by itself (FaceTime idles in the background), but it corroborates device activity the mic detector could not attribute to a process" (`src/helpers/meetingDetectionEngine.js#L88-L98`).
- When the last tracked meeting app exits during a recording, auto-end is asked to stop (`#L100-L115`).

**Process detector.** macOS subscribes to `NSWorkspaceDidLaunch/TerminateApplicationNotification` via Electron `systemPreferences.subscribeWorkspaceNotification` and maps bundle ids (`us.zoom.xos`, `com.microsoft.teams[2]`, `com.cisco.webexmeetingsapp`, `com.apple.FaceTime`) to keys (`src/helpers/meetingProcessDetector.js#L7-L20`, `#L88-L112`), with an initial scan by process name (`#L134-L155`);
- Windows/Linux poll every `POLL_INTERVAL_MS = 30 s` against image names such as `cpthost.exe` (Zoom) and `ms-teams_modulehost.exe` (`#L5`, `#L22-L31`, `#L156-L174`).
- A generation counter discards results from a superseded run (`#L129-L133`).
- Browsers have no process key (`#L7-L31`), so a Meet-in-Chrome call can only be caught by the device-activity path (inference from the maps, not traced).

**Mic-in-use detection (macOS).** A Swift helper reads Core Audio *process objects* and listens on `kAudioProcessPropertyIsRunningInput` per process, so it can attribute input to a bundle id; it ignores `com.apple.CoreSpeech` (Siri) (`resources/macos-mic-listener.swift#L106-L160`, `#L185-L198`, `#L224-L255`), and falls back to the device-level `kAudioDevicePropertyDeviceIsRunningSomewhere` aggregate (`resources/macos-mic-listener.swift#L388-L410`).
- Windows has its own `resources/windows-mic-listener.c`.

**Auto-end.** `MeetingAutoEndController` constants: `SILENCE_WINDOW_MS = 60 s`, `FAST_SILENCE_MS = 10 s`, ownership confirm `5 s`, ownership min active `30 s`, tick gap `5 s` (`src/helpers/meetingAutoEndController.js#L2-L19`); engine ticks every `AUTO_END_TICK_MS = 1000` (`src/helpers/meetingDetectionEngine.js#L16`).
- Inputs: external-mic state (is the *meeting app* still holding the mic?) and meeting-process exit (`src/helpers/meetingDetectionEngine.js#L100-L135`).

**System audio per OS — all via small native sidecars speaking framed PCM on stdout:**
- macOS: `macos-audio-tap.swift` creates a Core Audio **process tap** (`CATapDescription` with `processes = []`, mono, mixdown, private, `muteBehavior = .unmuted`) then a private aggregate device holding only that tap (`resources/macos-audio-tap.swift#L48-L62`, `#L114-L131`); it installs invalidation listeners and reports device death as an event (`#L170-L260`).
     - Node spawns it with `--sample-rate 24000 --chunk-ms 100`, start timeout 3 s (`src/helpers/audioTapManager.js#L12-L16`, `#L126-L140`) and checks the Mach-O CPU type against `process.arch` before spawning (`src/helpers/audioTapManager.js#L427-L450`).
- Windows: `windows-system-audio-helper.exe` uses **WASAPI process loopback** (`VAD\Process_Loopback`, Windows 10 2004+) in `EXCLUDE_TARGET_PROCESS_TREE` mode — everything except OpenWhispr itself — activated through `ActivateAudioInterfaceAsync` (`resources/windows-system-audio-helper.c#L1-L10`, `#L61-L83`, `#L249-L280`); the manager comments that this hears every app on every device, unlike Chromium display-media loopback (`src/helpers/windowsLoopbackAudioManager.js#L6-L16`).
- Linux: `linux-system-audio-helper.c` negotiates a **PipeWire** stream (via the desktop portal; `portalVersion` is probed) (`resources/linux-system-audio-helper.c#L635-L724`, `src/helpers/linuxPortalAudioManager.js#L6-L8`, `#L271-L281`).
- Fallback mode `loopback` (Chromium display-media) exists; the live-speaker policy only trusts `native` or Windows/Linux loopback (`src/helpers/liveSpeakerIdPolicy.js#L10-L11`).
     - A watchdog and a handover module cover a stalled tap or a device switch (`src/helpers/meetingSystemAudioWatchdog.js`, `src/helpers/meetingSystemAudioHandover.js`; 236 + 50 lines, not read line by line).

**Echo cancellation — two layers.**
1. A native **WebRTC AEC3** sidecar `meeting-aec-helper` (C++, CMake): `AudioProcessing` with echo canceller + high-pass + moderate noise suppression, *no* AGC or transient suppression, `EchoCanceller3Factory` (`native/meeting-aec-helper/src/aec_processor.cc#L31-L46`).
     - Node speaks a length-prefixed frame protocol tagging streams `SYSTEM_STREAM = 1` / `MIC_STREAM = 2` at 24 kHz (`src/helpers/meetingAecManager.js#L6-L10`, `#L216-L248`).
     - The helper binary is downloaded at build (`npm run download:meeting-aec-helper`, `package.json#L26`).
2. A JS **echo-leak detector** that cross-correlates mic against the last 6 s of system audio over lags up to 500 ms (step 120 samples) and classifies bleed as strong/partial/final/VAD-gated/double-talk with explicit thresholds (e.g. strong: correlation ≥ 0.75, residual ≤ 0.32, explained ≥ 0.65, mic/system ratio 1.25) (`src/helpers/meetingEchoLeakDetector.js#L7-L36`); live mic transcript segments that fall in a bleed window are suppressed (`src/helpers/ipcHandlers.js#L7913-L7914`).
     - A mic holdback/gate delays mic chunks so the decision can be made before text is emitted (`src/helpers/meetingMicHoldback.js`, `src/helpers/meetingMicGate.js`).

**Permissions/TCC.** Dev builds spawn the calendar helper through `macos-disclaim-exec`, which calls the private `responsibility_spawnattrs_setdisclaim` so the helper becomes its own TCC responsible process and its embedded usage strings apply (`resources/macos-disclaim-exec.c#L1-L35`).
- Hooks exist for mic, screen-recording and system-audio permission state (`src/hooks/usePermissions.ts`, `src/hooks/useScreenRecordingPermission.ts`, `src/hooks/useSystemAudioPermission.ts`).
- Entitlements: `resources/mac/entitlements.mac.plist`.

**Crash safety and storage.** The renderer spools recorded chunks into IndexedDB `openwhispr-recording-spool`, store `chunks` (`src/helpers/recordingSpool.js#L9-L10`), so an interrupted recording can be recovered on relaunch; audio files are kept by `audioStorage.js` and pruned by a retention resolver (`src/helpers/retentionSettings.js#L1-L19`).
- A `MeetingRecordingSession` + `meetingTranscriptPersistence.ts` persist transcript segments as they arrive (`src/helpers/meetingRecordingSession.ts`, `src/helpers/meetingTranscriptPersistence.ts#L1-L37`).

**Media.** While recording/dictating, playing media is paused: macOS through the vendored `mediaremote-adapter` (Perl entry + `MediaRemoteAdapter.framework`, because MediaRemote is closed to unprivileged Mach-O processes on recent macOS), Linux MPRIS, Windows GSMTC (`src/helpers/mediaPlayer.js#L96-L106`, `#L196-L246`).
- That is what `resources/mediaremote-adapter/` is for — not capture.

**Device changes.** `activeMicRecovery.js`, `micSelectionRecovery.js`, `staleMicDevice.js`, `micTrackHealth.js` (211/—/—/117 lines) recover a dead or switched mic track; not read in detail.


## A2 Transcription

**Dictation (out of scope, one paragraph).** Dictation is the product's origin: a global hotkey (macOS Globe-key Swift listener, Windows/Linux C key listeners, GNOME/KDE/Hyprland portal shortcuts) records, transcribes, optionally runs a "cleanup" or "agent" prompt, and pastes via native fast-paste helpers; a text monitor watches the user's later corrections to auto-learn dictionary words (`package.json#L11-L25` compile scripts; `src/config/prompts/registry.ts#L11-L35` prompt kinds `cleanup`, `dictationAgent`, `translate`, `chatAgent`; `src/helpers/textEditMonitor.js`, `src/helpers/autoLearnSetting.js`).
- Not analysed further.

**Engines.** Local: **whisper.cpp `whisper-server`** sidecar on `127.0.0.1:8178–8199`, startup timeout 30 s (Vulkan 120 s), health check 5 s, threads default 4 / auto ≤ 12 at 0.75 × cores / manual ≤ 64 (`src/helpers/whisperServer.js#L23-L35`, args `#L162`);
- Silero VAD is passed to whisper-server (`--vad --vad-model --vad-threshold --vad-min-speech-duration-ms …`) (`#L195-L208`) with a comment that dictation turns VAD off (it would leave near-silence decoding seeded with the dictionary prompt) while notes and meetings keep it to skip long silence (`src/helpers/whisperVadConfig.js#L3-L4`, `#L26-L28`).
- **Parakeet/Nemotron/Cohere via sherpa-onnx**: 16 kHz float32, offline segments ≤ 15 s (Cohere 30 s), cache-aware *online* streaming models up to 600 s per stream, silence RMS floor 0.001, default model `parakeet-tdt-0.6b-v3` (`src/helpers/parakeetServer.js#L24-L33`, `#L115`); catalogue: Parakeet TDT 0.6B, Parakeet Unified EN 0.6B, Nemotron Speech Streaming EN 0.6B, Nemotron 3.5 ASR Streaming 0.6B, Cohere Transcribe 2B (`src/models/modelRegistryData.json#L4-L93`).
- A WebSocket server (`parakeetWsServer.js`, 630 lines, not read line by line) serves the streaming path.
Cloud (BYOK or OpenWhispr cloud): meeting streaming providers `openai`, `assemblyai`, `deepgram`, `corti`, `tinfoil` (`src/helpers/meetingTranscriptionRouting.js#L9-L13`); local routing picks Parakeet for `nvidia`, Cohere for `cohere`, else Whisper `base` (`#L45-L54`).
- Plus OpenAI Realtime, Gemini Live, AssemblyAI/Deepgram streaming clients (`src/helpers/*Streaming.js`), a self-hosted / custom ASR shim (`examples/custom-asr-shim/`, `src/helpers/selfHostedTranscription.js`).

**Live vs final.** Meetings transcribe live per channel (mic and system as separate streams; `meetingRecordingStore.ts` keeps both, `src/stores/meetingRecordingStore.ts#L123-L132`), then post-meeting diarization re-assigns system-side speakers (A3).
- Cloud batch upload is chunked with 3 attempts, global concurrency 5, exponential backoff 5 s ×3 capped 45 s ± 1 s jitter, and abandons if > 50 % of chunks are lost (`src/helpers/cloudChunkPolicy.js#L16-L42`).

**Hardware.** macOS gets a prebuilt `whisper-server-darwin-arm64|x64` from their own release zips (`scripts/download-whisper-cpp.js#L25-L33`; Metal use is whisper.cpp's Apple default, not confirmed in this repo);
- CUDA and Vulkan whisper/llama binaries are separate downloads managed on demand, disk check ×2.5 (`src/helpers/gpuBinaryManager.js#L19-L20`; `src/helpers/whisperCudaManager.js`, `whisperVulkanManager.js`, `llamaVulkanManager.js`);
- Vulkan device lines are parsed from whisper-server stderr (`src/helpers/whisperServer.js#L221`), and a GPU server that dies mid-request is treated as a crash with CPU fallback (`#L261`).

**Quality/eval.** No WER/RTF numbers in code.
- A weekly **STT canary** (GitHub Actions, Mondays 06:30 UTC) probes every keyed provider's real auth + endpoint with half a second of silence and a canary phrase, failing the job when a provider changes (`scripts/stt-canary.mjs#L1-L16`, `#L46-L49`, `#L143-L144`; `.github/workflows/stt-canary.yml#L10`).
- Custom vocabulary = the dictation dictionary, also passed as the Whisper prompt (A2 comment above).
- Hallucination guard beyond VAD: Not found (searched `hallucinat` in `src/helpers`).


## A3 Speakers

**Me vs them.** Channels decide: mic = the user, system audio = everyone else; diarization and live identification run on the system stream only (`src/helpers/liveSpeakerIdPolicy.js#L10-L11`; session max speakers = expected count − 1, i.e. minus the user, `src/helpers/ipcHandlers.js#L7929-L7933`).

**In-person vs call.** A session records two raw PCM captures (system tap and mic); at stop exactly one feeds the diarizer and the other is deleted: any session that heard audible system audio diarizes the system channel, otherwise (an in-person meeting) the mic channel is diarized (`src/helpers/meetingDiarizationInput.js#L1-L32`).
- Dictation audio, by contrast, is kept as `.webm` per transcription (`src/helpers/audioStorage.js#L34-L37`).

**Live identification (during the meeting).** `LiveSpeakerIdentifier` runs Silero VAD (window 512 @ 16 kHz, speech 0.15 / silence 0.08, 24 silent windows end a segment) and computes a CAM++ embedding once ≥ 1.6 s of speech exists, re-identifying every 1.0 s (`src/helpers/liveSpeakerIdentifier.js#L33-L50`).
- New embeddings are matched first against this meeting's transient speakers, then against stored profiles, both with `MATCH_THRESHOLD = 0.65` **and** `MATCH_MARGIN = 0.03` over the runner-up (`src/helpers/liveSpeakerIdentifier.js#L48-L49`, `#L695-L747`).
- Inference is in a worker (`onnxruntime-node`, 80-mel fbank 25/10 ms, ≤ 8 s per embedding) (`src/workers/onnxWorker.js#L20-L31`, `#L62-L66`, `#L192-L197`).

**Post-meeting diarization.** A bundled `sherpa-onnx-diarize-<platform-arch>` binary (`src/helpers/diarization.js#L75-L90`) with pyannote **segmentation-3.0** + 3D-Speaker **CAM++ (voxceleb, 16 kHz)** + Silero VAD, downloaded from the sherpa-onnx releases (`src/helpers/diarization.js#L54-L64`); ~37 MB of models, disk check ×2.5 (`#L142-L143`); 60-minute timeout (`#L24`).
- Policy (`src/helpers/diarizationPolicy.js#L11-L44`): cluster threshold **0.55**, ramping to **0.7** between 15 and 60 min of audio (long meetings over-split otherwise); clusters under 1 s total dropped; "phantom" clusters (≤ 10 % share and ≤ 3 s mean segment) folded; the 2 largest clusters protected; a "collapsed" result (top cluster ≥ 93 % with mean segment ≤ 2.2 s) is detected.
- After merge, a 6 s context window may merge up to 3 neighbours (`src/helpers/diarization.js#L25-L26`).

**Voiceprints across meetings.** SQLite tables: `speaker_profiles(display_name, email, embedding BLOB, sample_count)`, `speaker_mappings(note_id, speaker_id → profile_id, display_name)`, `note_speaker_embeddings(note_id, speaker_id, embedding)` (`src/helpers/database.js#L984-L1015`).
- Embedding = 512-dim, centroid of up to 3 segments per cluster, segments ≥ 1.5 s, ≤ 8 s (`src/helpers/speakerEmbeddings.js#L7-L22`, `#L160-L194`, `#L247-L262`).
- Re-confirming a profile blends **0.3 × new + 0.7 × stored** and bumps `sample_count` (`src/helpers/database.js#L5940-L5960`); profiles can be merged (winner keeps, loser deleted, `#L6040-L6059`) and are keyed by e-mail when known (`#L5917-L5935`).

**Calendar attendees name speakers.** For a calendar-linked meeting, only profiles whose e-mail is among the event's attendees are candidates (`src/helpers/ipcHandlers.js#L7900-L7912`).
- In a **1:1** meeting the single remote voice is bound to the other attendee automatically and a profile is upserted from the live embedding — enrollment without any UI (`src/helpers/ipcHandlers.js#L7916-L7945`).

**Correction UI.** The note editor loads profiles (`src/components/notes/NoteEditor.tsx#L525-L535`) and offers suggested-profile states (`src/utils/transcriptSpeakerState.ts#L35-L45`, `src/helpers/speakerAssignmentPolicy.js#L15-L30`);
- `NoteParticipants.tsx` shows the attendee list;
- `speakerMerge.js` (110 lines) merges labels.
- Labels flow into the transcript text that feeds notes/chat (`src/helpers/transcriptFormatter.js`, 212 lines).


## A4 Notes, summaries and the LLM layer

**Note model.** A note has three fields kept apart: the user's personal `content`, the AI `enhanced_content` (summary), and the transcript as *source material, not an editable field* — the chat context literally says so (`src/hooks/useEmbeddedChat.ts#L90-L111`; FTS covers all three, `src/helpers/database.js#L409-L417`).
- Editor = **TipTap** (`package.json#L154-L161`: core, bubble/floating menu, mention, table, task-item…).

**Templates and actions (who writes them).** Built-ins are seeded on startup; a row is upgraded only while its prompt still equals a previous default, so a user's edit is never overwritten (`src/helpers/builtinActions.js#L1-L18`, `#L477-L500`).
- Templates: Detailed Notes, AI Summary (the default "Generate Notes"), Follow-up email, Create to-dos, Make shorter/longer, TL;DR, Create outline, Slack update, 1:1, Stand-up, Sales call, User interview (`src/helpers/builtinActions.js#L484-L610`).
- A template may be **sectioned**: a list of `## heading` + per-section instruction compiled into the prompt under a fixed format and footer, with the template's free text as "TEMPLATE INSTRUCTIONS (follow these where they differ…)" (`src/helpers/templatePrompts.js#L29-L78`).
- Actions are either *summary actions* (rewrite the existing summary) or *chat actions* (sent as a user turn into the note chat) (`#L80-L95`).

**The prompts.** Every generation starts with a shared `SOURCE_FIDELITY_RULE` — preserve meaning, keep uncertainty/negation/status ("proposed, not yet authorized") (`src/helpers/sourceFidelity.js#L1-L3`).
- The meeting system prompt: no preamble/title/attendee list, never reproduce the Meeting Context block, refer to people **only by transcript speaker labels and never invent identities**, 1–2-sentence summary, then `## Key Discussion Points / ## Decisions Made / ## Action Items / ## Follow-ups` (omit empty), action items as `- [ ] Action — Owner`, keep meaningful quotes verbatim, integrate the user's manual notes as emphasis (`src/helpers/builtinActions.js#L366-L386`).
- The input layout preamble explains speaker-label prefixes (`#L390-L395`).

**Merging typed notes with the transcript ("enhance").** `buildNoteRunInput` assembles `notes + meetingContext + ## Meeting Transcript`; an action that can work from an existing summary gets `## Current Summary + ## My Notes + meeting context` and **no transcript**, "so the request always fits and is never split" (`src/helpers/templatePrompts.js#L97-L126`).

**Long transcripts.** Token estimate at 3 Latin chars/token with CJK ranges (`src/helpers/noteChunking.js#L12-L34`);
- `planNoteChunks(body, budget, {preserveSpeakerLabels})` splits along the transcript and `splitChunkInHalf` re-splits a chunk that still fails (`#L108-L135`) — a map-reduce over chunks for small local contexts.

**Local LLM.** Bundled **llama.cpp `llama-server`** on `127.0.0.1:8221–8240`, startup 120 s, idle unload 5 min, health failure threshold 3, `--jinja`, default ctx 4096 with a "context step-down rung" that rewrites `--ctx-size` and retries (`src/helpers/llamaServer.js#L15-L35`, `#L218-L227`).
- Context sizing is a pure policy that reads KV-bytes-per-token from GGUF metadata and caps at 50 % of memory minus a 2 GiB OS reserve, baseline 16,384 — written after an unbounded context allocated ~15 GB (#1203) (`src/helpers/llamaContextPolicy.js#L1-L18`, `#L62-L128`; `src/helpers/ggufMetadata.js`).
- Local catalogue includes Qwen3.5 9B/4B/2B and Qwen3 8B Q4_K_M (`src/models/modelRegistryData.json#L871-L916`).
- Cloud: OpenAI, Anthropic, Gemini, Groq, Mistral, LAN (OpenAI-compatible on the network), OpenWhispr cloud, Tinfoil, Enterprise (Bedrock/Azure) (`src/services/ai/inferenceProviders/` file list).
- Reasoning-model `<think>` output is filtered while streaming (`src/services/ai/streamingThinkFilter.ts`, `thinkingSuppressionDialects.ts`).

**Regeneration/editing.** Titles are regenerable (`src/helpers/regenerableNoteTitle.js`); an assistant edit to a note is undoable (`src/components/notes/AssistantNoteUndoListener.tsx`, `src/helpers/noteUndo.js`); background action runs show toasts and a processing overlay (`src/components/notes/BackgroundActionToastListener.tsx`, `ActionProcessingOverlay.tsx`).
- Structured output/frontmatter for the summary: Not found (the summary is markdown; frontmatter exists only in the mirror files, A6).


## A5 Live assistance during the meeting

**What the user sees.** The meeting note opens with a live transcript rendered as a chat (me vs them bubbles), memoised so several partials per second only re-render the partial rows, with a recording/diarizing indicator (`src/components/notes/MeetingTranscriptChat.tsx#L607-L612`, `#L879-L925`); a small **recording pill** window (`MeetingRecordingPill.tsx`, 134 lines) and a realtime-transcription banner (`RealtimeTranscriptionBanner.tsx`).
- Live speaker names appear during the meeting from the live identifier (A3), retained 30 s and carried forward 8 s on the system channel (`src/stores/meetingRecordingStore.ts#L131-L132`).

**AI during the meeting.** Nothing proactive: no live topics, briefs, suggestions or running summary (searched `liveSummary|runningSummary|live suggestion|liveInsight|realtimeSummary` over `src/` — no hits).
- The user can *ask* the note chat at any time; it reads the in-progress transcript (preview only when over the token budget) (`src/hooks/useEmbeddedChat.ts#L90-L111`), and the dictation "agent" hotkey/tray item "Ask assistant" works during a meeting (`src/helpers/tray.js#L282-L290`).

**Screen-share hygiene.** Main, pill and other overlay windows call `setContentProtection(true)` (`src/helpers/windowManager.js#L200`, `#L2033`, `#L2253`) — the same intent as theuth's hidden island.

**Versus theuth.** theuth is ahead here: live TOPICS + tool-written BRIEFS on an always-on-top island and push-to-talk steering have no counterpart.
- OpenWhispr's live strengths are upstream of that — better audio (AEC3 + bleed detector) and live speaker *names* from voiceprints and calendar attendees.


## A6 Memory, search, agent access and integrations

**Storage.** One SQLite database (`better-sqlite3`-style synchronous API in `src/helpers/database.js`, 7,697 lines) holds `notes`, `transcriptions`, `folders`, `spaces`, `agent_conversations`/`agent_messages`, calendars + `calendar_events`, `contacts`/`contact_sources`, `speaker_*`, `connector_actions`, and analytics tables (`src/helpers/database.js#L290-L1495` — table list from the `CREATE TABLE` statements).
- Audio stays as files (`src/helpers/audioStorage.js`).

**Markdown mirror (Obsidian-style).** Every note is mirrored to a folder tree as `<id>-<slug>.md` with YAML frontmatter `id, title, type, folder, created, updated`, plus a separate `<id>-<slug>-transcript.md` with speaker-mapped segments (`src/helpers/markdownMirror.js#L131-L141`, `#L144-L166`, `#L186-L200`);
- Windows-reserved names are escaped with a `__ow-` prefix (`#L6-L7`); a scan recognises its own files by the leading `---` (`#L310-L321`).
- The mirror is one-way (written on save), not an editable source of truth (no file watcher found in this file).

**Full-text search.** FTS5 external-content table `notes_fts(title, content, enhanced_content)` kept in sync by insert/update/delete triggers and back-filled with `INSERT OR IGNORE` (`src/helpers/database.js#L409-L450`).
- Queries are tokenised with Unicode letter/number classes and every token becomes a quoted prefix term `"tok"*` (`src/helpers/noteSearch.js#L1-L14`) — safe against FTS syntax injection and good for CJK/diacritics.

**Semantic search.** Text embeddings = **all-MiniLM-L6-v2** ONNX (384-dim, ≤ 256 tokens) in the shared ONNX worker (`src/helpers/localEmbeddings.js#L7`, `#L55-L83`; `src/workers/onnxWorker.js#L30-L31`).
- Vectors live in a **bundled Qdrant** sidecar on `127.0.0.1` ports 6333–6350 with its own storage dir, 30 s startup timeout, health check every 5 s, restart after 8 failures, at most 3 restarts per session, and a `restarted(port)` event so clients re-wire (`src/helpers/qdrantManager.js#L16-L45`, `#L97-L117`).
- Collections `notes` and `conversation_chunks`, cosine, size 384 (`src/helpers/vectorIndex.js#L9-L33`, `#L95-L125`).
- Indexing is a background drain of a `pending_vector_changes` queue, 50 rows a page, 3 failures per row max, 30 s retry, and the model unloads after 5 min idle (`src/helpers/semanticSearchLifecycle.js#L1-L6`; `src/helpers/database.js#L4209`).
- Purges are journalled (`pending_vector_purges`, `src/helpers/database.js#L1495`).

**Hybrid ranking.** `db-semantic-search-notes` runs FTS5 and Qdrant in parallel with over-fetch (×2, ×4 when a folder is in scope), drops vector hits with score ≤ 0.3 or outside the SQLite-validated scope, then fuses with **Reciprocal Rank Fusion, K = 60** ("matching cloud implementation"), and falls back to FTS5 on any error (`src/helpers/ipcHandlers.js#L2281-L2341`).
- The agent's `search_notes` tool chains cloud → local hybrid → FTS5 (`src/services/tools/searchNotesTool.ts#L28-L31`, `#L69-L85`, `#L120-L130`).

**Chat / agent over notes.** The renderer-side agent registers tools conditionally: `search_notes`, `get_note`, `create_note`, `update_note` (omitted when the user sets "keep notes unchanged"), `list_folders`, `clipboard`, snippet/dictionary updaters, `web_search`, `get_calendar_events` + calendar availability, and connector tools (`src/services/tools/index.ts#L1-L64`).
- Connectors are first-party OAuth integrations for **Gmail, Slack, GitHub, Linear** (+ contact search and e-mail compose) with an approval queue for side-effecting actions (`src/helpers/connectors/` file list; `src/services/tools/connectors/runApprovalAction.ts`, `src/stores/connectorApprovalStore.ts`).
- Past agent conversations are stored and chunk-embedded (windows of 5 messages, overlap 2) for retrieval (`src/helpers/conversationChunker.js#L1-L22`, `src/helpers/vectorIndex.js#L108-L125`).
- Per-meeting chat lives in the note (`src/components/notes/MeetingTranscriptChat.tsx`, `EmbeddedChat.tsx`).

**The agent / voice assistant.** The default chat-agent prompt is a voice assistant ("Respond concisely and conversationally… You may be given a transcription of spoken input") (`src/config/prompts/registry.ts#L5-L8`).
- The tool loop runs up to `MAX_TOOL_STEPS = 20` (one step when no tools) and compacts transcript history before every step (`src/services/ReasoningService.ts#L121`, `#L1226-L1242`; `src/services/ai/transcriptHistory.ts`).
- When the assistant hotkey fires, the app captures the active display around the window the user was typing in (bounds from the text-edit monitor), scaled to ≤ 1,568 px on the long edge with a JPEG quality ladder 82 → 70 → 55 under 1.5 MB (`src/helpers/ipcHandlers.js#L5980-L5987`, `src/helpers/screenContextCapture.js#L1-L14`), and that screenshot rides every step of the tool loop "so the model keeps its vision after tool results come back" (`src/services/ReasoningService.ts#L1243-L1246`); the user's text selection is attached up to 6,000 characters (`src/utils/agentSelectionContext.ts#L1`).

**Agent access from outside (the Minutes-style axis).** No MCP server in the desktop code (grep `modelcontextprotocol|mcp` over `src/`, `main.js`, `package.json` hits only locale strings); the README points to a hosted MCP + public API (`README.md#L62`, `#L84`, README-only).
- Locally, the app runs a **loopback HTTP bridge** on `127.0.0.1:8200–8219` with a fresh 32-byte bearer token per launch, written with `{version, port, token}` to `~/.openwhispr/cli-bridge.json` mode 0600, loopback-address check, 1 MiB body cap (`src/helpers/cliBridge.js#L10-L20`, `#L111-L170`, `#L187-L195`).
- Routes: health, notes list/search/get/patch/delete, folders, dictionary, snippets, transcribe models + transcribe, transcriptions list/get/delete (+ audio delete) (`src/helpers/cliBridge.js#L384-L575`).
- A published CLI (`npm i -g @openwhispr/cli`) picks local bridge vs cloud API automatically, and **agent skills** (`agent-skills/openwhispr-cli/SKILL.md#L1-L40`, `agent-skills/openwhispr-api/SKILL.md`) teach Claude Code-style agents to drive it.

**Calendar.** Google: OAuth loopback flow with scopes `calendar.events.readonly` + `calendar.calendarlist.readonly` (`src/helpers/googleCalendarOAuth.js#L1-L12`); incremental sync via `syncToken`, 33-day lookahead, token TTL 24 h, 410 → full resync (`src/helpers/googleCalendarManager.js#L9-L17`, `#L174-L215`).
- Microsoft: Graph `calendarView/delta` with `Calendars.Read offline_access`, 38-day delta window, 7-day token TTL, series-master backfill retry 10 min (`src/helpers/microsoftCalendarOAuth.js#L10-L11`, `src/helpers/microsoftCalendarManager.js#L9-L23`, `#L201-L230`).
- Apple: EventKit via a Swift helper (`EKEventStore`, permission status, attendee `mailto:` e-mails, URL extraction) (`resources/macos-calendar-listener.swift#L24-L25`, `#L115-L210`; `src/helpers/appleCalendarManager.js`).
- What is pulled per event: summary, description, hangout/join link (or a URL extracted from location/description by host rules for Zoom/Meet/Teams/Webex), conference data, organizer, attendees with response status (`src/helpers/googleCalendarManager.js#L248-L278`, `src/helpers/meetingJoinUrl.js#L1-L15`).
- **How it is used:** attendees are upserted into a local `contacts` table (rooms and the user's own addresses excluded) (`src/helpers/googleCalendarManager.js#L280-L288`); joining an event creates (or resumes) a meeting note titled with the event summary, in the Meetings folder, with `participants = attendees` and `calendar_event_id` (`src/helpers/meetingDetectionEngine.js#L605-L651`); attendee e-mails restrict voice-profile matching (A3); a reminder fires `MEETING_REMINDER_LEAD_MS = 60 s` before start (`src/helpers/calendarReminderScheduler.js#L4`); the agent can read events and free/busy (`src/services/tools/index.ts#L60-L61`).

**Imports/exports.** Granola import (`src/helpers/granolaImport.js`, 438 lines); share dialog and spaces with members (team/cloud features, A8); markdown mirror as the export path.
- People/company pages: Not found (searched `src/components` for `person|company` page components; only `contacts` table + `findContactTool.ts`).


## A7 UX surfaces and onboarding

**Windows.** Several Electron windows with roles: a frameless always-on-top **main** voice window (the dictation pill), a **control panel** (the library/notes app; not always-on-top), a **notification** window (always-on-top, focusless overlay type on Linux compositors), an assistant window and the meeting recording pill (`src/helpers/windowConfig.js#L5-L45`, `#L137`, `#L180-L194`, `#L216-L271`); the pill docks 12 px from a screen corner (`#L128-L137`).
- Linux overlay type is chosen per compositor (Sway/XWayland, KDE, GNOME) (`#L14-L31`).

**Library.** Notes are organised in **spaces → folders** (personal + team spaces), with a default **Meetings** folder that calendar joins and detections write into, and a destination picker for where a detected meeting's note goes (`src/helpers/meetingNotificationDestination.js#L1-L20`; `src/components/notes/SpacesTree.tsx`, `SpaceGroupsSection.tsx`).
- Upload of an audio/video file or URL (yt-dlp is bundled: `package.json#L26`) creates a transcribed note (`UploadAudioView.tsx`, `src/helpers/urlAudioDownloader.js`); a batch queue view shows background jobs (`BatchQueueView.tsx`, `src/stores/batchQueueStore.ts`).

**Meeting page.** One note editor (TipTap) with the live transcript chat beside it; the side panel collapses under `SIDE_PANEL_BREAKPOINT_PX = 1024` (`src/stores/meetingRecordingStore.ts#L70`); participants strip (`NoteParticipants.tsx`), template picker and action chips (`TemplatePicker.tsx`, `ActionChips.tsx`), embedded chat (`EmbeddedChat.tsx`), a post-meeting "summary offer" (`test/components/meetingSummaryOffer.test.js` names it).
- States are tracked in `meetingRecordingStore.ts` (mic and system capture outcomes resolved in parallel with `Promise.allSettled`-style handling, `#L990-L996`) with tests for degraded / interrupted / silent system audio and unload flush (`test/stores/meetingRecordingStoreSystemAudio*.test.js`, `meetingRecordingStoreUnloadFlush.test.js`).

**Tray / hotkeys.** Tray menu: dictate, **Ask assistant**, **Start meeting recording** (started in the main process like the meeting hotkey), show/hide, quit (`src/helpers/tray.js#L273-L316`).
- Separate hotkeys for dictation, assistant and meeting (`src/helpers/hotkeyManager.js`, `test/components/meetingHotkeyClear.test.js`).

**Detection notification.** A detected meeting raises an in-app notification card (not an OS toast) with a queue: a card that vanishes without an answer releases only its own detection; an expiry settles all pending ones (`src/helpers/meetingDetectionEngine.js#L653-L716`).

**Onboarding.** Account route: `auth → permissions → languages → use-cases → dictation-hotkey → dictation-demo → notes`; full canonical order adds `required-models`, `activation-mode`, `assistant-hotkey`, `assistant-demo`, `setup-choice`, then BYOK or local model steps (`src/components/onboarding/flow.ts#L115-L149`).
- Calendar connection is a notes-step sub-flow (`CalendarConnectionsStep.tsx`; `flow.ts#L114`).
- A permission-guide overlay points at System Settings rows (`PermissionGuideOverlay.tsx`, `src/helpers/permissionGuideManager.js`).

**Mobile companion (one paragraph).** `openwhispr-mobile/` is an Expo 55 / React Native 0.83 app using `whisper.rn` for on-device Whisper, RevenueCat (`react-native-purchases`) for subscriptions and **AppsFlyer** (`react-native-appsflyer`) for install attribution, with drizzle (SQLite) and a StoreKit config (`openwhispr-mobile/package.json#L2`, `#L48`, `#L87-L99`; directory listing).
- Not analysed further.


## A8 Privacy, local-only guarantees and business model

**Local by default?** It is a choice at onboarding: "setup-choice" → cloud (OpenWhispr account), BYOK or local (`src/components/onboarding/flow.ts#L125-L149`).
- The README says "choose between fully private offline transcription with local speech-to-text models" or cloud (`README.md#L17`, `#L30`, README-only).
- With local chosen, transcription (whisper.cpp/sherpa-onnx), diarization (sherpa-onnx), embeddings (MiniLM ONNX), vector index (Qdrant) and LLM (llama-server) all run as local sidecars bound to `127.0.0.1` (A2/A4/A6 cites).

**What leaves the machine.** Calendar OAuth syncs (Google/Microsoft, read-only scopes) are direct device → provider (`src/helpers/googleCalendarOAuth.js#L4-L7`, `src/helpers/microsoftCalendarOAuth.js#L10-L11`); connectors (Gmail/Slack/GitHub/Linear) only when connected; cloud transcription/LLM only when that provider is configured; the agent's `web_search` when enabled (`src/services/tools/index.ts#L56`).

**Telemetry claim — verified for the desktop app, with a nuance.** No third-party analytics/crash SDK in the desktop tree: grep for `posthog|sentry|mixpanel|amplitude|segment.io|plausible|datadog` over `src/`, `main.js`, `package.json` returns only false positives (`sEntry` in `ControlPanel.tsx`, the word "Amplitude" in `localSpeechGate.js`).
- There *is* first-party usage analytics: an `analytics_events` table written locally (`src/helpers/database.js#L1201-L1227`, inserts `#L1720`, `#L1837`) that powers a local "Insights" view and is uploaded to the OpenWhispr API in batches of 200, ≤ 5 batches per pass, **only for a signed-in account that granted Insights consent**, re-checking consent between batches (`src/services/AnalyticsService.ts#L16-L35`, `#L198-L235`; consent coalescing `src/helpers/insightsConsentCoordinator.ts#L13-L40`).
- A leaderboard is opt-in (`src/services/LeaderboardService.ts`, `src/stores/leaderboardParticipationStore.ts`).
- The **mobile** app does ship AppsFlyer attribution (`openwhispr-mobile/package.json#L88`).

**Accounts and paid tiers.** Cloud account (email discovery, OAuth, billing portal, usage meter) in `src/lib/auth.ts`, `src/lib/workspaceBilling.ts`, `src/hooks/useUsage.ts`, `src/lib/upsell.ts`.
- Team **spaces**, sharing, members and invitations are cloud features (`src/services/SpacesService.ts`, `NoteSharingService.ts`, `InvitationsService.ts`); cloud semantic search is the first strategy for `search_notes` when signed in (`src/services/tools/searchNotesTool.ts#L69-L85`).
- Exactly which features are paywalled is decided server-side (policy store), so not provable from the client: `src/stores/policyStore.ts`, `src/helpers/workspacePolicyManager.js`.

**Enterprise.** README: "enforce organization policy, company SSO and SCIM, and centrally managed Amazon Bedrock or Azure OpenAI access without distributing cloud keys" (`README.md#L61`, README-only).
- In code: managed config validates provider `bedrock|azure`, modes `disabled | managed_default | managed_required` per inference scope (`dictationCleanup, dictationAgent, noteFormatting, chatIntelligence, dictationTranslation`) plus a managed transcription scope, AWS role-ARN/region and Azure version patterns (`src/helpers/enterpriseManagedConfig.mjs#L1-L17`); identity manager and policy headers on requests (`src/helpers/enterpriseIdentityManager.js`, `src/helpers/policyRequestHeaders.js`).

**Confidential cloud option.** **Tinfoil** provider: attested-enclave inference — the SDK's `SecureClient` verifies the enclave and pins the TLS connection to the attested key, one client per session (`src/helpers/tinfoilSecureClient.js#L1-L14`).
- Secrets at rest: `src/helpers/secretCrypto.js`, `src/helpers/tokenStore.js` (not read).

**Licence.** MIT (`LICENSE`).


## A9 Engineering, platforms and traction

**Architecture.** Electron main (`main.js`) + preload + React renderer; a 7,697-line SQLite manager and a 13,029-line `ipcHandlers.js` (main.js 2,135 lines) are the backbone (`src/helpers/database.js`, `src/helpers/ipcHandlers.js`).
- Heavy work is pushed into **native sidecars** spoken to over stdio/HTTP: macOS Swift helpers (audio tap, mic listener, calendar listener, globe key, fast paste, media remote, window bounds, text monitor), Windows C helpers (system audio, mic listener, key listener, fast paste, text monitor), Linux C helpers (system audio, key listener, fast paste, text monitor), the C++ WebRTC AEC helper, and downloaded binaries whisper-server, llama-server, sherpa-onnx, qdrant, yt-dlp (`package.json#L11-L37`; `resources/` listing).
- Several helpers are versioned and released independently (e.g. `windows-system-audio-helper-v1.2.0`, `windows-mic-listener-v1.1.0`) with their own build workflows (`.github/workflows/build-*.yml`).
- A sidecar registry + PID files + a reaper exist for orphaned helpers after a crash (by file names `src/helpers/sidecarRegistry.js`, `sidecarPidFile.js`, `sidecarReaper.js`; not read).
- ONNX inference (speaker + text embeddings, VAD) runs in a worker thread with a generation counter so a restarted worker invalidates cached loads (`src/workers/onnxWorker.js#L62-L66`; `src/helpers/localEmbeddings.js#L55-L67`).

**Platforms per feature.** System audio: macOS process tap, Windows process loopback, Linux PipeWire portal (A1).
- Process detection: NSWorkspace on macOS, polling elsewhere (A1).
- Apple Calendar: macOS only (EventKit helper).
- GPU: Metal/CUDA/Vulkan (A2).

**Tests and evals.** 820 files under `test/` run by `node --import tsx --test "test/**/*.test.js"` (`package.json#L78`): helpers 522, components 101, services 66, utils 43, lib 27, hooks 22, stores 19 (directory counts).
- Meeting coverage is dense: 32 `test/helpers/*meeting*` files, 9 calendar files, echo-leak and echo-cancellation-gate tests (`test/helpers/meetingEchoLeakDetector.test.js`, `meetingEchoCancellationGate.test.js`), diarization policy / speaker count / speaker reconcile tests.
- Evals: `scripts/meeting-diarization-eval.js` compares an actual diarization against fixtures and counts placeholder speakers (`scripts/meeting-diarization-eval.js#L6-L30`, `#L150-L166`); weekly **STT and LLM canaries** in CI (`.github/workflows/stt-canary.yml#L10`, `.github/workflows/llm-canary.yml#L11`);
- `scripts/selection-edit-eval/`; CodeQL and lockfile-lint workflows.

**Diagnostics/updater.** `src/helpers/debugLogger.js` with per-area tags (`"meeting"` everywhere above), `DEBUG.md`, `TROUBLESHOOTING.md`; updater hook `src/hooks/useUpdater.ts`; release workflow builds + notarizes (`.github/workflows/build-and-notarize.yml`, `release.yml`); winget and nix update workflows.

**Traction (GitHub API via the GitHub tools, 2026-10-10).** 9,219 stars, 1,142 forks, 438 open issues+PRs, created 2025-06-19.
- App releases: v1.7.6 (07-18), v1.8.0/1 (07-30/08-13), v1.8.3 (08-13), v1.9.0 (08-24), v1.9.1 (08-27), v1.9.2 (08-29), v1.10.0 (09-11), v1.10.1 (09-14), v1.10.2 (09-15) — roughly weekly, plus separately tagged native helpers.
- Top issues by reactions are almost all dictation/platform: #459 Android support (11, open), #681/#707 Homebrew (10/8, closed), #62 modifier hotkeys (5), #1400 cancel in push-to-talk (5, open), #1100 Wayland D-Bus crash (5), #348 start minimised (5), #233 dual-language hotkeys (5, open), #1329 Niri/Wayland CLI toggle (5, open), #742 ~350 ms start latency via AudioWorklet pre-roll (4, open), #1207 reasoning-model cold start ~10 s (4).
- **No meeting/diarization issue is in the top 20** — meetings are newer than the dictation user base. (Unauthenticated REST was rate-limited; numbers came through the authenticated GitHub tools.)


## Map to the three questions

theuth facts below are taken from AXES.md's summary (the OURS analyst verifies them); OpenWhispr facts carry cites.

### Q1 What they offer that theuth does not — and the reverse
OpenWhispr has, theuth (per AXES.md) does not:
- **Meeting auto-detection and auto-end**: process-attributed mic-in-use (`resources/macos-mic-listener.swift#L106-L160`), NSWorkspace launch/exit as context only (`src/helpers/meetingDetectionEngine.js#L88-L115`), calendar imminence 5 min (`#L15`), silence/ownership auto-end windows (`src/helpers/meetingAutoEndController.js#L2-L19`).
- **Echo cancellation** between mic and system: WebRTC AEC3 sidecar (`native/meeting-aec-helper/src/aec_processor.cc#L31-L46`) plus a correlation bleed detector that suppresses echoed mic segments (`src/helpers/meetingEchoLeakDetector.js#L7-L36`, `src/helpers/ipcHandlers.js#L7913-L7914`).
- **Echo-cancelled capture on Windows and Linux.** AXES.md lists theuth capture as macOS, but theuth knowledge (`facts/theuth-meeting-capture-is-flexaudio-cross-platform`, 2026-09-23) says flexaudio already gives theuth system/process loopback on Windows (WASAPI) and Linux (PipeWire) with the mic marked `MicNoAec` — `Partial("no echo cancellation; headphones recommended")`.
     - OpenWhispr ships the missing piece: a portable AEC3 sidecar for every OS (`native/meeting-aec-helper/src/aec_processor.cc#L31-L46`) on top of process loopback EXCLUDE-self and a PipeWire portal stream (`resources/windows-system-audio-helper.c#L61-L83`, `resources/linux-system-audio-helper.c#L635-L724`).
     - OURS analyst to confirm theuth's current state.
- **Calendar integration** (Google/Microsoft/Apple): auto-titled meeting notes with attendees as participants, contacts, reminders, agent calendar tools (`src/helpers/meetingDetectionEngine.js#L605-L651`, `src/helpers/googleCalendarManager.js#L248-L288`, `src/helpers/calendarReminderScheduler.js#L4`).
- **Voiceprints recognised live, across meetings**, candidate set restricted to the event's attendees, automatic enrollment in 1:1s (`src/helpers/liveSpeakerIdentifier.js#L695-L747`, `src/helpers/ipcHandlers.js#L7900-L7945`).
- **Per-meeting-type templates** (1:1, stand-up, sales call, user interview…) with sectioned compile and one-click actions (follow-up e-mail, Slack update, to-dos) (`src/helpers/builtinActions.js#L484-L610`, `src/helpers/templatePrompts.js#L29-L78`).
- **Hybrid keyword + semantic search with RRF** across notes and past chats (`src/helpers/ipcHandlers.js#L2281-L2341`, `src/helpers/vectorIndex.js#L95-L125`).
- **Markdown mirror** of every note + transcript (`src/helpers/markdownMirror.js#L131-L200`); **Granola import**.
- **External-agent access** through a token-guarded loopback bridge + published CLI + SKILL.md files (`src/helpers/cliBridge.js#L10-L20`, `agent-skills/openwhispr-cli/SKILL.md#L1-L40`); hosted MCP per README (`README.md#L62`).
- **Connectors with approval cards** (Gmail, Slack, GitHub, Linear) inside note chat (`src/services/tools/index.ts#L64`, `src/helpers/connectors/`).
- **Enterprise managed providers** (Bedrock/Azure, per-scope disabled/default/required) and **attested-enclave inference** (Tinfoil) (`src/helpers/enterpriseManagedConfig.mjs#L1-L17`, `src/helpers/tinfoilSecureClient.js#L1-L14`); in-person meeting mode that diarizes the mic (`src/helpers/meetingDiarizationInput.js#L1-L32`);
     - Linux; a mobile app.

theuth has, OpenWhispr does not:
- **Live topics and tool-written briefs** on an always-on-top island, push-to-talk steering — OpenWhispr's AI is passive during meetings (A5: no `liveSummary|runningSummary|…` hits in `src/`).
- **A general agent harness** behind the meeting: subagents, a knowledge base the transcript is harvested into, a cross-meeting brief cache, schedules, an MCP *client*;
     - `meeting_read`/`meeting_search`/`meeting_speakers` tools.
     - OpenWhispr's agent is a renderer-side tool loop over notes/calendar/connectors (`src/services/tools/index.ts#L1-L64`).
- **Three separate Ogg Opus tracks kept per meeting**; OpenWhispr keeps raw PCM until stop and then deletes the channel it does not diarize (`src/helpers/meetingDiarizationInput.js#L4-L8`).
- **Segment split in the correction pass**: OpenWhispr has merge + suggested profiles; split Not found (searched `splitSpeaker|speakerSplit|splitSegment` in `src/`).

### Q2 What we can learn — techniques worth borrowing (cite → where it lands in theuth)
1. **Process-attributed mic-in-use** via Core Audio process objects + `kAudioProcessPropertyIsRunningInput`, ignoring `com.apple.CoreSpeech` (`resources/macos-mic-listener.swift#L106-L160`, `#L224-L255`) → Swift companion emits "meeting app X holds the mic" to `crates/meeting` for an auto-start prompt.
2. **Signals ranked: mic activity triggers, process presence corroborates, app exit ends** (`src/helpers/meetingDetectionEngine.js#L88-L135`) + auto-end windows 60 s / 10 s / 5 s / 30 s (`src/helpers/meetingAutoEndController.js#L2-L19`) → `crates/meeting` session lifecycle.
3. **AEC3 sidecar fed with system as far-end reference** (EC + HPF + moderate NS, no AGC) (`native/meeting-aec-helper/src/aec_processor.cc#L31-L46`; framing `src/helpers/meetingAecManager.js#L216-L248`) → mic track before live STT; theuth already records the system track to use as reference, and this is exactly what would clear theuth's `MicNoAec` / "headphones recommended" degradation on Windows and Linux (per theuth knowledge `facts/theuth-meeting-capture-is-flexaudio-cross-platform`).
4. **Echo-leak classifier** (≤ 500 ms lag cross-correlation, residual/explained thresholds, double-talk guard) to drop mic segments that are only bleed (`src/helpers/meetingEchoLeakDetector.js#L7-L36`) → live transcript de-duplication between "me" and "them".
5. **Match = threshold AND margin over runner-up** (0.65 / 0.03), re-ID every 1 s once 1.6 s of speech exists (`src/helpers/liveSpeakerIdentifier.js#L33-L50`, `#L695-L747`) → theuth speaker matching against enrolled profiles.
6. **Profile update as EMA 0.3 new / 0.7 old with sample_count; e-mail-keyed profiles; merge keeps winner** (`src/helpers/database.js#L5917-L5960`, `#L6040-L6059`) → theuth speaker profile store.
7. **Attendee-restricted candidates + 1:1 auto-enroll** (`src/helpers/ipcHandlers.js#L7900-L7945`) → needs calendar; the cheapest big win for naming speakers.
8. **Duration-ramped clustering threshold 0.55 → 0.7 over 15–60 min, phantom folding, collapse detection** (`src/helpers/diarizationPolicy.js#L11-L44`) → theuth post-meeting diarization (same CAM++ family, so thresholds are directly comparable).
9. **Hybrid search**: FTS5 prefix-quoted tokens (`src/helpers/noteSearch.js#L1-L14`), over-fetch ×2/×4, vector floor 0.3, scope re-validation, **RRF K = 60**, FTS fallback (`src/helpers/ipcHandlers.js#L2281-L2341`) → `meeting_search`, `knowledge_search`, `nb_search`.
10. **Prompt rules**: shared source-fidelity rule (`src/helpers/sourceFidelity.js#L1-L3`), "people only by speaker labels, never invent identities", empty sections omitted, `- [ ] Action — Owner` (`src/helpers/builtinActions.js#L366-L386`) → theuth meeting summary/note prompt.
11. **Summary-only inputs for follow-up actions** (no transcript → never split) (`src/helpers/templatePrompts.js#L97-L126`) and **sectioned templates** compiled from heading + instruction (`#L29-L78`), with **seeded defaults upgraded only while untouched** (`src/helpers/builtinActions.js#L1-L18`) → theuth meeting templates / note actions.
12. **Token-guarded loopback bridge** (per-launch 32-byte token in a 0600 file, loopback check) + a SKILL.md that teaches external agents the CLI (`src/helpers/cliBridge.js#L111-L195`, `agent-skills/openwhispr-cli/SKILL.md#L18-L40`) → a theuth skill file for Claude Code/Codex that drives theuth's meeting tools.
13. **Markdown mirror** with frontmatter + separate transcript file (`src/helpers/markdownMirror.js#L131-L200`) → optional theuth meeting export folder (Obsidian).
14. **Calendar incremental sync** (Google syncToken with 24 h TTL + 410 → full; Graph delta with 7-day TTL, windowed) (`src/helpers/googleCalendarManager.js#L9-L17`, `#L174-L215`; `src/helpers/microsoftCalendarManager.js#L9-L23`, `#L201-L230`) → a theuth calendar source (EventKit first via the companion, `resources/macos-calendar-listener.swift#L24-L25`, `#L115-L210`).
15. **Weekly provider canaries in CI** (`scripts/stt-canary.mjs#L1-L16`, `.github/workflows/stt-canary.yml#L10`) → theuth provider layer.
16. **Sidecar supervision pattern** (port range, health every 5 s, restart after N failures, restart cap per session, `restarted(port)` re-wire) (`src/helpers/qdrantManager.js#L16-L45`) and **memory-bounded llama context sizing from GGUF metadata** (`src/helpers/llamaContextPolicy.js#L62-L128`) → any theuth sidecar / local model runner.

### Q3 How to make theuth better — highest leverage, in order
1. **Calendar-aware meetings** (EventKit via the Swift companion first): auto-title, attendees → participants, attendee-restricted voiceprint matching and 1:1 auto-enrollment, and — theuth-only — feed the event description + attendees into the pre-meeting brief cache so the island has briefs before the first word (borrow 7, 14).
2. **Auto-detect + auto-end** from process-attributed mic use, calendar imminence and silence windows, with "process presence is context only" (borrow 1, 2).
3. **AEC3 on the mic track with the system track as reference + the bleed classifier**, so speakers-not-headphones meetings stop producing duplicate "me" lines and wrong topics, and Windows/Linux lose the `MicNoAec` degradation (borrow 3, 4).
4. **Speaker matching hygiene**: threshold + margin, EMA profiles with sample counts, duration-ramped clustering in the post pass (borrow 5, 6, 8).
5. **Retrieval and prompts**: RRF hybrid for `meeting_search`/knowledge, and the source-fidelity + speaker-label-only + summary-only-input rules in the meeting note prompt (borrow 9, 10, 11).


## Cites self-check

Re-opened 87 cites from this report with `read_file(offset, limit)` and checked that each range contains the identifier or phrase the claim rests on; 0 misses.
- Corrections made while writing (before this pass): the Metal claim in A2 softened to "not confirmed" after `scripts/download-whisper-cpp.js#L25-L33` showed only per-arch zips; the browser-meeting sentence in A1 marked as inference; sidecar reaper in A9 marked "by file names";
- Q1's cross-platform-capture line corrected against theuth knowledge (theuth has Windows/Linux loopback; it lacks AEC there).

- `src/helpers/meetingProcessDetector.js#L7-L20` — held (re-opened; contains `us.zoom.xos`)
- `src/helpers/meetingProcessDetector.js#L88-L112` — held (re-opened; contains `NSWorkspaceDidLaunchApplicationNotification`)
- `src/helpers/meetingDetectionEngine.js#L88-L98` — held (re-opened; contains `context-only`)
- `src/helpers/meetingDetectionEngine.js#L15-L18` — held (re-opened; contains `IMMINENT_THRESHOLD_MS`)
- `src/helpers/meetingAutoEndController.js#L2-L19` — held (re-opened; contains `SILENCE_WINDOW_MS`)
- `resources/macos-mic-listener.swift#L144-L160` — held (re-opened; contains `kAudioProcessPropertyIsRunningInput`)
- `resources/macos-mic-listener.swift#L224-L255` — held (re-opened; contains `com.apple.CoreSpeech`)
- `resources/macos-audio-tap.swift#L48-L62` — held (re-opened; contains `AudioHardwareCreateProcessTap`)
- `resources/macos-audio-tap.swift#L114-L131` — held (re-opened; contains `kAudioAggregateDeviceIsPrivateKey`)
- `src/helpers/audioTapManager.js#L12-L16` — held (re-opened; contains `DEFAULT_SAMPLE_RATE`)
- `src/helpers/audioTapManager.js#L427-L450` — held (re-opened; contains `architecture mismatch`)
- `resources/windows-system-audio-helper.c#L249-L280` — held (re-opened; contains `PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE`)
- `native/meeting-aec-helper/src/aec_processor.cc#L31-L46` — held (re-opened; contains `EchoCanceller3Factory`)
- `src/helpers/meetingAecManager.js#L6-L10` — held (re-opened; contains `MIC_STREAM`)
- `src/helpers/meetingEchoLeakDetector.js#L7-L36` — held (re-opened; contains `STRONG_BLEED_CORRELATION`)
- `src/helpers/ipcHandlers.js#L7913-L7914` — held (re-opened; contains `shouldSuppressMicSegment`)
- `resources/macos-disclaim-exec.c#L1-L35` — held (re-opened; contains `responsibility_spawnattrs_setdisclaim`)
- `src/helpers/recordingSpool.js#L9-L10` — held (re-opened; contains `openwhispr-recording-spool`)
- `src/helpers/mediaPlayer.js#L196-L246` — held (re-opened; contains `MediaRemote`)
- `src/helpers/liveSpeakerIdentifier.js#L33-L50` — held (re-opened; contains `MATCH_MARGIN`)
- `src/helpers/liveSpeakerIdentifier.js#L695-L747` — held (re-opened; contains `_findStoredProfileMatch`)
- `src/helpers/diarization.js#L54-L64` — held (re-opened; contains `pyannote-segmentation-3-0`)
- `src/helpers/diarizationPolicy.js#L11-L44` — held (re-opened; contains `LONG_AUDIO_CLUSTER_THRESHOLD`)
- `src/helpers/database.js#L984-L1015` — held (re-opened; contains `note_speaker_embeddings`)
- `src/helpers/database.js#L5940-L5960` — held (re-opened; contains `0.3 * incoming`)
- `src/helpers/ipcHandlers.js#L7900-L7912` — held (re-opened; contains `attendeeEmails`)
- `src/helpers/ipcHandlers.js#L7916-L7945` — held (re-opened; contains `upsertSpeakerProfile`)
- `src/helpers/speakerEmbeddings.js#L7-L22` — held (re-opened; contains `EMBEDDING_DIM`)
- `src/workers/onnxWorker.js#L20-L31` — held (re-opened; contains `TEXT_EMBED_DIM`)
- `src/helpers/meetingDiarizationInput.js#L1-L32` — held (re-opened; contains `diarizedSource`)
- `src/helpers/database.js#L409-L450` — held (re-opened; contains `notes_fts`)
- `src/helpers/noteSearch.js#L1-L14` — held (re-opened; contains `buildNoteSearchQuery`)
- `src/helpers/ipcHandlers.js#L2281-L2341` — held (re-opened; contains `Reciprocal Rank Fusion`)
- `src/helpers/qdrantManager.js#L16-L45` — held (re-opened; contains `MAX_RESTARTS_PER_SESSION`)
- `src/helpers/vectorIndex.js#L9-L33` — held (re-opened; contains `Cosine`)
- `src/helpers/semanticSearchLifecycle.js#L1-L6` — held (re-opened; contains `DRAIN_PAGE_SIZE`)
- `src/helpers/markdownMirror.js#L131-L141` — held (re-opened; contains `folder:`)
- `src/helpers/markdownMirror.js#L186-L200` — held (re-opened; contains `-transcript.md`)
- `src/helpers/cliBridge.js#L10-L20` — held (re-opened; contains `cli-bridge.json`)
- `src/helpers/cliBridge.js#L384-L575` — held (re-opened; contains `/v1/notes/search`)
- `src/helpers/googleCalendarOAuth.js#L1-L12` — held (re-opened; contains `calendar.events.readonly`)
- `src/helpers/googleCalendarManager.js#L9-L17` — held (re-opened; contains `SYNC_TOKEN_TTL_MS`)
- `src/helpers/googleCalendarManager.js#L280-L288` — held (re-opened; contains `notContacts`)
- `src/helpers/microsoftCalendarManager.js#L9-L23` — held (re-opened; contains `DELTA_TOKEN_TTL_MS`)
- `src/helpers/microsoftCalendarOAuth.js#L10-L11` — held (re-opened; contains `Calendars.Read`)
- `src/helpers/meetingDetectionEngine.js#L605-L651` — held (re-opened; contains `participants`)
- `src/helpers/calendarReminderScheduler.js#L4-L4` — held (re-opened; contains `MEETING_REMINDER_LEAD_MS`)
- `src/services/tools/index.ts#L1-L64` — held (re-opened; contains `registerConnectorTools`)
- `src/services/tools/searchNotesTool.ts#L69-L85` — held (re-opened; contains `RRF`)
- `src/helpers/whisperServer.js#L23-L35` — held (re-opened; contains `AUTO_THREAD_RATIO`)
- `src/helpers/whisperServer.js#L195-L208` — held (re-opened; contains `--vad-threshold`)
- `src/helpers/parakeetServer.js#L24-L33` — held (re-opened; contains `ONLINE_MAX_SEGMENT_SECONDS`)
- `src/helpers/meetingTranscriptionRouting.js#L9-L13` — held (re-opened; contains `tinfoil`)
- `src/helpers/cloudChunkPolicy.js#L16-L42` — held (re-opened; contains `CLOUD_CHUNK_MAX_LOSS_RATIO`)
- `src/helpers/llamaServer.js#L15-L35` — held (re-opened; contains `DEFAULT_CONTEXT_SIZE`)
- `src/helpers/llamaContextPolicy.js#L62-L128` — held (re-opened; contains `USABLE_MEMORY_FRACTION`)
- `src/helpers/templatePrompts.js#L97-L126` — held (re-opened; contains `never split`)
- `src/helpers/templatePrompts.js#L29-L78` — held (re-opened; contains `TEMPLATE INSTRUCTIONS`)
- `src/helpers/builtinActions.js#L366-L386` — held (re-opened; contains `NEVER guess or invent`)
- `src/helpers/builtinActions.js#L1-L18` — held (re-opened; contains `never touched`)
- `src/helpers/sourceFidelity.js#L1-L3` — held (re-opened; contains `SOURCE_FIDELITY_RULE`)
- `src/helpers/windowManager.js#L2253-L2253` — held (re-opened; contains `setContentProtection`)
- `src/helpers/enterpriseManagedConfig.mjs#L1-L17` — held (re-opened; contains `managed_required`)
- `src/helpers/tinfoilSecureClient.js#L1-L14` — held (re-opened; contains `attested`)
- `src/services/AnalyticsService.ts#L16-L35` — held (re-opened; contains `MAX_BATCHES_PER_PASS`)
- `src/components/onboarding/flow.ts#L115-L149` — held (re-opened; contains `setup-choice`)
- `scripts/stt-canary.mjs#L1-L16` — held (re-opened; contains `canary`)
- `package.json#L78-L78` — held (re-opened; contains `node --import tsx --test`)
- `package.json#L154-L161` — held (re-opened; contains `@tiptap`)
- `openwhispr-mobile/package.json#L88-L88` — held (re-opened; contains `appsflyer`)
- `src/stores/meetingRecordingStore.ts#L131-L132` — held (re-opened; contains `SYSTEM_SPEAKER_CARRY_FORWARD_MS`)
- `src/helpers/tray.js#L282-L290` — held (re-opened; contains `startMeetingRecording`)
- `src/hooks/useEmbeddedChat.ts#L90-L111` — held (re-opened; contains `transcript is source material`)
- `README.md#L61-L62` — held (re-opened; contains `MCP`)
- `src/helpers/noteChunking.js#L108-L135` — held (re-opened; contains `splitChunkInHalf`)
- `src/config/prompts/registry.ts#L5-L8` — held (re-opened; contains `voice assistant`)
- `src/services/ReasoningService.ts#L121-L121` — held (re-opened; contains `MAX_TOOL_STEPS = 20`)
- `src/services/ReasoningService.ts#L1226-L1242` — held (re-opened; contains `compactTranscriptHistory`)
- `src/services/ReasoningService.ts#L1243-L1246` — held (re-opened; contains `keeps`)
- `src/helpers/ipcHandlers.js#L5980-L5987` — held (re-opened; contains `captureActiveDisplay`)
- `src/helpers/screenContextCapture.js#L1-L14` — held (re-opened; contains `MAX_EDGE_PX`)
- `src/utils/agentSelectionContext.ts#L1-L1` — held (re-opened; contains `6000`)
- `src/helpers/conversationChunker.js#L1-L22` — held (re-opened; contains `CHUNK_OVERLAP`)
- `scripts/download-whisper-cpp.js#L25-L33` — held (re-opened; contains `whisper-server-darwin-arm64`)
- `src/helpers/meetingJoinUrl.js#L1-L15` — held (re-opened; contains `meet.google.com`)
- `src/helpers/windowConfig.js#L180-L194` — held (re-opened; contains `alwaysOnTop`)
- `src/helpers/audioStorage.js#L34-L37` — held (re-opened; contains `.webm`)

Unverifiable here: GitHub numbers (taken through the authenticated GitHub tools on 2026-10-10 after the unauthenticated REST API returned 403 rate-limit); README-only statements are labelled as such in the text.

