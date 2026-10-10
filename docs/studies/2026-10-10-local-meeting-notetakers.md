# Study: open-source local meeting note-takers vs theuth — Meetily, Anarlog (ex-Hyprnote), OpenWhispr, Minutes

- **Date:** 2026-10-10 (recon 12:12–12:31; seven read-only children on nine shared axes, then synthesis)
- **Question (Sky):** "Let's take the top 4 and do a study of what they offer versus ~/Develop/theuth/ Then let's think about what we can learn from them, what can we borrow, how can we use this information to make theuth better?" — the top four of the same-day survey of open-source, fully local meeting recorders (knowledge `facts/oss-local-meeting-notetakers-survey-2026-10-10`).
- **Status:** answered — twenty borrows ranked and grouped (§6), four waves proposed (§9), five decisions needed from Sky (§10). This file is the living record.
- **Licence:** all four are MIT. Everything here is **techniques only**; no code is copied. Clones (read-only, not in git): `~/Develop/spike-meeting-notetakers/` — meetily `a2cb62e` (2026-09-10), anarlog `ef69625` (2026-10-10), openwhispr `5f9a88b` (2026-10-10), minutes `d3285c7` (2026-10-08).
- **Evidence:** appendices under `docs/studies/2026-10-10-local-meeting-notetakers/` — A Meetily (316 `path#L` cites), B Anarlog engine (471), C Anarlog surface (283), D OpenWhispr (386), E Minutes (447), F theuth inventory on main `2a1238f2` (386; no meeting-stack change through `2de5c411`), G web survey (62 sources), AXES. The authors re-opened 345 of their own cites (2 corrected); the orchestrator re-opened 42 load-bearing cites in both trees — all held. Knowledge: `facts/meeting-notetakers-source-recon-2026-10-10`, `facts/theuth-meetings-inventory-vs-notetaker-axes-2026-10-10`, `facts/meeting-notetaker-user-evidence-and-granola-2026-10-10`, `facts/theuth-borrow-list-from-meeting-notetakers-2026-10-10`.

## 1. Verdict

**theuth is ahead during the meeting and behind before and around it.** None of the four runs tool-backed AI while the call is live: Meetily, Anarlog and OpenWhispr show a transcript or captions only, and Minutes' "Coach" is a fast model reading a pre-computed 7,000-character card (E, `crates/core/src/copilot/battle_card.rs#L5-L20`). theuth writes factual briefs with tools during the call (`crates/meeting/src/tools.rs#L26-L57`), reuses them across meetings from a spoken-alias cache with no model call (`crates/meeting/src/cache.rs#L1-L5`), gates every batch with a local decision model that learns from taps and dismissals (`crates/meeting/src/oracle.rs#L1-L20`), and shows them on an island that never takes focus and is hidden from screen share (`crates/desktop/src/island/mod.rs#L88-L92`). The web survey found nothing local that does this; the nearest product is Microsoft Teams Facilitator, cloud-only and licensed (G §6).

