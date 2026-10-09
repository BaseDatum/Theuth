# REPORT

Read-only web survey: FluidVoice (altic-dev), how users receive it, and the state of desktop dictation and voice-command tools in October 2026. Time of survey: 2026-10-09, 16:20–16:55 BST.

Tool limits that affected coverage:
- The unauthenticated GitHub REST API ran out of quota partway through. Issue and release data therefore came from the authenticated GitHub MCP (`search_issues` / `list_releases` / `issue_read`). The numbers are the same, but I cite the human-readable github.com URLs.
- `search_web` was degraded: Brave was rate-limited, and Google, DuckDuckGo and Qwant blocked the requests. As a result, **Reddit was not reached** (r/macapps and r/LocalLLaMA reviews: not found). I did not reach the help pages for ChatGPT desktop voice, Perplexity voice, Raycast AI voice or Dragon (not found). Most facts below come straight from vendor pages, the repo and the HN Algolia API.

## 1. FluidVoice traction and release cadence (numbers, dates, URLs)

- **Repo:** altic-dev/FluidVoice. Created 2025-09-21. **11,968 stars, 867 forks, 50 watchers, 114 open issues+PRs.** Last push 2026-10-06. Licence GPL-3.0. Description: "Fastest and only macOS Dictation app with on-device STT and custom trained AI enhancement model. Windows pre-build available! A local Wispr Flow alternative." [1]
- **Licence history:** GPLv3 from 2026-02-23. Earlier versions were Apache-2.0 [2].
- **Vendor claims:** "100,000+ downloads". Requires macOS 15.0+ (Apple Silicon and Intel). "Free forever, no tiers". Installs with `brew install --cask fluidvoice` [2].
- **Search interest:** 67 open issues match an open-issue search sorted by reactions [3].
- **macOS stable cadence** (from the 30 most recent releases) [4]:
  - v1.5.12 2026-04-08 · v1.5.13 05-02 · v1.5.14 05-29 · v1.5.15 06-04
  - v1.6.0 06-22 · v1.6.1 06-28 · v1.6.2 07-06 · v1.6.3 07-14 · v1.6.4 07-14 · v1.6.5 07-21 · v1.6.6 07-31 · v1.6.7 08-05 · v1.6.8 08-11 · v1.6.9 08-18
  - That is **14 stable releases in about 4.3 months**, roughly one every 9 days.
  - No stable release since 2026-08-18. Betas since then: v1.6.10-beta.7 on 2026-09-23 and v1.6.10-beta.8 on 2026-10-06 [4].
- **Windows line:** windows-v0.0.1 on 2026-07-08 through windows-v0.0.10 on 2026-09-08, 10 pre-releases in 2 months. Separate runtime artefacts: `windows-runtime-parakeet-v0.4.0-fv1` (2026-09-08) and `windows-runtime-llama-b9870-argmax-v2` (2026-07-14) [4].
- **Highlights by release:**
  - beta.8: Fluid Speech Mini/Pico (smaller English-only local models with custom words and pronunciation training). Model-weight updates without an app update, with resumable, verified downloads that keep the old model if the update fails. Pinned transcripts. A **local API to list meetings and fetch transcripts** [5].
  - beta.7: experimental pronunciation training "that learns how you say words—not just text replacements". Personalised dashboard. Idle unloading of Fluid Intelligence. The full raw transcript is kept when Fluid-1's context is exceeded [6].
  - v1.6.3: Fluid-1 is "2.2x faster on Apple silicon". AI dictation "fails closed when Fluid-1 is unavailable". Train by Voice keeps earlier samples [7].
  - v1.6.2: dictionary learns from voice ("say a difficult word a few times, then save the misheard versions as one replacement"). Fluid Intelligence handles ~2,000 words, up from ~200. Spoken punctuation converts to symbols without AI. Optional speed boost (+15% speed, +~100 MB) [8].
  - v1.5.14: "Synced Command Mode with AI Enhancement by default, with inline model controls, clearer provider errors, overlay notifications, and improved Thinking display" [9].
