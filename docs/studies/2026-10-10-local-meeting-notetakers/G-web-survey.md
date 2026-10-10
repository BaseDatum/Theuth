# REPORT

Scope: user evidence and field facts only, collected 2026-10-10 between 12:16 and 12:40 (+01:00). Sources: GitHub REST and search APIs (via read_url and the GitHub MCP), HN Algolia, vendor pages and a few reviews. Reddit could not be read directly (every reddit.com page returned an access challenge), so Reddit evidence is limited to search-result snippets. "Not found" means it was not found inside the time box.

## 1. Traction and cadence (per app: stars, releases in the last 90 days with dates, contributors, open issues)

The 90-day window is 2026-07-12 → 2026-10-10. Open issues and open PRs come from the search API, and the two add up to the repo's `open_issues_count`.

**Meetily** (Zackriya-Solutions/meetily) [1][2]
- 31,576 stars · 3,454 forks · MIT · Rust · created 2024-12-26 · last push 2026-09-15.
- Releases in the last 90 days: **1**, v0.4.1 (2026-09-12). The ones before it were v0.4.0 (2026-06-05) and v0.3.0 (2026-03-03), so a major release about every quarter.
- Top contributors (commits): sujithatzackriya 263, safvanatzack 208, athulchandroth 62, mohammedsafvan 42, p-s-vishnu 9. That is a vendor team.
- Issues: 204 open, 157 open PRs, 346 issues ever.
- HN: Show HN 2025-02-22 got 4 points and 4 comments (https://news.ycombinator.com/item?id=43137186). A second Show HN on 2025-11-04 got 1 point. Little HN traction despite having the most stars.

**Anarlog** (fastrepl/anarlog, ex-Hyprnote) [3][4]
- 9,471 stars · 791 forks · MIT · Rust · created 2024-12-09 · last push 2026-10-10.
- Releases in the last 90 days: **58**.
  - 49 stable: desktop_v1.1.13 (2026-07-12) → v1.2.0 (07-15) → v1.3.0 (07-18) → v1.4.9 (08-13) → v1.4.24 (09-16) → v1.4.29 (2026-10-08).
  - 9 nightlies: v1.4.24-nightly.1–.9, 2026-09-11 → 09-19.
  - That is about one stable release every 2 days.
- Top contributors: yujonglee 4,721, ComputelessComputer 3,574, devin-ai-integration[bot] 1,105, duckduckhero 181, harshikaalagh-netizen 54. A bot is the #3 contributor.
- Issues: **the GitHub issue tracker and Discussions are both turned off** (`has_issues=false`, `has_discussions=false`). Search still counts 966 old issues and 8 open ones, but returns none of them. 3 open PRs. Where users report bugs now: not found (the HN thread pointed to Discord).
- HN: Launch HN "Hyprnote (YC S25)", 2025-07-29, 270 points, 180 comments (https://news.ycombinator.com/item?id=44725306). OWhisper ("Ollama for realtime speech-to-text"), 289 points, 75 comments (https://news.ycombinator.com/item?id=44901853).

**OpenWhispr** (OpenWhispr/openwhispr) [5][6]
- 9,219 stars · 1,142 forks · MIT · JavaScript/Electron · created 2025-06-19 · last push 2026-10-10.
- Releases in the last 90 days: **16**.
  - 10 app releases: v1.7.6 (07-18), v1.8.1 (07-30), v1.8.0 and v1.8.3 (08-13), v1.9.0 (08-24), v1.9.1 (08-27), v1.9.2 (08-29), v1.10.0 (09-11), v1.10.1 (09-14), v1.10.2 (09-15).
  - 6 helper binaries: windows-mic-listener v1.0.0 (07-14) and v1.1.0 (08-18), windows-fast-paste v2.0.0 (08-06), linux-text-monitor v1.0.0 (08-12), windows-system-audio-helper v1.1.0 (08-29) and v1.2.0 (09-30).
- Top contributors: gabrielste1n 1,578, Chadpiha 249, xAlcahest 168, hsusul 78, boseq 54.
- Issues: 222 open, 216 open PRs, 970 issues ever. It has the biggest PR backlog of the four.
- HN: no thread for this repo found. The "OpenWhispr" HN post (5 points) links to a different repo, MrPrinceRawat/OpenWhispr.

**Minutes** (silverstein/minutes) [7][8]
- 1,542 stars · 167 forks · MIT · Rust · created 2026-03-18 (7 months old) · last push 2026-10-08.
- Releases in the last 90 days: **24**.
  - v0.21.0 (07-13), v0.22.0–0.22.1 (07-16/17), v0.23.0 (07-23), v0.24.0 (07-27).
  - v0.25.0–0.25.6 (08-14 → 08-29), v0.26.0–0.26.3 (09-05 → 09-15), v0.27.0–0.27.2 (09-25 → 10-01), v0.28.0–0.28.2 (10-07 → 10-08).
  - Plus two archive tags on 08-12.
- Top contributors: silverstein 1,934, x1-wealth 194, dependabot 76, erikcw 14, gregoire22enpc 11. The site says 34 contributors and 2,556 npm downloads a month (https://useminutes.app/).
- Issues: 59 open, 17 open PRs, 256 issues ever. Reactions are almost all 0–1, so issues show what is being built, not what users want.
- HN: no thread found (searched "useminutes" and "minutes MCP meetings").

## 2. What users praise (top 8 across the four, with evidence links)

1. **No bot, both sides of the call captured locally.**
   - Hyprnote launch thread: "hosted/online alternatives (Granola, Notion recorder, ChatGPT recorder etc) don't really make sense if you can record locally" (https://news.ycombinator.com/item?id=44725306).
   - A maintainer in the same thread confirms it "listens to both sounds coming out (system audio output) and going in (microphone input)".
2. **Works on real meetings on day one.** "Installed, and already used to transcribe two meetings. At the moment I think that I'm going to carry on using this. Keep going, it looks great!" (same thread).
3. **Fully open source with no API calls.** "This one is completely open source and local, no api calls required at all" (same thread).
4. **Custom templates plus a local LLM plus Obsidian.** An r/LocalLLaMA post says Hyprnote "Transcribes audio from me and other participants into text, then creates a summary using LLM based on a template I can customize. I can use local LLMs like Ollama" (search snippet, https://www.reddit.com/r/LocalLLaMA/comments/1n0zcx2/free_local_ai_meeting_notetaker_hyprnote_obsidian/).
5. **Transcripts as plain markdown files the user owns.**
   - Muesli HN: "An OSS on device version where I can store transcripts as markdowns in my file system" (https://news.ycombinator.com/item?id=48011116).
   - Minutes builds its pitch on this: "Plain Markdown. Readable without Minutes. Ten years from now, grep still works." (vendor page, https://useminutes.app/).
6. **Agents can reach the meeting history.**
   - Hyprnote thread request: "registering an MCP server and calling an MCP tool upon transcript completion (and/or summary completion) would help" (item 44725306).
   - Minutes ships "CLI + 34 MCP tools" for "Claude, Codex, Gemini, and any MCP client" (https://useminutes.app/). The same-day survey counted ~31; the site now says 34.
7. **Meetily's tech choices and platform reach.**
   - "I love the choice for tauri! And all the other choices made are IMHO top notch, too!" (https://news.ycombinator.com/item?id=43137186).
   - A blog review praises its native Windows builds (https://andrew.ooo/posts/meetily-privacy-first-ai-meeting-assistant-review/). Treat that review with caution: it calls Granola macOS-only, but Granola's own homepage lists Windows.
8. **On-device ASR is good enough for English.** The Muesli maintainer: Parakeet and Whisper "are great for English, faster than cloud hosted models a little less accurate" (item 48011116).

No user praise was found for OpenWhispr on HN, and Reddit was unreachable. Its evidence is stars and the issue volume. One review site calls Hyprnote "Genuinely private… No meeting bot" (https://kompozy.io/reviews/hyprnote). That site sells its own product, it says Hyprnote is macOS-only (contradicted by the same-day survey), and it gives the Launch HN date as 2025-08-11 (HN says 2025-07-29). Low weight.

## 3. What users complain about / request (top 15 across the four: issue URL, reaction count, one line; group by app)

**Meetily** (ranked by reactions; four of the top asks are paid-only features in PRO, see below)
1. https://github.com/Zackriya-Solutions/meetily/issues/32 · 46 (31 👍) · open since 2025-02-21 · Linux support.
2. https://github.com/Zackriya-Solutions/meetily/issues/233 · 16 · closed · summaries in languages other than English.
3. https://github.com/Zackriya-Solutions/meetily/issues/226 · 15 · open · identify the speaker in transcripts. Related: #230 "Speaker Diarization / Identification", 7 reactions, 16 comments, open.
4. https://github.com/Zackriya-Solutions/meetily/issues/254 · 12 · closed · the app never asks for microphone permission on macOS.
5. https://github.com/Zackriya-Solutions/meetily/issues/376 · 12 · open · the Community Edition cannot change the recordings folder.
6. https://github.com/Zackriya-Solutions/meetily/issues/243 · 11 · open · make summary templates configurable.
7. https://github.com/Zackriya-Solutions/meetily/issues/241 · 9 · open · multitrack audio. Also #238 (9, closed): process meetings that were already recorded.
8. https://github.com/Zackriya-Solutions/meetily/issues/218 · 6 · open · auto-record when a meeting is detected. Also #387 (2 reactions, 11 comments).
- Further down: #301 (8) custom OpenAI-compatible transcription API · #333 (7) not using the CUDA GPU · #474 (6) domain vocabulary hints (Whisper initial_prompt).
- **The paywall sits on these asks.** Meetily PRO ($10/user/month billed yearly as $120, or $25 monthly) includes "Speaker diarization (who said what)", "Custom summary templates", "Auto-detect meetings", "Advanced exports (PDF, DOCX, Markdown)" and "AI assistant & script integrations (MCP, webhooks, CLI)". The Community edition has "No Pro features" (https://meetily.ai/pricing).

**OpenWhispr** (mostly dictation asks; its meeting complaints are new and have no reactions yet)
9. https://github.com/OpenWhispr/openwhispr/issues/459 · 11 · open · Android app.
10. https://github.com/OpenWhispr/openwhispr/issues/681 · 10 · closed · Homebrew package. Also #707 (8, closed).
11. https://github.com/OpenWhispr/openwhispr/issues/1400 · 5 · open · cancel dictation in hold/push-to-talk mode. Also #1100 (5) D-Bus crash on Wayland and #1329 (5) other Wayland compositors.
12. Meeting quality reports with no reactions:
    - https://github.com/OpenWhispr/openwhispr/issues/2582 (2026-10-09, open): "Meeting echo stays in the note when a later mic segment was captured earlier".
    - #323 (1, closed): "Small local models Llama 3.2 3B hallucinate output".
    - #1458 (closed): Whisper invents text on trailing silence ("Thank you", "Продолжение следует…").

**Minutes** (reactions ≤ 1, so these show where the product struggles, not how many people want something)
13. Call capture and detection:
    - https://github.com/silverstein/minutes/issues/1110 (closed 2026-10-07): Go Live in a Google Meet call captured only the user's mic.
    - #244: call detection fires falsely when a dictation tool is the app using the mic.
    - #286 (open): Meet and Teams-web detection is opt-in.
    - #69 (1): Linux PipeWire monitor capture.
14. Speaker naming and agent access:
    - https://github.com/silverstein/minutes/issues/245 (1, closed): speaker attribution replaced the user's configured name with a name spoken in the transcript.
    - #933 (closed): the MCP server's 15-second corpus-lease budget "denies every query on a real corpus".
    - #315 (closed): the Whisper model was reloaded for every VAD chunk, 227 times in one 49-minute meeting.

**Anarlog** (no tracker, so this comes from the 180-comment Launch HN thread, https://news.ycombinator.com/item?id=44725306)
15. Speaker ID, analytics and missing features:
    - Speaker ID was advertised before it worked. "the inability to tell who said what is a show stopper." Another user: "a 30-minute meeting with 20 people and it put the entire conversation under a single" speaker. The maintainer admitted the landing page was "outdated".
    - Telemetry versus "local": "You might want to reconsider 'no data ever leaves your machine'". A Linux build "keeps panicking because it requires dozen(s) of API key[s]"; the maintainer replied that only PostHog and Sentry keys are needed, for release builds, and that analytics can be turned off.
    - Feature asks: "automated meeting detection, push to transcribe (with custom rewrite actions)", "Is there any chance of a headless mode?", Windows and Linux.

## 4. Granola as the reference product (2026 features: enhance-your-notes, templates/recipes, chat across meetings, people and company pages, sharing, calendar, iOS; price; what users love and hate; what each of the four copies)

**Product** (https://www.granola.ai/)
- "Effortless notes, enhanced instantly": the user types rough notes and AI enhances them.
- "Uses your computer audio, so doesn't invite a bot". "Private by default". Available for macOS, Windows, iOS and Android.

**Calendar**
- It "integrates with your calendar and automatically detects upcoming meetings".
- It "preps a Brief before every external meeting: who's attending, what you discussed last time, and what matters now".
- Pre-meeting briefs and follow-up emails are Business-tier only (https://www.granola.ai/pricing).

**2026 timeline** (from https://www.granola.ai/updates)
- Sign in with Microsoft (2026-01-15).
- Attendee cards on @mentions and smarter regeneration warnings (2026-01-08).
- Delete parts of a transcript (2026-01-28).
- Granola MCP for Claude, ChatGPT and Cursor, 4 Feb (https://www.granola.ai/blog/granola-mcp).
- Chat "rebuilt from the ground up to be agentic" across your notes, transcripts, Team Space and notes shared with you, with "inline citations", plus new Recipes, 21 Apr (https://www.granola.ai/blog/granola-chat-just-got-smarter).
  - These two posts carry no year; their position in the updates list places them in 2026.
- Android (2026-07-01, https://www.granola.ai/blog/introducing-granola-for-android).
- Apple Watch (2026-07-28, https://www.granola.ai/blog/granola-for-apple-watch; 28 points on HN).
- 32 languages (2026-09-02).

**Earlier features still in the product**
- People and Companies views, plus Attio and Zapier (2025-09-08, https://www.granola.ai/updates/three-new-features-customer-relationships).
- Team Folders (2025-07-14). "Edit notes just by asking" (2025-07-24). 29 built-in templates.
- Time remaining in the meeting (2025-12-12).
- iOS launch date: not found. The pricing page lists Desktop, iOS, Android and Apple Watch apps on every plan.

**Price** (https://www.granola.ai/pricing)
- **Basic $0:** 30-day history; chat across meetings with the model on "Auto only"; MCP limited to personal notes from the last 30 days; templates, Recipes and the Slack integration included.
- **Business $14/user/month:** unlimited history, model selection, pre-meeting briefs, follow-up emails, a workspace-wide dictionary, Notion/HubSpot/Attio/Affinity/Zapier, API, full MCP.
- **Enterprise $35/user/month:** SSO and SCIM, HIPAA, audit API, org-wide auto-deletion, admin controls over MCP and API, org-wide training opt-out.
- Every plan: "Audio deleted after transcription", SOC 2 Type II.

**What users love**
- Vendor-chosen quotes: Linear's CEO, "just clicked instantly"; Brex's CEO on "precise, reliable summaries" (https://www.granola.ai/).
- A Hyprnote-thread user: "I honestly don't care that much if Granola uploads my notes".

**What users hate: sharing and training defaults**
- The Verge, 2026-04-02: "By default, your notes are viewable to anyone with the link"; "Enterprise customers are opted out of AI training by default, but people on all other plans aren't"; transcripts are processed in the cloud; "at least one major company has denied use of the tool to a senior executive" (https://www.theverge.com/ai-artificial-intelligence/906253/granola-note-links-ai-training-psa).
- The same Hyprnote-thread user was "extremely disturbed to find out that Granola was automatically making each of my note…" links public (item 44725306).
- r/ArtificialInteligence thread: "Granola – your meeting notes are public!" (https://www.reddit.com/r/ArtificialInteligence/comments/1m9lox6/granola_your_meeting_notes_are_public/).
- Security: "One Click Account Takeover in Granola AI Notetaker" (Strix, HN 2026-07-21, https://news.ycombinator.com/item?id=48993371). The Strix page returned HTTP 500; only the title was seen.

**What each of the four copies from Granola**
- **Anarlog:** the repo description is literally "Open source Granola AI Alternative"; the launch post said "Think of it as an open-source Granola"; it copies the notepad plus template-summary flow.
- **Minutes:** "Granola/Otter alternative that Claude Code, Codex, Cursor, and any MCP client can query". It copies Granola's MCP direction, but with local files.
- **Meetily:** puts Granola-style features (templates, auto-detect, MCP) in its paid PRO tier.
- **OpenWhispr:**
  - A calendar-driven "Join & transcribe" prompt.
  - A "Detailed Notes" summary: "decisions, action items with owners and due dates, and open questions".
  - Its prompt card is "excluded from screen capture" (https://docs.openwhispr.com/help/meetings/record-a-meeting).
- Not copied by any of the four (not found): people/company pages and follow-up emails.

## 5. The rest of the field, one paragraph each, only what is evidenced: Ghost Pepper, Muesli, MacParakeet, ownscribe, Speakr, Scriberr; commercial: Otter, Fireflies, Fathom, Jamie, Bluedot, tl;dv, Krisp, Notion AI meeting notes, Apple Notes / macOS 26 call recording + summaries, Zoom AI Companion, Microsoft Teams Copilot — distinctive mechanism + price

Open-source and self-hosted projects are free.

**Ghost Pepper** (https://github.com/matthartman/ghost-pepper)
- 3,199 stars. The API detects no licence file. Last push 2026-07-27.
- "100% private on-device voice models for speech-to-text and meeting transcription on macOS".
- Its hold-to-talk Show HN reached 467 points and 200 comments (https://news.ycombinator.com/item?id=47666024).
- The "Ghost Pepper Meet" follow-up (local transcription plus diarization) got 27 points, with comments calling it "spam reposting" (https://news.ycombinator.com/item?id=47868032).

**Muesli** (https://github.com/Muesli-HQ/muesli)
- 1,382 stars · MIT · last push 2026-10-08.
- "agent-native local meeting transcription + dictation for macOS (Granola + WisprFlow alternative)".
- Show HN: 15 points (item 48011116). Optional cloud post-processing on top of local ASR.

**MacParakeet** (https://github.com/moona3k/macparakeet)
- 699 stars. The API says NOASSERTION; the Show HN title says GPL-3.0. Last push 2026-10-10.
- Dictation, file/media transcription, meeting recording, "Transforms" and "a public automation CLI".
- Show HN: 1 point (https://news.ycombinator.com/item?id=47525645).

**ownscribe** (https://github.com/paberr/ownscribe)
- 106 stars · MIT · last push 2026-08-14.
- "Local-first meeting transcription and summarization CLI". The same-day survey found WhisperX + pyannote and a bundled Phi-4-mini.

**Speakr** (https://github.com/murtaza-nasir/speakr)
- 4,077 stars · AGPL-3.0 · last push 2026-10-04. A self-hosted web app for transcribing recordings.

**Scriberr** (https://github.com/rishikanthc/Scriberr)
- 3,092 stars · MIT · last push 2026-09-20. "Self-hosted AI audio transcription".

**Otter** (https://otter.ai/pricing)
- Pro $16.99/user/month, or $8.49 billed annually. Business $30, $24 on promotion.
- The free tier lists 300 transcription minutes a month and a "New Otter MCP server". The capture mechanism is not shown on the page.

**Fireflies** (https://fireflies.ai/pricing)
- Free. Pro $18, or $10/seat/month billed annually. Business $29 / $19. API access listed. Mechanism not shown on the page.

**Fathom** (https://www.fathom.ai/pricing)
- Free forever. Team $19 / $15 per user (2-user minimum). Business $34 / $25.
- Team adds comments, folders and keyword alerts.

**Jamie** (https://www.meetjamie.ai/pricing, https://www.meetjamie.ai/)
- "No bot joins your call", "Data stays in the EU", "context enriched briefings from your past meetings" before each meeting.
- Free €0, Plus €21, Pro €39 (billed annually), Team €33. Includes "Ask Jamie (Chat)" and "MCP for AI tools"; Pro has a 5-hour meeting-length limit.

**Bluedot** (https://www.bluedothq.com/pricing)
- Chrome extension plus desktop and mobile apps.
- Free (5 meetings, lifetime), Basic $14 (audio only), Pro $20, Business $32. Webhooks and an MCP integration.

**tl;dv**: the pricing page is JavaScript-only and returned an empty shell. Not found.

**Krisp** (https://krisp.ai/pricing/)
- A "Bot-free" note-taker built around its "#1 Noise Cancellation"; also takes in-person notes and does accent conversion.
- Core $16, or $8/user/month annually. Advanced $30 / $15.

**Notion AI Meeting Notes** (https://www.notion.com/help/ai-meeting-notes)
- Requires the Business or Enterprise plan.
- The desktop app "captures all sides of the conversation using your system audio and mic". The browser captures the mic only.

**Apple**
- Notes on Mac records and transcribes audio, and Apple Intelligence can summarise the transcript (https://support.apple.com/guide/notes/record-and-transcribe-audio-apdb5106e334/mac).
- Phone and FaceTime audio calls can be recorded in the Phone app "starting in iPadOS 26 or macOS Tahoe" (https://support.apple.com/en-us/121583), with transcript summaries on Mac (https://support.apple.com/guide/mac-help/use-apple-intelligence-in-phone-mchld69597ca/mac).
- It ships with the OS; no separate price was found.

**Zoom AI Companion**
- Announced as "available at no additional cost" to paid users (https://news.zoom.com/zoom-ai-companion/; that is the 2023 announcement).
- 2026 add-on pricing: not found.

**Microsoft Teams**
- Facilitator "Generates real-time AI-generated notes during meetings", summarises decisions and open questions, and answers questions during the meeting. "A Microsoft Copilot license is required" (https://support.microsoft.com/en-gb/teams/copilot/facilitator-in-microsoft-teams-meetings).
- Intelligent recap needs Teams Premium or a Copilot licence; audio recap needs Copilot (https://learn.microsoft.com/en-us/microsoftteams/intelligent-recap-calls-meetings). Licence prices: not fetched.

**Newcomer:** Wispr Flow, a dictation app, is launching "a Granola-styled meeting notetaker" (TechCrunch, 2026-08-05; headline only, https://techcrunch.com/2026/08/05/wispr-flow-is-preparing-to-launch-a-meeting-notetaker-updated-terms-suggest/). Dictation apps are turning into meeting apps: OpenWhispr, Ghost Pepper, Muesli and MacParakeet all started as dictation tools.

## 6. Technique state of the art 2026 for local meeting assistants: echo cancellation for mic vs system audio, diarization models and voiceprints, Parakeet/Whisper/Moonshine/Apple SpeechAnalyzer quality, what local LLM sizes produce acceptable meeting notes, meeting auto-detection techniques, calendar-driven speaker naming, live AI assistance during calls

**Echo between the mic and system tracks.** No vendor documents an echo-cancellation method; that is not found. User reports show it is still unsolved in shipping apps:
- OpenWhispr #2582 (2026-10-09): echo stays in the note.
- Meetily #642: "Attribute transcripts to microphone or system audio". Meetily #337: muted-mic awareness.
- Minutes #280: native-call capture rejected when the mic track is quiet on a USB speakerphone.
- Minutes #1110: only the mic was captured in Google Meet.
- Bluetooth headsets break capture: Meetily #450, OpenWhispr #2001.

**Diarization.** Figures are DER %, legacy pyannote 3.1 → community-1 (open, CC-BY-4.0, works offline) → precision-2 (paid, hosted), from https://huggingface.co/pyannote/speaker-diarization-community-1.

| Benchmark | 3.1 | community-1 | precision-2 |
|---|---|---|---|
| AMI IHM | 18.8 | 17.0 | 12.9 |
| AMI SDM | 22.7 | 19.9 | 15.6 |
| DIHARD 3 | 21.4 | 20.2 | 14.7 |
| CALLHOME | 28.5 | 26.7 | 16.6 |
| VoxConverse | 11.2 | 11.2 | 8.5 |

- community-1 adds "exclusive" diarization (one speaker per moment) "for simpler reconciliation with transcription timestamps".
- How the apps handle it:
  - Minutes uses pyannote (https://useminutes.app/).
  - OpenWhispr labels speakers live and cleans them up after the call: "Still identifying speakers. Labeling accuracy will improve once the call ends." (docs).
  - Meetily keeps diarization in PRO.
  - Hyprnote's early speaker ID failed in a 20-person meeting (§3).
- Measured accuracy of voiceprint recognition across meetings: not found.

**ASR quality**
- **Parakeet TDT 0.6B v3:** 25 European languages with automatic language detection, word and segment timestamps, up to 24 minutes of audio with full attention or 3 hours with local attention, CC BY 4.0 (https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3).
- **Argmax benchmark, June 2025** (~12 h subset of earnings22, conversational English; https://www.argmaxinc.com/blog/apple-and-argmax). WER % and speed (seconds of audio per second):

| Model | WER % | Speed |
|---|---|---|
| Apple SpeechTranscriber | 14.0 | 70× |
| whisper-base.en | 15.2 | 111× |
| whisper-small.en | 12.8 | 35× |
| parakeet-v2 | 11.7 | 359× |

  - SpeechAnalyzer: 10 languages, macOS/iOS 26+, no diarization, no language detection.
- **Moonshine:** claims models from "higher accuracy than Whisper Large V3" down to "tiny 1MB", tuned for streaming with low latency (vendor, https://github.com/moonshine-ai/moonshine).
- **Hallucination and performance traps:** Whisper invents tails on silence (OpenWhispr #1458); reloading the model per VAD chunk (Minutes #315).

**What local LLM size gives acceptable notes**
- No measured threshold was found.
- Evidence so far:
  - "Small local models Llama 3.2 3B hallucinate output" (OpenWhispr #323).
  - A review lists "the default local summarizer is a small model" as a limit of Hyprnote (kompozy, see the caveat in §2).
  - Bundled models seen in the same-day survey: HyprLLM 1.7B (Anarlog), Phi-4-mini (ownscribe). Muesli says its best notes come via ChatGPT or Claude accounts.
  - Minutes: "Summarization is optional; Claude can do it conversationally."
  - HN is split: "I want to use the best possible model for it" versus "never understood why an AI meeting notetaker needed sota LLMs" (item 44725306).

**Meeting auto-detection.** OpenWhispr documents a design with two signals (https://docs.openwhispr.com/help/meetings/how-meeting-detection-works):
- The signals: "Sustained microphone activity" (this catches browser meetings) and "A calendar event" (Google, Microsoft or Apple).
- A running Zoom, Teams, Webex or FaceTime app is "only used as context" and never triggers a prompt on its own.
- Overlapping signals produce **one** prompt.
- It stays quiet while already recording, mid-dictation, during a cool-down after a recording, and after a dismissal.
- The mic prompt and the calendar prompt have separate switches.

Others:
- Minutes detects calls, with false positives from dictation tools (#244); it auto-stops when the call app exits (#129); auto-record is opt-in (#867).
- Granola detects upcoming meetings from the calendar.
- Meetily: auto-detect is PRO.
- Hyprnote (2025): "working on this".

**Calendar-driven speaker naming**
- A direct mapping from attendees to diarized speakers: not found in any product.
- What exists nearby:
  - Granola's Brief lists "who's attending", and attendee cards appear on @mentions.
  - OpenWhispr's "Join & transcribe" starts from the calendar link.
- Failure mode on record: an LLM assigned a name *spoken in the meeting* to the user (Minutes #245, also #491).

**Live AI during calls**
- Teams Facilitator: real-time co-authored notes, decisions and open questions, answers during the meeting. Enterprise, cloud, Copilot licence.
- Jamie and Granola: pre-meeting briefings from past meetings.
- Minutes: streaming partials "every few seconds".
- OpenWhispr: a live transcript tab, and its UI is hidden from screen capture.
- Granola: time remaining in the meeting.
- No local app was found doing tool-backed, mid-meeting research the way theuth's briefs do.

## 7. Map to the three questions (Q1 what they offer vs a harness like theuth · Q2 what to learn/borrow · Q3 how to make it better) — from the user-evidence angle

**Q1: what they offer that theuth (as AXES describes it) does not**
- Calendar-driven start and pre-meeting brief (Granola Business tier, Jamie, OpenWhispr).
- Auto-detection with suppression rules (OpenWhispr).
- Notes templates and Recipes (Granola; Meetily #243 with 11 reactions; Meetily charges for templates).
- External agent access over MCP:
  - Local: Minutes, 34 tools.
  - Cloud vendors: Granola, Otter, Jamie, Bluedot.
- Plain markdown on disk (Minutes; requested on Muesli HN).
- Phone and watch capture (Granola iOS, Android, Watch).
- Linux capture (Meetily #32, 46 reactions; Minutes #69).

**Q1: what theuth has that they do not**
- Tool-backed live briefs during the meeting. The closest equivalent is Teams Facilitator, which is cloud-only and licensed.
- Knowledge harvest across meetings plus a brief cache.
- Speaker enrollment and a correction pass, where the others show only provisional live labels.
- A general harness around all of it.

**Q2: techniques to learn (no code)**
- OpenWhispr's detection: two signals merged into one prompt, meeting apps as context only, mid-dictation and cool-down suppression, separate switches. Lands in theuth's meeting start.
- Granola's Brief recipe: attendees + "what you discussed last time" + "what matters now". Lands in theuth's brief cache and knowledge lookup, run *before* the meeting.
- Recipes: user prompts runnable across meetings (Granola, 21 Apr). Lands in theuth's skills/schedules.
- Chat with inline citations to transcript lines (Granola Chat). Lands in the meeting chat and `meeting_search`.
- Honest provisional speaker labels that settle after the call (OpenWhispr).
- Guards against known failure modes, usable as test cases:
  - echo duplicates (OpenWhispr #2582)
  - silence hallucinations (#1458)
  - per-chunk model reloads (Minutes #315)
  - a spoken name overriding the user's identity (Minutes #245)
  - an MCP time budget that starves real corpora (Minutes #933)
- The privacy lesson: Granola's link-sharing and training defaults drew press coverage and enterprise bans. Keep theuth private by default, with sharing explicit.

**Q3: the highest-leverage changes, in order, by user evidence**
1. **Calendar integration.** Pre-meeting brief, attendee list, auto-titles, plus the two-signal start prompt. These are the most-copied paid features: Granola Business, Meetily PRO, Jamie.
2. **A read-only MCP server and markdown export of the meeting history**, so Claude Code, Codex and others can query it. Demand: Hyprnote HN, Minutes' 2,556 npm downloads a month; Granola and Otter both added MCP in 2026.
3. **Note templates and Recipes per meeting type.** Meetily #243 and the PRO gating show users value them.
4. **Speaker naming from attendees plus enrollment**, guarding against names spoken in the meeting. Speaker ID is Meetily's #3 and #13 ask and Hyprnote's show-stopper.
5. **Echo and channel-attribution robustness, then Linux/Windows capture parity.** The most-reacted issue in the whole set is Meetily's Linux support at 46.

## 8. Sources (numbered URL list)

1. https://api.github.com/repos/Zackriya-Solutions/meetily
2. https://github.com/Zackriya-Solutions/meetily/releases
3. https://api.github.com/repos/fastrepl/anarlog
4. https://github.com/fastrepl/anarlog/releases
5. https://api.github.com/repos/OpenWhispr/openwhispr
6. https://github.com/OpenWhispr/openwhispr/releases
7. https://api.github.com/repos/silverstein/minutes
8. https://github.com/silverstein/minutes/releases
9. https://api.github.com/search/issues?q=repo:Zackriya-Solutions/meetily+is:issue&sort=reactions&order=desc (same query for each repo)
10. https://meetily.ai/pricing
11. https://news.ycombinator.com/item?id=44725306
12. https://news.ycombinator.com/item?id=43137186
13. https://news.ycombinator.com/item?id=48011116
14. https://www.reddit.com/r/LocalLLaMA/comments/1n0zcx2/free_local_ai_meeting_notetaker_hyprnote_obsidian/
15. https://useminutes.app/
16. https://docs.openwhispr.com/help/meetings/record-a-meeting
17. https://docs.openwhispr.com/help/meetings/how-meeting-detection-works
18. https://www.granola.ai/pricing
19. https://www.granola.ai/
20. https://www.granola.ai/updates
21. https://www.granola.ai/blog/granola-chat-just-got-smarter
22. https://www.granola.ai/blog/granola-mcp
23. https://www.granola.ai/blog/granola-for-apple-watch
24. https://www.granola.ai/blog/introducing-granola-for-android
25. https://www.granola.ai/updates/three-new-features-customer-relationships
26. https://www.theverge.com/ai-artificial-intelligence/906253/granola-note-links-ai-training-psa
27. https://news.ycombinator.com/item?id=48993371
28. https://www.reddit.com/r/ArtificialInteligence/comments/1m9lox6/granola_your_meeting_notes_are_public/
29. https://techcrunch.com/2026/08/05/wispr-flow-is-preparing-to-launch-a-meeting-notetaker-updated-terms-suggest/
30. https://kompozy.io/reviews/hyprnote (review site selling its own product; low weight)
31. https://andrew.ooo/posts/meetily-privacy-first-ai-meeting-assistant-review/ (blog; contains a factual error)
32. https://github.com/matthartman/ghost-pepper
33. https://news.ycombinator.com/item?id=47666024
34. https://news.ycombinator.com/item?id=47868032
35. https://github.com/Muesli-HQ/muesli
36. https://github.com/moona3k/macparakeet
37. https://news.ycombinator.com/item?id=47525645
38. https://github.com/paberr/ownscribe
39. https://github.com/murtaza-nasir/speakr
40. https://github.com/rishikanthc/Scriberr
41. https://otter.ai/pricing
42. https://fireflies.ai/pricing
43. https://www.fathom.ai/pricing
44. https://www.meetjamie.ai/pricing
45. https://www.meetjamie.ai/
46. https://www.bluedothq.com/pricing
47. https://tldv.io/pricing (empty shell, not read)
48. https://krisp.ai/pricing/
49. https://www.notion.com/help/ai-meeting-notes
50. https://support.apple.com/guide/notes/record-and-transcribe-audio-apdb5106e334/mac
51. https://support.apple.com/en-us/121583
52. https://support.apple.com/guide/mac-help/use-apple-intelligence-in-phone-mchld69597ca/mac
53. https://news.zoom.com/zoom-ai-companion/
54. https://support.microsoft.com/en-gb/teams/copilot/facilitator-in-microsoft-teams-meetings
55. https://learn.microsoft.com/en-us/microsoftteams/intelligent-recap-calls-meetings
56. https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3
57. https://huggingface.co/pyannote/speaker-diarization-community-1
58. https://www.argmaxinc.com/blog/apple-and-argmax
59. https://github.com/moonshine-ai/moonshine
60. https://news.ycombinator.com/item?id=44901853
61. https://hn.algolia.com/api/v1/search?query=Hyprnote&tags=story (HN points and comments, same pattern for each query)
62. Internal: knowledge `facts/oss-local-meeting-notetakers-survey-2026-10-10` (same-day survey, used only for the facts marked "same-day survey")