Where theuth trails, all four and the users agree: a meeting starts only by hand (`crates/meeting/src/oracle.rs#L382` is the only "calendar" in the code), there is no calendar, one fixed summary prompt (`crates/meeting/src/note.rs#L23-L34`), no MCP server for outside agents (`crates/theuth/src/mcp.rs#L1-L6` serves only theuth's own ACP children), no recovery of a recording cut off by a crash (`crates/meeting/src/lib.rs#L3781-L3790`), no device follow, no echo cancellation off macOS (`crates/meeting/src/capture/system.rs#L106-L116`), English-only live STT (`crates/voice/src/lib.rs#L64`), and no evals.

- **Q1 — what they offer that theuth does not:** auto-detection from the process that holds the mic; calendar titles, attendees and pre-meeting briefs; speaker naming from attendees; templates and stricter note prompts; meeting history for external agents over MCP; crash-safe capture and device follow; echo cancellation on every OS; markdown on disk; eval harnesses. **What theuth has that they do not:** the live briefing loop, the cross-meeting brief cache, the oracle, push-to-talk asks into the live session, the capture-hidden island, live speaker labels kept by the post pass, a note the summary reorganises without rewriting, and a meeting that is a full agent session with knowledge harvest and notebooks.
- **Q2 — what to learn:** the strongest techniques are OpenWhispr's and Anarlog's detection rules (mic-in-use per process, app presence only as context, cooldowns), Anarlog's pre-meeting brief ranking and attendee-restricted voiceprint matching, Minutes' agent-safety rules (restricted meetings, audit, routing skill) and decision/intent schema, Anarlog's proposal-gated agent writes, OpenWhispr's prompt rules and hybrid search, and Meetily's and Anarlog's crash-safe recording (§6).
- **Q3 — how to make theuth better:** first make every meeting robust, local-capable and templated (wave M1); then extend the live advantage to *before the first word* — detection, calendar and a pre-meeting brief written by theuth's own brief writer (M2, the headline bet nobody else can make); then make theuth the meeting memory every agent can read (M3); quality and echo work last (M4, §9).

## 2. Method

| Child | Scope | Output | Re-opened by author |
|---|---|---|---|
| recon-meetily | Meetily source, A1–A9 | A, 613 lines | 45, all held |
| recon-anarlog-engine | Anarlog capture, detection, AEC, STT, speakers, engineering | B, 407 lines | 88, all held |
| recon-anarlog-surface | Anarlog notes, templates, search, agents, UX, paid boundary | C, 592 lines | 30, 1 corrected |
| recon-openwhispr | OpenWhispr meetings, speakers, search, calendar, agent | D, 435 lines | 87, all held |
| recon-minutes | Minutes MCP, skills, schema, guards, speakers | E, 416 lines | 60, 1 corrected |
| recon-ours | theuth meeting stack on the same axes | F, 694 lines | 35, all held |
| recon-web (read-only) | issues by reactions, HN, Reddit snippets, Granola, the field | G, 415 lines | 62 sources |

All children were read-only (no builds). The orchestrator spot-checked the decisive facts while they ran (Anarlog's ONNX AEC crate and detect crate; theuth's VoiceProcessingIO AEC and note-first summary turn) and re-opened 42 cites afterwards. GitHub's unauthenticated search was rate-limited for several children; traction numbers come from the GitHub API through other routes (G §1).

## 3. The four in one screen

**Meetily** (31.6k stars, Rust/Tauri + Next.js, paid PRO; A). The most starred and the most overstated. Its "professional mixing with intelligent ducking" is a plain sum with a hard clip (`frontend/src-tauri/src/audio/pipeline.rs#L158-L203`); mic and system audio are mixed *before* transcription, so the open build has no diarization and not even me-vs-them (`pipeline.rs#L848-L903`); "4x faster" exists only in the repo description; a PostHog key is hard-coded (`analytics/commands.rs#L12-L14`). Worth taking: 30 s checkpoint files plus a recovery dialog for interrupted meetings (`audio/incremental_saver.rs#L18-L93`), VAD silence of 500 ms live vs 2000 ms batch with the measured reason (`pipeline.rs#L16-L27`), Whisper guards (`whisper_engine/whisper_engine.rs#L596-L601`), context-sized map-reduce for local models only (`summary/service.rs#L432-L476`), an instructions-in-transcript guard and "None noted in this section." (`summary/processor.rs#L229-L253`), backup-and-restore on regenerate, a device-loss policy (`audio/device_monitor.rs#L62-L70`). PRO holds diarization, templates, auto-detect, exports and MCP.

**Anarlog** (ex-Hyprnote, 9.5k stars, Rust/Tauri monorepo, 58 releases in 90 days, GitHub issues disabled; B, C). The deepest engine. Meeting detection fires when an app not on an ignore list holds the mic for 15 s (`plugins/detect/src/mic_usage_tracker.rs#L8`), with a 10-minute cooldown per app (`#L9`) and accessibility analysis of Zoom/Meet/Teams/Slack huddles (`crates/detect/src/meeting_ax.rs`), which can also read *and send* in-meeting chat. Echo: an FFT lag probe locks the system→mic lag up to 600 ms (`crates/audio-actual/src/capture/stream.rs#L30`) before a two-stage ONNX echo canceller (`crates/aec/src/onnx/model.rs#L1-L24`); headphones switch it off and auto-enroll the owner. Speakers are named from calendar attendees with a mutual unique match at cosine ≥ 0.62 and margin 0.08 (`crates/voiceprint/src/matching.rs#L3-L163`). Notes: a pre-meeting brief ranked by same series > same title + shared people > shared people (`apps/desktop/src/session/insights/pre-meeting.ts#L21-L102`), the user's note snapshotted and their bold/H3 treated as importance (`crates/template-app/assets/enhance.system.md.jinja#L12-L27`), the stream head validated against the template. Agents get an 11-tool read-only MCP server (`apps/cli/src/mcp.rs#L42-L281`) and may only *propose* edits that a person applies (`crates/agent-access/src/proposals.rs#L95-L133`). No AI runs during the meeting; the bundled llama.cpp is disabled (`crates/local-llm-core/src/server.rs#L9-L17`); custom dictionaries and summary formats are Pro.

**OpenWhispr** (9.2k stars, Electron, dictation + meetings; D). The best detection and speaker hygiene. Mic-in-use per process (`resources/macos-mic-listener.swift#L106-L160`) starts a meeting; a running meeting app is context only because "FaceTime idles in the background" (`src/helpers/meetingDetectionEngine.js#L88-L135`). A WebRTC AEC3 helper cleans the mic with system audio as reference (`native/meeting-aec-helper/src/aec_processor.cc#L31-L46`) and a bleed classifier drops lines that are only speaker echo. Voiceprints need ≥ 0.65 and a 0.03 margin over the runner-up (`src/helpers/liveSpeakerIdentifier.js#L33-L50`); only the event's attendees are candidates and a 1:1 enrolls the other attendee automatically (`src/helpers/ipcHandlers.js#L7900-L7945`). Search fuses SQLite keyword and a local MiniLM vector index by reciprocal rank (`src/helpers/vectorIndex.js#L9-L33`). Note prompts: source fidelity, speakers only by label, `- [ ] Action — Owner` (`src/helpers/templatePrompts.js#L29-L126`). No MCP server in the desktop app — agents use a loopback HTTP bridge with a per-launch token (`src/helpers/cliBridge.js#L10-L20`). No AI during the meeting.

**Minutes** (1.5k stars, Rust + Tauri, MCP-first; E). The agent-memory reference. It writes no summary by default (`crates/core/src/config.rs#L1710-L1714`): the user's own agent summarises over MCP. Restricted meetings are hidden from agents unless a human grant, a per-call flag and an audit line all line up (`crates/mcp/src/index.ts#L466-L520`); a routing skill maps question shapes to the smallest read-only tool (`tooling/skills/sources/minutes-mcp-recall/skill.md#L35-L75`); the frontmatter carries `decisions[].supersedes` and typed intents (`crates/core/src/markdown.rs#L1442-L1474`); knowledge facts carry a confidence level and source meeting (`crates/core/src/knowledge.rs#L148-L202`). Speaker work overlaps theuth's (pyannote-rs + wespeaker), with enrollment gates of ≥ 5 s speech, agreement ≥ 0.70, SNR ≥ 8 dB, clipping ≤ 5 % (`crates/core/src/voice.rs#L13-L32`). Weak spots: meeting search is FTS5 rebuilt in memory on every call; thin user traction (top issues have one reaction).

## 4. theuth on the same axes (appendix F; main `2a1238f2`)

| Axis | The four, at best | theuth today | Gap |
|---|---|---|---|
| A1 Capture, detection | mic-in-use detection (Anarlog, OpenWhispr); AEC on every OS; crash-safe files; device follow | three Opus tracks; VoiceProcessingIO AEC on macOS (`capture/mic.rs#L38-L42`); Windows/Linux capture without AEC; manual start | detection, recovery, device follow, AEC off macOS |
| A2 Transcription | Parakeet/Whisper local, guards, multilingual | Nemotron 560 ms live + Parakeet TDT post over speech only (`crates/voice/src/stt.rs#L1-L7`) | English-only live; no vocabulary in meetings |
| A3 Speakers | attendee-restricted voiceprints, 1:1 auto-enroll, quality gates | Sortformer live labels at 0.035× real time kept by the post pass (`post/diarize.rs#L112-L120`); enroll from a segment; matches only as post suggestions | attendee naming, auto-enroll, confidence + source |
| A4 Notes | templates, fidelity rules, user notes as signal, length scaling | note-first summary that reorganises without rewriting (`note.rs#L16-L35`); one prompt | templates and prompt rules |
| A5 Live help | captions; Minutes' card-based Coach | tool-backed briefs, cache, oracle, push-to-talk, island | **theuth leads** |
| A6 Memory, agents | MCP for outside agents (Minutes, Anarlog), proposals, hybrid search, calendar, markdown | meeting_* tools in every session, knowledge harvest, notebooks (`summary.rs#L159-L181`); FTS only (`search.rs#L1-L3`) | external MCP, calendar, hybrid search, vault |
| A7 UX | start prompt, pre-meeting view, calendar join | meeting page (156), island, meeting chat | start prompt, pre-meeting state |
| A8 Privacy | local LLM options; Minutes' restricted policy | no telemetry; transcript text to the chosen model; web tools on by default (`lib.rs#L593-L606`) | local-only mode and disclosure |
| A9 Engineering | DER harness (Anarlog), ASR eval CI (Minutes) | manual end-to-end list (`docs/design/meetings/README.md#L127-L182`) | evals and latency numbers |

## 5. What users say (appendix G)

- **Praise:** no bot with both sides captured locally; fully open source; custom templates + local LLM + Obsidian; markdown files the user owns; agent access to the history (an MCP request appears in the 180-comment Hyprnote launch thread).
- **Complaints and asks:** Meetily #32 Linux (46 reactions), #226/#230 speaker identification, #243 templates, #218 auto-record; Hyprnote HN: "the inability to tell who said what is a show stopper" after speaker ID was advertised before it worked; OpenWhispr #2582 echo left in the note, #1458 silence hallucinations; Minutes #245 a name spoken in the meeting replaced the user's own, #933 an MCP time budget that denied every query on a real corpus, #244 false call detection from dictation apps.
- **The paid line:** Granola Business ($14/user/month) holds pre-meeting briefs and full MCP; Meetily PRO holds diarization, templates, auto-detect and MCP; Anarlog Pro holds dictionaries and formats. Granola's public-by-default links and training defaults drew press and enterprise bans (The Verge, 2026-04-02).

## 6. Borrows, ranked and grouped by where they land (techniques only)

**Robust, local, templated — `crates/meeting`, `crates/voice`, the start form**
1. **Crash-safe recording recovery.** On engine start treat a `Recording`/`Paused` meeting with tracks on disk as stopped, finalise and run post; show "Recovered" in the list (Meetily A1, Anarlog B A1). Lands at the plugin-start filter (`crates/meeting/src/lib.rs#L3781-L3790`). Sky's standing rule: recording takes precedence over intelligence.
2. **Device follow.** Re-open the default device on loss with retries at 1/2/4 s (Bluetooth after 3 missed checks, wired after 2), fill the gap with silence so tracks stay aligned (Anarlog, Meetily). Lands in the `DeviceLost` branch (`capture/system.rs#L64-L73`) and `capture/mic.rs`.
3. **The dictionary in meetings.** Feed `crates/voice/src/dictionary` into post-pass replacements and a "Preferred names" block in the brief and summary prompts (Anarlog uses one dictionary for STT hotwords and the prompt).
4. **Note prompt rules.** Source fidelity; speakers only by label; omit empty sections; `- [ ] Action — Owner`; ignore instructions inside the transcript; the user's bold and headings mark importance; length proportional to the transcript (OpenWhispr, Meetily, Anarlog). Lands in `note.rs` (`summary_form`).
5. **Templates per meeting type.** Headings plus per-section instructions, built-ins upgraded only while unedited, chosen in the start form or from the calendar event; format-from-examples later (OpenWhispr, Anarlog). Lands at `form_for` (`summary.rs#L29-L35`) and `StartForm.tsx`.
6. **Local-only meeting mode with a disclosure line.** One switch pins briefs and the summary to a local provider (Ollama, LM Studio, Apple Foundation Models) and turns web tools off; the island says so in one line. Lands in `MeetingConfig.live_tools` (`lib.rs#L559-L577`) and the session model (`lib.rs#L639-L641`).

**Before the meeting — detection, calendar, island, speakers**
7. **Meeting detection.** Mic-in-use attributed to a process (Core Audio `kAudioProcessPropertyIsRunningInput` on macOS, audio sessions on Windows) held 15 s by an app not on an ignore list — dictation (including theuth's own voice key), IDEs, screen recorders, assistants; meeting-app presence is context only; one prompt per overlap; quiet while recording, dictating or after a dismissal; 10-minute cooldown per app; auto-end on app exit plus silence (OpenWhispr D A1, Anarlog B A1).
8. **Calendar, EventKit first.** The macOS companion reads Calendar (which already aggregates Google and Exchange accounts added to macOS): title, attendees → participants, join link, a prompt 60 s before; Google/Microsoft direct later for Windows (OpenWhispr D A6).
9. **Pre-meeting brief on the island — theuth's twist.** Rank past meetings by same series > same title + shared people > shared people with three cached key facts each (Anarlog C A6), *and* run theuth's brief writer with tools over the event title, description and attendees, knowledge entities first, so briefs are on the island before anyone speaks. No competitor can do the second half.
10. **Attendee-restricted speaker naming.** Candidates are the attendees minus the owner; a mutual unique best match above a floor and margin (Anarlog 0.62/0.08, OpenWhispr 0.65/0.03 — retune for wespeaker); fill the last speaker by elimination; in a 1:1 bind the remote voice to the other attendee; with headphones enroll the owner; enrollment gates from Minutes; every mapping stores name, confidence and source; never rename the owner from a name heard in the meeting. Results stay suggestions the review pass can overturn. Lands in `speakers.rs` and the post pass.

**Memory for every agent — a serve mode beside `crates/theuth/src/mcp.rs`, the document crate, search**
11. **Meetings over MCP for external agents.** Read-only tools over the existing `meeting_*` RPCs (list, search, read by cursor, speakers, briefs, decisions); restricted meetings hidden unless a human grant, a per-call flag and an audit line agree; a routing skill (Minutes E A6, Anarlog C A6).
12. **Proposal-gated writes.** An outside agent proposes a note edit with the base version and a diff; a person applies it (Anarlog `proposals.rs`). Fits theuth's approvals.
13. **Decisions and intents in the note.** `decisions[].supersedes` and intents {kind, who, by_date, status}; harvest into knowledge with confidence levels and the source meeting, so "did that decision stick?" has an answer across meetings (Minutes).
14. **Hybrid meeting search.** Full-text plus local embeddings fused by reciprocal rank (K = 60), weak vector hits dropped, keyword fallback; reuse the notebooks' block index (OpenWhispr).
15. **Markdown + YAML export per meeting** into a vault folder, readable without theuth (Minutes).
16. **Import past meetings** from Granola, Fireflies, Otter and others through *their* MCP servers, as a routine (Anarlog C A6). theuth is already an MCP client; no engine work.

**Quality and echo — capture, post, autoresearch**
17. **Echo cancellation off macOS.** WebRTC AEC3 with the system track as reference, a lag probe up to 600 ms, and a bleed check (mic vs the last 6 s of system audio, or stem-loudness correlation ≥ 0.85 for a lone remote speaker) (OpenWhispr, Anarlog, Minutes).
18. **A post-STT guard pass** (repetition collapse at ≥ 3 segments and ≥ 0.8 similarity, known phrases, trailing noise) — mostly Whisper failure modes; measure on Parakeet first (Minutes, Meetily).
19. **A multilingual live tier** beside `nemotron-streaming-en`.
20. **Evals.** A DER dev set with a keep-if-better loop (Anarlog's `crates/pyannote-local/eval/PROGRAM.md#L1-L44` is an autoresearch loop — design 140 runs exactly this), streaming WER in CI (Minutes), brief and summary quality.

*Maybe later:* slide capture by screen signature as brief context; read-only in-meeting chat through accessibility as brief context (Anarlog). Never typing or sending.

## 7. Do not borrow

- Mixing mic and system audio before transcription (Meetily) — it throws away the channel; keep three tracks.
- Typing or sending into meeting chat through accessibility (Anarlog).
- A hosted calendar relay (Anarlog via Nango) — EventKit or local OAuth instead.
- Paywalling basics: templates, dictionaries, speaker naming, MCP.
- No summary by default, or shelling out to `claude -p` as the summariser (Minutes) — theuth has its own loop.
- Rebuilding full-text search in memory on every call (Minutes).
- Telemetry SDKs or first-party usage upload (Meetily PostHog, OpenWhispr insights) — theuth has none; keep it that way.
- English-first-then-translate summaries (Meetily) — not needed with multilingual models; revisit only for small local ones.
- Marketing ahead of code (Meetily's "ducking" and "4x", Hyprnote's early speaker ID) — ship the eval first.

## 8. Their weaknesses to design against

- Speaker ID promised before it worked: one 20-person meeting all under a single speaker (Hyprnote HN). Ship naming as suggestions with confidence, never silent renames.
- Echo duplicates in notes (OpenWhispr #2582), silence hallucinations (#1458), a model reloaded per VAD chunk 227 times in one meeting (Minutes #315), the owner renamed from a spoken name (#245), an MCP budget that starved real corpora (#933), false detection from dictation tools (#244) — each becomes a regression test.
- Granola's public-by-default links and training defaults — theuth stays private by default with sharing explicit.
- Single-pass summaries with no chunking (Anarlog) and small local models that hallucinate (OpenWhispr #323) — a local-only mode needs the map-reduce guard and an honest model note.

## 9. Sequencing proposed

| Wave | Borrows | Lands in | Size | Why this order |
|---|---|---|---|---|
| M1 Robust, local, templated | 1–6 | `crates/meeting`, `crates/voice`, start form | M | cheap, felt in every meeting, and the base the later waves need |
| M2 Before the meeting | 7–10 | macOS companion (EventKit, detection), meeting, island, speakers | L | the headline bet: theuth's live briefs start before the first word |
| M3 Memory for every agent | 11–16 | new MCP serve mode, document crate, search | M | turns theuth into the meeting memory for Claude Code, Codex and others |
| M4 Quality and echo | 17–20 | capture, post, autoresearch | M–L | measured improvements once evals exist |

## 10. Open questions for Sky

1. **Detection default:** prompt to start (recommended) · record automatically · off.
2. **Calendar source:** macOS Calendar through the companion first (recommended — local, no OAuth, covers Google and Exchange accounts already added to macOS) · Google and Microsoft OAuth first (works on Windows) · both now.
3. **Meetings over MCP for outside agents:** read-only with restricted meetings hidden (recommended) · read plus proposed edits · not now.
4. **Local-only mode:** a per-meeting switch with a disclosure line (recommended) · one global setting · not needed.
5. **Order:** M1 first (recommended) · M2 first · M3 first.

## 11. Appendices

- A `2026-10-10-local-meeting-notetakers/A-meetily-source-recon.md`
- B `2026-10-10-local-meeting-notetakers/B-anarlog-engine-recon.md`
- C `2026-10-10-local-meeting-notetakers/C-anarlog-surface-recon.md`
- D `2026-10-10-local-meeting-notetakers/D-openwhispr-source-recon.md`
- E `2026-10-10-local-meeting-notetakers/E-minutes-source-recon.md`
- F `2026-10-10-local-meeting-notetakers/F-theuth-meetings-inventory.md`
- G `2026-10-10-local-meeting-notetakers/G-web-survey.md`
- `2026-10-10-local-meeting-notetakers/AXES.md` — the shared axes and briefs' context
