# AXES — open-source local meeting note-takers (Meetily · Anarlog · OpenWhispr · Minutes) vs theuth meetings

Shared numbering for every recon report. Every report uses these headings in this order, `## A1` … `## A9`,
then `## Map to the three questions`, then `## Cites self-check`. A claim about code carries a cite
`path#L<a>-L<b>` (repo-relative). "Not found" is a valid finding — write it with what you searched; never guess.

Context (the ask, 2026-10-10 12:08, Sky, verbatim): "Let's take the top 4 and do a study of what they offer versus
~/Develop/theuth/ Then let's think about what we can learn from them, what can we borrow, how can we use this
information to make theuth better?"

The top 4 (from a same-day web survey; all MIT-licensed; Sky wants NO code copied regardless of licence — techniques only):
- Meetily — Zackriya-Solutions/meetily, 31.6k stars, Rust/Tauri + Next.js (+ Python backend), community edition + paid PRO.
- Anarlog — fastrepl/anarlog, 9.5k stars, Rust/Tauri monorepo; same codebase as Hyprnote (YC S25), renamed char → anarlog;
  char.com is now a separate hosted product; managed "Pro (Cloud)" transcription optional.
- OpenWhispr — OpenWhispr/openwhispr, 9.2k stars, Electron; dictation + meeting transcription + notes, local diarization,
  calendar sync.
- Minutes — silverstein/minutes, 1.5k stars, Rust + Tauri; records meetings/memos locally, markdown + YAML to disk,
  ~31 MCP tools so Claude Code / Codex / any MCP client can query the history.

theuth today, from its design docs (the OURS analyst verifies every line): a meeting feature in `crates/meeting` + the desktop
meeting page + an always-on-top "island". Mic + system audio capture (macOS), three Ogg Opus tracks per meeting; live STT;
live TOPICS picked by a cheap model and BRIEFS (facts about the thing, never what to say) written with tools (knowledge first,
then web) and shown on the island; push-to-talk steering; the island and main window hidden from screen capture by default;
after the meeting: diarization (pyannote-rs segmentation + wespeaker CAM++), a user correction pass (relabel/merge/split,
enroll a speaker from a segment), summary + a meeting note (block document), a meeting chat session, `meeting_read` /
`meeting_search` / `meeting_speakers` agent tools, transcript harvested into knowledge, a brief cache across meetings.
theuth is a general agent harness (orchestrator, subagents, knowledge base, notebooks, schedules, MCP client, companion apps),
so a borrow can land in the meeting feature OR in the wider harness.

## A1 Capture and meeting detection
How a meeting starts (manual, auto-detect from a meeting app using the mic, calendar event, window title), what is captured
(mic, system audio, per-app audio), per-OS mechanisms (macOS ScreenCaptureKit / Core Audio process taps, Windows WASAPI
loopback, Linux PipeWire/Pulse), echo cancellation between mic and system tracks, mixing/ducking/normalisation, permissions
and onboarding (TCC), recording format and storage, crash-safety/recovery of an interrupted recording, retention, device
changes mid-meeting (AirPods, docks).

## A2 Transcription
Engines and models (Whisper/whisper.cpp, Parakeet, Moonshine, Apple SpeechAnalyzer, sherpa-onnx, cloud), live vs batch
(live partials then a better final pass?), chunking/VAD, hallucination guards, language detection/multilingual, model
catalogue/download/verify, hardware acceleration, measured latency/RTF/WER numbers, custom vocabulary/hotwords, how "local"
is enforced vs cloud fallbacks.

## A3 Speakers
Me-vs-them from channels, diarization (live or post; which models), voiceprints/enrollment and recognition across meetings,
naming speakers from calendar attendees, correction UI (relabel/merge/split), how speaker labels flow into notes.

## A4 Notes, summaries and the LLM layer
User-typed notes during the meeting and how they are merged with the transcript ("enhance"), templates per meeting type
(who writes them, format, variables, sharing), the actual prompts, local LLM path (Ollama/LM Studio/llama.cpp/bundled model)
vs cloud, long-transcript handling (chunking/map-reduce), action items/decisions extraction, structured output (frontmatter),
regeneration and editing, the editor used.

## A5 Live assistance during the meeting
What the user sees live (transcript, floating widget/overlay/notch), anything the AI does DURING the meeting (live
suggestions, ask-during-meeting, running summary, alerts), steering, latency — and how that compares with theuth's island +
live topics + briefs.

## A6 Memory, search, agent access and integrations
Storage format (markdown files, sqlite), full-text and semantic search, chat/ask across meetings, MCP servers/tools and
agent integrations (Claude Code, Codex, OpenCode…), exports (Obsidian, Notion, Slack, email), calendar integration
(Google/Microsoft/Apple: pre-meeting context, attendees, auto-titles), people/company pages, follow-ups and sharing.

## A7 UX surfaces and onboarding
Main window and library layout, the meeting page live and after, editor, tray/menu bar, hotkeys, floating widget, first-run
onboarding and permissions flow, states shown (idle/recording/processing/done/failed), notifications, mobile companion.

## A8 Privacy, local-only guarantees and business model
What is local by default vs optional cloud, what leaves the machine and when, telemetry/analytics and opt-out, accounts and
sign-in, paid tiers and exactly which features are held back from the open build, enterprise features, licence.

## A9 Engineering, platforms and traction
Architecture (Tauri/Electron/Rust crates/sidecars), platforms supported per feature, tests and eval tooling (transcript and
summary evals), diagnostics/logging, updater, release cadence, repo health (stars, issues, PRs).

## Map to the three questions
Q1 What they offer that theuth does not — and what theuth has that they do not.
Q2 What we can learn — techniques worth borrowing, each with a cite and where it would land in theuth.
Q3 How to make theuth better — the 3–5 highest-leverage changes, in order.

## Cites self-check
Re-open ≥ 15 of your own cites before you finish; list `path#L — held | corrected`.