- **Hacker News is thin.** The only "FluidVoice" story is 48739409 (2026-06-30, 2 points, 1 comment) [10]. About ten comment mentions in Apr–Oct 2026 (§2) [11].

## 2. What users praise (top 8, with evidence)

1. **Fully local and private, a "non-cloud Wispr Flow".** Asked for a non-cloud Wispr alternative: "FluidVoice!! They are cooking up smaller and smaller dictation polish models to do everything locally." (HN 49812281, 2026-09-23) [11]. "privacy focused with a bit more enhancements like custom dictionary" (HN 49619313, 2026-09-09) [11].
2. **Raw dictation is fast.** "When I delete the fluid intelligence model … the words appear in the textbox in the same second. It's amazingly fast" (#925) [12]. The vendor claims <100 ms perceived latency (demo shows 86 ms) [2].
3. **A complete local stack.** "Kokoro for TTS, Parakeet for STT and Fluid-1 for text formatting (I use it with FluidVoice)" (HN 48824163) [11]. "FluidVoice has been working well enough for me on my laptop" (HN 49809811) [11].
4. **Free and open source.** The vendor page quotes "Thank you for making it free, forever!" and "super cool of you to make it open source" [2]. Issue #29: "I greatly appreciate the ongoing development and truly love the app" (together with a stability complaint) [13].
5. **Command Mode is liked for built-in Mac actions.** "Command Mode is great for the built-in Mac actions it already knows how to take, but the set of things it can do is fixed" (#275, which asks for MCP) [14].
6. **Recovers intent from stumbles.** Vendor-curated testimonial: "I stumbled while dictating and the transcriber caught it and output the word I had intended to say!" [2].
7. **Multilingual and file transcription.** Vendor-curated: works in German and French, including transcribing a video [2]. Model lineup covers Nemotron Speech 3.5 (~40 languages), Parakeet TDT v3 (25), Cohere Transcribe (14), Whisper (up to 99) and Apple Speech [2].
8. **Speed of shipping and responsiveness.** 14 stable releases in ~4 months (§1). Long-running requests were closed by shipping: Homebrew cask (#92, 10 reactions, closed; the site now shows `brew install --cask fluidvoice`) [15][2], and auto-learned vocabulary (#272 closed, then voice-trained dictionary in v1.6.2 and v1.6.3) [16][8][7].

Counterpoint from HN: "I tried FluidVoice (found it buggy) and Handy (had problems with hotkey) before eventually landing on TypeWhisper" (HN 48714104) [11].

## 3. What users complain about / request (top 12, with issue URLs and reaction counts)

Format: reactions / comments. Ranked by reactions plus comment volume.

1. **Crashes and hangs when the audio device changes (AirPods, mic switching).**
   - #644 crash on audio source switch, 4/15 [17]
   - #682 hang on device change in 1.6.5 (AVAudioEngine stalls in `_GetHWFormat`), 1/19 [18]
   - #665 SIGABRT on device change, 0/15 [19]
   - #709 hang switching to AirPods, 1/13 [20]
   - #788 wrong mic after update, 2/14 [21]
   - #694 flapping between mics, 4/10 [22]
   - #846 AirPods 3 disconnects, 0/15 [23]
   - Earlier crash wave: #62 Sonoma crash, 5/18 [24]; #159 SIGTRAP during streaming after warmup, 5/5 [25]
   - #29 "frequent crashes, often without any warning", 2/22 [13]
2. **Text not reaching the target app (paste/insert).**
   - #479 dictated list not inserted in Claude Code under cmux, only "a" appears, 3/16 [26]
   - #1013 incomplete pastes in VS Code, 0/6 [27]
   - #469 won't paste into any app, 1/2 [28]
   - #213 pastes the clipboard instead of the dictation, 1/13 [29]
   - #555 transcribed text not inserted, 0/12 [30]
   - #43 auto-paste fails sometimes, 0/9 [31]
   - #929 dictation overwrites the system clipboard, 0/10 [32]
   - Requests: #481 "None" insertion mode [33]; #627 onboarding without Accessibility, using a clipboard fallback, 1/4 [34]
3. **Fluid Intelligence (closed local LLM) is slow or drops output.** #925, 1/19: "takes around 5 seconds or even longer … Sometimes the input doesn't appear at all" on an M3 Pro with 36 GB [12].
4. **The AI cleanup answers the transcript instead of cleaning it, or truncates it.**
   - #388, 2/4: puts user instructions in the user message next to the transcript, "much more likely to lead to failed instruction following" — move them to the system prompt [35]
   - #167 "AI responds to the sentence instead of improving it", 0/8 [36]
   - #457 enhancement cuts off parts, 5/12 [37]
5. **Mic stops picking up audio after 3 seconds.** #852, 1/19 (keep-open) [38].
6. **Hotkey reliability.**
   - #751 trigger key has a startup delay and drops opening/closing words (1.6.6 regression), 0/15 [39]
   - #337 hotkey dead on boot until the app is opened, 2/1 [40]
   - #373 push-to-talk slowdown, 0/9 [41]
   - #498 deleted modifier-only shortcut stays active, 1/2 [42]
   - #422 Right Cmd always triggers Command Mode, 0/3 [43]
   - #1088 "Hotkey Initializing...", 1/1 [44]
7. **Resource use.** #948 high idle CPU over long sessions (204 CPU-min in ~17 h), 3/7 [45].
8. **The overlay lies about state.** #676 the normal-dictation overlay hides before transcription and insertion finish, leaving no processing feedback, 1/4 [46].
9. **Command Mode safety and visibility.**
   - #377 require confirmation for all commands, 0/0 [47]
   - #444 appears frozen while running broad terminal commands, 0/0 [48]
   - #446 blocked by macOS permission dialogs hidden behind its overlay, 0/0 [49]
   - #275 MCP client support, 0/9 [14]
   - #308 make Command Mode's system prompt a customisable profile, 1/0 [50]
   - #35 / #165 can't disable it, or disabling needs a restart [51][52]
10. **Write Mode cannot read the selected text in other apps.** #259, 0/3: in Zed the selection was replaced by "please provide the text you want translated" [53].
11. **Configuration and models on disk.**
    - #331 keep config in `~/.config`, 5/4 (the most-reacted open issue) [54]
    - #405 custom or shared model directory, 2/1 [55]
    - #616 multiple copies downloaded on update, 0/3 [56]
12. **Languages and audio etiquette.**
    - #506 several active languages at once, 4/6 [57]
    - #100 preferred languages, 3/2 [58]
    - #979 mute audio while capturing, 4/2 [59]
    - #1023 volume ducking, 1/2 [60]
    - Also: #463 automatic history clearing, 3/1 [61]; #518 auto-Enter after paste when a modifier is held, 1/4 [62]; #406 spoken punctuation and editing commands, 1/3 [63]; #18 diarization, 10/7, closed [64]

## 4. Command mode and Write mode in the field — FluidVoice vs the others (table)

| Product | Voice command / agent mode | Rewrite-selected-text mode | How results are delivered | Safety / confirmation |
|---|---|---|---|---|
| FluidVoice | "Command Mode": the LLM returns tool calls that run terminal commands (#444) [48]. Has a model picker and a "Thinking display" (v1.5.14) [9]. Marketed as one of three "Predefined modes: Write Mode, Command Mode, and Direct Dictation" [2] | "Write Mode": select text, press Right Option+R, dictate an instruction, and the selection is replaced (#259) [53]. Called "Edit Mode" in v1.6.6 and not usable with Fluid-1 [65] | Command overlay window. Users report it covering macOS permission dialogs (#446) [49] and looking frozen during long commands (#444) [48] | Not every command needs confirmation; #377 asks for that [47]. No MCP yet (#275) [14] |
| Wispr Flow | "Command Mode": hold Fn+Ctrl (Mac) or Ctrl+Win+Alt (Win), speak, release. Off by default (Settings → Experimental). No "Hey Flow" prefix needed. Spoken trigger phrases are English-only [66] | Edit or draft text in the focused text box [66] | Edited text is "pasted as returned, without normal dictation formatting". Searches open Perplexity with the selected text appended. App-building commands open in the browser [66] | Esc/Backspace cancels with a notification. Releasing before listening starts cancels. Warns at 19 min, stops at 20 min [66] |
| Superwhisper | Modes; plus **coding-agent plugins** (Claude Code, Codex, Cursor, OpenCode…): it "pops up when your agent needs your input or finishes working" [67] | Context checkboxes (Application, Copied text, Selected text; Pro) feed AI processing [68] | Done notifications with the agent's final message [67] | "Voice approvals": approve or deny gated tool calls by speaking [67] |
| VoiceInk | "AI Assistant: Built-in voice assistant mode for a quick chatGPT like conversational assistant" [69] | "Context Aware: … understands your screen content" [69] | Not found | Not found |
| Typeless | "Ask anything" [70] | "Select any text and speak your command … shorter, longer, change its tone, or even generate a creative response" [70] | Inline replacement [70] | Not found |
| Aqua Voice | "'Send it' voice command" (Max tier) [71] | Custom Instructions (Pro) [71] | "See words live with Realtime Mode" (Max) [71] | Not found |
| Talon | Command, dictation, mixed and sleep modes driven by a command grammar (community `.talon` files) [72] | Via grammar commands [72] | Direct key and action injection [72] | Sleep mode until a wake command [72] |
| Claude (desktop/web) | Voice mode on desktop and web, "built to work best from your phone". Uses web search and connected tools (Gmail, Calendar, Docs, Slack) during voice [73] | n/a | Spoken response in a chat window; can switch between text and voice in one conversation [73] | Push-to-talk option for noisy settings [73] |
| ChatGPT desktop / Perplexity / Raycast | Not found (help pages blocked, or the page did not mention voice) [74] | — | — | — |

## 5. The field (one paragraph per product, price, distinctive mechanism)

**Wispr Flow.** Mac, Windows, iOS and Android. Free tier capped at 2,000 words/week on desktop and 1,000 on mobile. Pro $15/user/mo monthly or $12 annual; Growth $23–33 monthly or $18–26 annual; Enterprise custom [75]. What sets it apart: "Flow edits while you speak" ("Backtrack", filler removal, numbered lists, auto-punctuation). Its **dictionary learns automatically**: "When you correct a spelling, Flow adds it automatically to your personal dictionary". Snippets are spoken cues that paste canned text. "Styles" set tone per app (English, desktop only). It uses surrounding context to spell names, works when whispering, and does dev file tagging [76]. Command Mode is in §4 [66]. The Notetaker product exposes notes to AI tools over MCP [75].

**Superwhisper.** macOS, Windows, iOS, Android. Free with local Whisper. Pro $8.49/mo, $84.99/yr or $249.99 lifetime [77]. What sets it apart: **Modes**, each pairing a voice model with optional AI processing. Settings per mode: language, model, "Activate for apps" auto-switching, Playback when recording (Keep/Pause/Lower/Mute), system-audio capture, speaker identification. Message/Mail/Note modes have a Tone slider. Context checkboxes cover app, copied text and selected text [68]. It also has a CLI and MCP server over history, a doc on tuning modes and vocabulary from dictation history with a coding agent [78], and the coding-agent voice-approval plugins [67].

**VoiceInk.** macOS, now also iOS. Open source (GPL per the repo). Paid licence for updates: $25 for 1 Mac, $39 for 2, $49 for 3, one-time with lifetime updates [69][79]. What sets it apart: "Modes" that auto-apply settings by app or URL ("Power Mode" in earlier marketing). Screen-content context awareness. Personal dictionary plus "Smart Replace" expansions. A built-in AI Assistant voice mode [69][80].

**MacWhisper.** By Jordi Bruin, macOS, closed source. Free tier uses small Whisper models. Pro is a one-time Gumroad licence ("~$30" per a third-party summary; I could not read the Gumroad listing) [81]. What sets it apart: a polished file-transcription editor with diarization, batch processing, system-wide dictation, AI summaries and SRT/VTT/PDF export [81].

**Aqua Voice.** Free gives 1,000 words and the Avalon model. Pro $8/mo annual or $10 monthly. Max $24/mo annual or $30 monthly. Business $24/seat/mo [71]. What sets it apart: Realtime Mode shows words live, and a "Send it" voice command (both Max), plus Custom Instructions [71].

**Willow Voice.** Free gives unlimited "Frontier Mini" (the weaker model) and 20 "Scribe" uses a week. Pro $15/mo or $12 annual. Business $35 or $28. Mac, iOS and Windows; Android "coming soon" [82]. What sets it apart: "Smart memory of your writing style", style matching per context, a personal dictionary, and a privacy mode [82][83].

**Typeless.** Mac, Windows, Linux, iOS, Android. Free with limited words. Pro $12/member/mo annual or $30 monthly [84]. What sets it apart: auto-edits when you change your mind mid-sentence, removes repetition, a dictionary that can be "added automatically or manually", per-app tone, live translation, and "Ask anything" voice editing of the selection [70].

**Monologue (Every).** Mac, iPhone, iPad, Apple Watch. Price not found on the home page. What sets it apart: its own dictation model "mono-1". It reads the code around the cursor to get file names right for coding-agent prompts. Self-corrections handled. Voice notes and bot-free meeting transcripts handed to agents [85].

**Handy.** Open source, free, Windows/macOS/Linux. What sets it apart: hold-to-record **or** tap-to-toggle on one shortcut, with Hold-only and Toggle-only modes. Silero VAD filters silence. Whisper (with GPU) or Parakeet V3 (CPU-optimised, automatic language detection). Pastes into the focused app [86]. On HN one user reports "had problems with hotkey" [11].

**Whispering (Epicenter).** The Epicenter repo README now describes a local-first Yjs store and the "Honeycrisp" notes app under AGPL-3.0. I could not confirm Whispering's current status or features from it (not found) [87].

**Apple macOS 26: SpeechAnalyzer.** A `SpeechAnalyzer` holds modules; most uses only need `SpeechTranscriber`, which analyses audio "from files or capture devices" [88]. Price and the system-dictation UX: not found in what I read.

**Windows 11: Voice Access and Voice Typing "Fluid dictation".** Copilot+ PCs only. It fixes grammar, punctuation and recognition errors in real time. French, German and Spanish were added in July 2026 [89]. Starting with builds 26100.8973 / 26200.8973 / 28000.2804, Fluid dictation is **off by default for new users** [90]. Free with Windows.

**Talon.** Command/dictation/mixed/sleep modes plus a user-editable command grammar (talonhub community files) [72]. Price not found in what I read. Dragon: not surveyed (time).

## 6. Technique state of the art 2026 (EOU streaming, LLM cleanup, learned dictionary, gestures, agent voice UIs)

**EOU and streaming ASR.**
- NVIDIA `parakeet_realtime_eou_120m-v1` [91]:
  - Streams and detects end of utterance by emitting an `<EOU>` token, at "80ms~160 ms" latency.
  - English only. No punctuation or capitalisation.
  - Cache-aware streaming FastConformer with 17 encoder layers, attention context [70,1], RNNT decoder, 120M parameters.
  - Needs at least 160 ms of 16 kHz audio. Built for NeMo Voice Agent pipelines. NVIDIA Open Model License.
- NVIDIA `nemotron-speech-streaming-en-0.6b` [92]:
  - 600M parameters, cache-aware streaming, native punctuation and capitalisation, "configurable low-latency streaming chunk sizes".
  - Updated checkpoint on 2026-03-12. The page also mentions a multilingual sibling with language detection.
- FluidVoice ships "Nemotron Speech 3.5 (Ultra Fast Low Latency and Multilingual)" and "Parakeet Flash (Beta)", and claims "optimized routing … across short bursts and long dictation sessions" [2].

**LLM post-processing ("Flow"-style).** The 2026 norm is filler removal, backtracking or self-correction, list formatting and a tone per app:
- Wispr: Backtrack and Styles [76]. Typeless [70], Monologue [85], VoiceInk [69] and Superwhisper's Tone slider [68] do the same.
- FluidVoice trained its own local model, Fluid-1 (~3.5 GB optional download) [2], with Fluid-1 Mini at 1.5 GB (windows-v0.0.8) [93]. It picks a prompt by detecting the focused app [2]. Monologue also trained its own (mono-1) [85].
- Lessons from the field:
  - Put the instructions in the **system prompt**, never next to the transcript, or models answer the transcript (#388) [35].
  - An on-device LLM step can turn instant dictation into 5 s or more (#925) [12].
  - FluidVoice now "fails closed when Fluid-1 is unavailable" [7] and keeps the raw transcript when context overflows [6].
  - FluidVoice Windows decides AI cleanup **per dictation by which shortcut was pressed**: the normal shortcut never uses it, the Fluid Intelligence shortcut always does, and an overlay selector can override the dictation in progress (windows-v0.0.7) [93].
  - Windows Fluid dictation does the cleanup live, but only on Copilot+ PCs [89].

**Learned personal dictionary.**
- Wispr adds a word automatically when you correct its spelling [76]. Typeless adds entries automatically or manually [70].
- FluidVoice went another way: **pronunciation training**. "Train by Voice" stores spoken samples of a word and maps misheard variants to it (v1.6.2, v1.6.3, beta.7), works with Parakeet vocabulary boosting (#272), and keeps samples across model versions (beta.8) [8][7][6][5][16].
- Superwhisper documents building vocabulary and modes from dictation history with a coding agent [78].

**Gestures.**
- Handy supports hold and toggle on one key, plus hold-only and toggle-only [86].
- Wispr: hold to command, release to run. Esc cancels. Releasing before listening starts cancels. A system shortcut pressed within the first second cancels. Hard cap at 20 min. Separate Push-to-talk and Hands-free bindings. Caps Lock is allowed alone only for Push-to-talk and Hands-free [66].
- Claude offers push-to-talk as the option for noisy settings [73].
- FluidVoice supports several primary shortcuts and mouse buttons (v1.6.1) [94]. Its users hit startup-delay word loss (#751) and modifier-only ghosts (#498) [39][42].
- Published research comparing hold and toggle, or on the double-tap-to-lock pattern: **not found**.

**Agent voice UIs.**
- Superwhisper's agent plugins: pop up on an agent question or turn end, dictate the answer, approve or deny tool calls by voice, and get a done notification with the final message [67].
- Wispr Command Mode pastes results inline or hands them to the browser or Perplexity [66].
- FluidVoice Command Mode uses an overlay with a Thinking display [9]. Its failure modes (frozen-looking during long tool runs, covering OS permission dialogs) are documented in #444 and #446 [48][49].
- Claude voice uses connected tools during a spoken conversation [73].
- ChatGPT desktop voice, Perplexity and Raycast AI voice UIs: not found.

## 7. Map to the three questions (Q1 better dictation · Q2 command mode · Q3 ask-the-agent-anywhere with visible output)

**Q1: better dictation.**
- *What the field does:*
  - Learned vocabulary, either from corrections (Wispr [76], Typeless [70]) or from spoken samples (FluidVoice Train by Voice [8][7]).
  - Spoken punctuation converted to symbols without AI (FluidVoice v1.6.2) [8].
  - Tone per app chosen by the focused app (FluidVoice [2], Wispr Styles [76], Superwhisper auto-activation [68]).
  - AI cleanup chosen per press by a second shortcut (FluidVoice Windows) [93].
  - Ducking or muting media while recording (Superwhisper [68]; FluidVoice requests #979 and #1023) [59][60].
  - Realtime word preview (Aqua Max) [71].
- *What users say:*
  - The top pains are **delivery failures** (#479, #1013, #213, #929) and **audio-device robustness** (#644, #682, #709). Engine quality is not where the complaints are [26][27][29][32][17][18][20].
  - The overlay must stay visible until insertion finishes (#676) [46].
  - Keep the clipboard intact (#929) [32].
  - Make AI cleanup optional per dictation, because LLM latency kills the "instant" feel (#925) [12].
  - Keep cleanup instructions in the system prompt (#388) [35].

**Q2: command mode.**
- *What the field does:*
  - Wispr: hold-speak-release, no wake phrase, result pasted raw, search handed to Perplexity, cancel with Esc, hard time cap [66].
  - FluidVoice: an LLM with tool calls into a terminal, a model picker, a Thinking display [48][9].
  - Superwhisper: voice approvals of tool calls [67].
  - Talon: an explicit grammar with modes [72].
- *What users say about FluidVoice's Command Mode:* they want confirmation for every command (#377), MCP tools (#275), a customisable system prompt (#308), progress shown while long commands run (#444), OS permission prompts kept reachable (#446), and an easy way to disable it (#35, #165) [47][14][50][48][49][51][52].

**Q3: ask the agent anywhere and see the output.**
- *What the field does:*
  - Superwhisper is the closest shipped equivalent: it surfaces at the agent's questions and turn ends with the final message, and voice answers and approvals go back into the agent [67].
  - Claude voice runs tools during voice, with push-to-talk [73].
  - Wispr sends search output to a browser rather than an overlay [66].
  - FluidVoice exposes a local API for meetings and transcripts (beta.8) [5]. Superwhisper has a CLI and MCP server over history [78].
- *What users say:*
  - The output surface must not hide OS dialogs or look frozen (#444, #446) [48][49].
  - Users want external tools (MCP) inside the voice agent (#275) [14].
  - A visible state from press to delivered matters (#676) [46].
  - Reddit and HN evidence on voice-agent UIs specifically: not found.

## 8. Sources (numbered URL list)

1. https://github.com/altic-dev/FluidVoice (repo metadata via API: https://api.github.com/repos/altic-dev/FluidVoice)
2. https://www.altic.dev/fluid
3. https://github.com/altic-dev/FluidVoice/issues?q=is%3Aissue+is%3Aopen+sort%3Areactions-%2B1-desc
4. https://github.com/altic-dev/FluidVoice/releases
5. https://github.com/altic-dev/FluidVoice/releases/tag/v1.6.10-beta.8
6. https://github.com/altic-dev/FluidVoice/releases/tag/v1.6.10-beta.7
7. https://github.com/altic-dev/FluidVoice/releases/tag/v1.6.3
8. https://github.com/altic-dev/FluidVoice/releases/tag/v1.6.2
9. https://github.com/altic-dev/FluidVoice/releases/tag/v1.5.14
10. https://news.ycombinator.com/item?id=48739409
11. https://hn.algolia.com/api/v1/search?query=FluidVoice&tags=comment (items 49812281, 49809811, 49619313, 48824163, 48714104, 50013078, 49421577: https://news.ycombinator.com/item?id=<id>)
12. https://github.com/altic-dev/FluidVoice/issues/925
13. https://github.com/altic-dev/FluidVoice/issues/29
14. https://github.com/altic-dev/FluidVoice/issues/275
15. https://github.com/altic-dev/FluidVoice/issues/92
16. https://github.com/altic-dev/FluidVoice/issues/272
17. https://github.com/altic-dev/FluidVoice/issues/644
18. https://github.com/altic-dev/FluidVoice/issues/682
19. https://github.com/altic-dev/FluidVoice/issues/665
20. https://github.com/altic-dev/FluidVoice/issues/709
21. https://github.com/altic-dev/FluidVoice/issues/788
22. https://github.com/altic-dev/FluidVoice/issues/694
23. https://github.com/altic-dev/FluidVoice/issues/846
24. https://github.com/altic-dev/FluidVoice/issues/62
25. https://github.com/altic-dev/FluidVoice/issues/159
26. https://github.com/altic-dev/FluidVoice/issues/479
27. https://github.com/altic-dev/FluidVoice/issues/1013
28. https://github.com/altic-dev/FluidVoice/issues/469
29. https://github.com/altic-dev/FluidVoice/issues/213
30. https://github.com/altic-dev/FluidVoice/issues/555
31. https://github.com/altic-dev/FluidVoice/issues/43
32. https://github.com/altic-dev/FluidVoice/issues/929
33. https://github.com/altic-dev/FluidVoice/issues/481
34. https://github.com/altic-dev/FluidVoice/issues/627
35. https://github.com/altic-dev/FluidVoice/issues/388
36. https://github.com/altic-dev/FluidVoice/issues/167
37. https://github.com/altic-dev/FluidVoice/issues/457
38. https://github.com/altic-dev/FluidVoice/issues/852
39. https://github.com/altic-dev/FluidVoice/issues/751
40. https://github.com/altic-dev/FluidVoice/issues/337
41. https://github.com/altic-dev/FluidVoice/issues/373
42. https://github.com/altic-dev/FluidVoice/issues/498
43. https://github.com/altic-dev/FluidVoice/issues/422
44. https://github.com/altic-dev/FluidVoice/issues/1088
45. https://github.com/altic-dev/FluidVoice/issues/948
46. https://github.com/altic-dev/FluidVoice/issues/676
47. https://github.com/altic-dev/FluidVoice/issues/377
48. https://github.com/altic-dev/FluidVoice/issues/444
49. https://github.com/altic-dev/FluidVoice/issues/446
50. https://github.com/altic-dev/FluidVoice/issues/308
51. https://github.com/altic-dev/FluidVoice/issues/35
52. https://github.com/altic-dev/FluidVoice/issues/165
53. https://github.com/altic-dev/FluidVoice/issues/259
54. https://github.com/altic-dev/FluidVoice/issues/331
55. https://github.com/altic-dev/FluidVoice/issues/405
56. https://github.com/altic-dev/FluidVoice/issues/616
57. https://github.com/altic-dev/FluidVoice/issues/506
58. https://github.com/altic-dev/FluidVoice/issues/100
59. https://github.com/altic-dev/FluidVoice/issues/979
60. https://github.com/altic-dev/FluidVoice/issues/1023
61. https://github.com/altic-dev/FluidVoice/issues/463
62. https://github.com/altic-dev/FluidVoice/issues/518
63. https://github.com/altic-dev/FluidVoice/issues/406
64. https://github.com/altic-dev/FluidVoice/issues/18
65. https://github.com/altic-dev/FluidVoice/releases/tag/v1.6.6
66. https://docs.wisprflow.ai/articles/4816967992-how-to-use-command-mode
67. https://superwhisper.com/docs/get-started/coding-agents.md
68. https://superwhisper.com/docs/modes/modes
69. https://tryvoiceink.com/
70. https://www.typeless.com/
71. https://aquavoice.com/pricing
72. https://talon.wiki/Basic%20Usage/basic_usage/
73. https://support.claude.com/en/articles/11101966-using-voice-mode-on-claude-mobile-apps
74. https://help.openai.com/en/articles/9617425-voice-mode-faq (403 Cloudflare) · https://www.raycast.com/core-features/ai (no voice mention found)
75. https://wisprflow.ai/pricing
76. https://wisprflow.ai/features
77. https://superwhisper.com/docs/billing/plans.md
78. https://superwhisper.com/docs/llms.txt
79. https://github.com/Beingpax/VoiceInk
80. https://raw.githubusercontent.com/Beingpax/VoiceInk/main/README.md
81. https://whipscribe.com/tools/macwhisper
82. https://willowvoice.com/pricing
83. https://help.willowvoice.com/en/articles/12854184-willow-pricing-plans-overview
84. https://www.typeless.com/pricing
85. https://www.monologue.to/
86. https://raw.githubusercontent.com/cjpais/Handy/main/README.md
87. https://raw.githubusercontent.com/EpicenterHQ/epicenter/main/README.md
88. https://developer.apple.com/documentation/speech/speechanalyzer
89. https://dictaflow.io/blog/windows-fluid-dictation-copilot-plus-2026.html
90. https://www.elevenforum.com/t/enable-or-disable-fluid-dictation-in-voice-typing-on-windows-11.42286/
91. https://huggingface.co/nvidia/parakeet_realtime_eou_120m-v1
92. https://huggingface.co/nvidia/nemotron-speech-streaming-en-0.6b
93. https://github.com/altic-dev/FluidVoice/releases (tags windows-v0.0.7, windows-v0.0.8)
94. https://github.com/altic-dev/FluidVoice/releases/tag/v1.6.1