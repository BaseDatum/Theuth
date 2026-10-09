# Study: FluidVoice vs theuth — dictation, command mode, and asking the agent anywhere by voice

- **Date:** 2026-10-09 (recon 16:14–16:45, Sky's M3 Max at load 7–15; three read-only children on shared axes)
- **Question (Sky):** "do a study of dictation and FluidVoice and determine what we can learn from it and how we can incorporate those things into theuth. Specifically: better dictation (ours is already pretty good), the command mode, and a different type of dictation mode from the Ctrl+S draw/say/ask voice mode — where you can ask the agent anything with your voice anytime and actually see the output; we kind of already have that in the companion app — how can we make this better with lessons from FluidVoice?"
- **Status:** answered — fifteen borrows ranked and grouped (§6), four waves proposed (§9), five decisions needed from Sky (§10). This file is the living record.
- **Licence:** FluidVoice is GPL-3 (Apache-2 before 2026-02-23). Everything here is **techniques only**; no code is copied. Clone (read-only, not in git): `~/Develop/spike-fluidvoice/FluidVoice` at HEAD `7daffb52` (2026-10-06).
- **Evidence:** appendices under `docs/studies/2026-10-09-fluidvoice-dictation-command-ask/` — A their source recon (288 `path#L` cites, 48 re-opened by its author, 18 by the orchestrator: all held), B our inventory on main `4bd11af3` (222 cites; 20 + 14 re-opened: all held), C the web survey (94 sources), AXES. Knowledge: `facts/fluidvoice-dictation-command-rewrite-recon-2026-10-09`, `facts/theuth-voice-dictation-inventory-vs-fluidvoice-axes-2026-10-09`, `facts/theuth-borrow-list-from-fluidvoice-2026-10-09`.

## 1. Verdict

**The model layer is a draw; the lead is everything around it.** FluidVoice and theuth both run NVIDIA Nemotron/Parakeet on-device (theirs through CoreML/FluidAudio at 160–320 ms chunks, ours Nemotron 0.6B int8 through ONNX Runtime at 560 ms windows). What makes FluidVoice feel better is the *pipeline around the model* — a deterministic shaping pass (spoken punctuation, `/commands`, `@mentions`, paths), a dictionary that learns from the user's own corrections grounded in the audio, a spoken "send it", hotkey hygiene, a hardened capture path, and the clipboard-lease / verify / target-assessment stack that puts text into other apps. theuth has **none of the shaping layer** (text leaves the model and lands in the field unchanged, `crates/voice/src/live.rs#L218-L233`), **no correction loop, no history, no device choice, no latency number anywhere**, and dictates only into its own inputs.

**Q1 — better dictation:** build a pure, tested `voice::shape` layer in the engine (every surface inherits it: Composer, TUI, companion, phone), seed its dictionary from the project itself (code index symbols, agent names, knowledge titles — a twist FluidVoice cannot make), learn corrections from the fields we already own (no accessibility tree needed), add spoken end commands ("send it", "scratch that", "new line"), harden capture (device change re-arms instead of ending; mic at 0 ms; standby), and *measure* the first-word latency before choosing a faster first-partials path. Put an LLM "polish" step only in front of **human-destined** text — never before an agent prompt; the agent reads raw text fine and FluidVoice's users pay 5 s+ for that step.

**Q2 — command mode:** FluidVoice's command mode is a *single tool* (`execute_terminal_command` in zsh, 30 s, ≤ 20 turns) with osascript one-liners taught in the system prompt and a substring deny-list for confirmation. theuth's desk already out-tools it (eight `computer_*` tools, shell, browser, MCP, risk policy). Borrow the **wrapper**: a third voice-key *mode* chosen at the key (dictate · ask · command; another mode's key while holding switches mode), a compact step-typed card (checking / executing / verifying with the tool's purpose) ending in one ✓/✗ line, approvals shown *in* the card and answerable by voice, follow-ups in the same card — plus the one tool we lack and they profit from: native-app control through AppleScript/Shortcuts.

**Q3 — ask anywhere and see the output:** the companion already has the hard parts (hold-to-talk into the desk, the AnswerCard with streamed tool names, screen context, opt-in spoken replies — landed on main `49fa43f8` while this study was written). What FluidVoice does better is the *answer surface*: one persistent card for Ask and voice key alike (ours shows two receipt lines for Ask, `AskRouter.swift#L108-L118`), expandable from the notch into a panel and back, markdown, follow-up in place, a session list, presented on the pointer's screen, and an overlay that never disappears before the work is done (their users' #676 is the complaint to design against).

## 2. Method

| child | kind | axis | output |
|---|---|---|---|
| fv-theirs | theuth subagent, read-only on the clone | A1–A8 of AXES.md over FluidVoice HEAD 7daffb52 (375 Swift files / 168k lines) | Appendix A (286 lines, 288 cites) |
| fv-ours | theuth subagent, read-only on main 4bd11af3 (code_* index) | the same axes over `crates/voice`, `talk`, `computer`, `desktop`, the two companions, the TUI, designs 43/45/54/70/76/89/94/96/117/130/134/145/149 | Appendix B (390 lines, 222 cites) |
| fv-web | `tools: read_only` web survey | GitHub API/issues by reactions, HN Algolia, vendor pages, the field (Wispr Flow, Superwhisper, VoiceInk, MacWhisper, Aqua, Willow, Typeless, Monologue, Handy, Talon, Apple SpeechAnalyzer, Windows Fluid dictation), EOU models | Appendix C (314 lines, 94 sources) |

All three delivered in 6–10 minutes; the orchestrator re-opened 32 load-bearing cites across both trees (29 on keyword, 3 confirmed by reading). No builds, no runs; the machine was shared with the 149/151 waves.

## 3. FluidVoice in one screen (what it does — cites in Appendix A, `Sources/Fluid/`)

1. **Shaping order:** raw ASR → spoken-punctuation pass (phrase→symbol table with a spacing class per symbol; "dot"/"slash"/operators convert only in symbol context; the model's own `,`/`.` next to a spoken symbol are removed) → literal formatter for `/commands`, `@mentions`, terminal-autocomplete spacing → dictionary replacements → optional AI cleanup → delivery. The AI gate is frozen once at stop into a snapshot so nothing changes mid-flight (`ASRService+SpokenPunctuationFormatting.swift#L214-L290`, `#L635-L874`, `ASRService+DictationLiteralFormatting.swift#L236-L262`, `ContentView.swift#L2727-L2743`).
2. **AI cleanup is per app and per shortcut slot** — bundle id → prompt profile *and* whether AI runs at all; the transcript travels as JSON data in the user message, instructions in the system prompt; default prompt "smallest edits"; fails closed when the local model is down (`DictationAIPostProcessingGate.swift#L22-L36`, `DictationPromptRequest.swift#L3-L26`).
3. **Automatic dictionary learning:** watch the target field 30 s after insertion; single-word, non-inflection edits inside the inserted range; suggest after 2 occurrences in 7 days, 7-day dismiss cooldown, ≤ 3 ignores per session; PCM kept 120 s so the edit is matched to the spoken word (thresholds 0.70 known / 0.85 new; negatives veto, never accept); rules cannot expand their own output (Git→GitHub never → GitHubHub) (`AutomaticDictionaryCorrectionTracker.swift#L5-L10`, `DictionaryCorrectionEditPolicy.swift#L6-L40`, `DictionaryLearningEvidence.swift#L13-L75`, `DictionaryReplacementProtection.swift#L3-L6`); Parakeet CTC keyword boosting ≤ 256 terms (`ParakeetVocabularyStore.swift#L79-L84`).
4. **Spoken "send it":** armed by live partials, sticky across re-decodes, near-miss tolerance len/5, auto-stop after 1.5 s settle + 0.35 s silence, never dispatches Return into a secure field or after focus moved; the overlay shows detected → counting down → sent (`SpokenSendParser.swift#L37-L99`, `TypingService.swift#L47-L60`).
5. **Delivery into other apps:** three-way target assessment (editable / certainly not / unknown — refuse only when certain); clipboard *lease* (snapshot every pasteboard item, write with Transient/Concealed markers + a session id, ⌘V, restore at 500 ms only if the clipboard still holds our text); paste *verification* by AX value/count/caret at 150/350/1000 ms → "paste not landed" card carrying the transcript (`DeliveryTargetAssessment.swift#L25-L70`, `PasteDeliveryCoordinator.swift#L644-L722`, `PasteVerifier.swift#L72-L121`).
6. **Rewrite mode:** AX selection → the spoken instruction is the user message, the selection goes into the system prompt through an editable template, profile per mode and per app, multi-turn refine, focus restored before the paste (`RewriteModeService.swift#L50-L77`, `#L156-L240`).
7. **Command mode = one tool** (`CommandModeService.swift#L18`, `#L697-L737`, `#L827-L925`, `TerminalService.swift#L70-L105`) — see §1 Q2. Steps mirrored into the notch, expandable to a window; sessions persisted; model picker per mode.
8. **Hotkeys:** Toggle / Hold / Automatic (tap < 0.4 s toggles, hold = push-to-talk; no double-tap lock); modifier-only keys incl. fn by physical keycode, left/right distinct; another key during the hold cancels; repeats never toggle; another mode's key while recording *switches* mode; overlay on the screen under the pointer; Esc via local + global monitors (`GlobalHotkeyManager.swift#L313`, `#L1593-L1600`, `#L1682-L1688`, `#L1806-L1808`, `OverlayScreenResolver.swift#L9-L12`, `NotchOverlayManager.swift#L131-L150`).
9. **Capture:** custom Core Audio IOProc → C ring, drained off the realtime thread; first-PCM readiness 2 s; silent-device detection; mic priority list with clamshell skip; device changes collapse into one re-setup; 8 s engine standby + idle prewarm; media pause reconciler. WebRTC APM is vendored but used **only** for meeting echo cancellation (`AudioCaptureReadinessGate.swift#L29-L44`, `ASRService.swift#L2082-L2086`, `Meeting/AEC/MeetingAECModels.swift#L5-L50`).
10. **History and API:** raw-vs-enhanced diff + "undo AI", WAV per dictation with a budget, analytics for `no_editable_target` / `paste_not_landed`; a loopback-only local HTTP API (127.0.0.1:47733, no token) for transcribe / postprocess / history / meetings / dictionary (`LocalAPI/LocalAPIRouter.swift#L12-L29`).

## 4. theuth on the same axes (Appendix B; main `4bd11af3`)

| axis | FluidVoice | theuth today | gap |
|---|---|---|---|
| A1 capture | IOProc ring, readiness gate, device priority, re-arm on change, standby/prewarm, media pause | default input only, `Aec::None`, device loss ends the dictation, companion waits 250 ms before opening the mic (`mic.rs#L54-L61`, `dictation.rs#L249`, `HoldToTalk.swift#L128-L145`) | robustness + first-audio latency |
| A2 STT | Parakeet EOU 160 ms, Nemotron 320 ms CoreML, boosting ≤ 256 terms, manifest catalog | Nemotron 0.6B int8, 560 ms windows, VAD 160/300 ms, `Stream::feed(window)` only (`stt.rs#L32-L34`, `#L237-L266`, `vad.rs#L106-L118`) | no vocabulary path; no measured latency |
| A3 shaping | punctuation/literal/dictionary/AI, per app | none beyond a punctuation trim (`live.rs#L218-L233`) | the whole layer |
| A4 delivery | any app: lease, verify, assess, failure card, rewrite | own inputs only; selection replaced by dictated words (`dictation.ts#L211-L215`); `computer_type` is the agent's, not the user's | strategic fork (§10 Q1) |
| A5 command | one-tool loop, step types, confirm list, sessions | desk turn with screen context; 8 `computer_*` + shell + browser + MCP; AnswerCard; no AppleScript/Shortcuts (`PointController.swift#L272-L308`, `tools.rs#L143-L385`) | mode identity, step card, native-app tool |
| A6 overlay/keys | 3 hotkey modes, fn, chord cancel, mode switch, pointer screen, state-rich notch | hold 200 / double-tap lock / Esc per gesture, tested in Swift + Rust (`PointMachine.swift#L12-L22`); island hosts voice view; 149 island-click fix in flight, spoken replies landed `49fa43f8` | pointer-screen presentation, mode switch, Automatic option |
| A7 history | diff, undo AI, audio budget, stats | nothing (`dictation.ts#L245-L251`) | the loop |
| A8 quality | 1,341 commits, weekly releases, evals under `tools/` | tests for gesture/probes; no dictation eval, no latency record | an eval + a number |

## 5. What users say (Appendix C)

Praise: "non-cloud Wispr Flow", raw dictation "appears in the same second" once the local LLM is removed, free and open, command mode "great for built-in Mac actions". Complaints by weight: **audio-device changes crash or hang** (#644 #682 #709 #788 #694), **text not landing / clipboard clobbered** (#479 #1013 #213 #929), Fluid Intelligence 5 s+ (#925), the cleanup model answering the transcript instead of cleaning it (#388 #167 #457), hotkey startup delay dropping words (#751), overlay hiding before processing ends (#676), high idle CPU (#948), command mode: confirm everything / MCP / custom prompt / frozen look / OS dialogs hidden behind the overlay (#377 #275 #308 #444 #446). The field: Wispr's dictionary learns from spelling corrections; Superwhisper pops up at a coding agent's questions and lets you approve tool calls by voice; Handy offers hold-or-toggle on one key; Apple and Windows ship built-in cleanup (Copilot+ only, off by default for new users).

## 6. Borrows, ranked and grouped by where they land (techniques only)

**Q1 — engine (`crates/voice`), every surface inherits**
1. **`voice::shape`** — pure Rust, unit-tested: context-gated spoken punctuation, "new line"/"new paragraph", literal `/commands`, `@mentions`, paths, flags, identifiers (terminal-autocomplete spacing), strip the model's own punctuation beside a spoken symbol, dictionary replacements with the self-expansion guard. Off/on per surface in config; the TUI and the phone get it for free.
2. **Project-seeded dictionary** — canonical terms from the code index (symbols, file names), agent names, project and knowledge titles → fuzzy/phonetic match of misheard variants → replacement, per project. Our substitute for CTC boosting, which parakeet-rs does not expose.
3. **Correction learning without AX** — diff the dictated span against what the user sends; single-word non-inflection edits; suggest after 2 repeats in 7 days (cooldowns as theirs); keep the utterance PCM ~120 s so a suggestion is grounded in the spoken word.
4. **Spoken end commands on partials** — "send it" (armed by partials, settle 1.5 s + 0.35 s silence, never in secure fields), "scratch that", "new line".
5. **Capture robustness** — input-device priority + clamshell skip, device change re-arms, first-PCM gate + silent-device detection, 8 s standby + prewarm, mic at 0 ms, media pause/duck.
6. **Latency floor, measured** — record press → first partial on every surface; then a faster first-partials path if the number says so. Caveat from the 2026-09-29 tiers recon: Parakeet realtime EOU 120M is WER 9.3 % against 6.94 % for our Nemotron 0.6B, so "faster" means a two-tier scheme (fast partials, accurate finals) or Nemotron's 320 ms export — not a model swap (parakeet-rs Burn/Metal bench 2026-10-08 is the GPU option).
7. **Opt-in polish for human-destined text only** — notes, documents, Slack/Telegram/Signal replies, voice notes; per-destination prompt profile; transcript as data, instructions in the system prompt; fail closed; undo from history. Never before agent prompts.
8. **Dictation history** — last N dictations with raw-vs-shaped diff and audio under a budget, local only; feeds 3 and 7.

**Q2 — companion + `crates/computer`**
9. **A command MODE at the key** — dictate · ask · command as separate bindings; another mode's key while holding switches mode; command = the desk with a command prompt profile (act → verify → one ✓/✗ line), a compact step-typed card, a turn cap, follow-ups in the card.
10. **Approvals in the card** — stream steps live (no frozen look), show the pending approval on the card, accept a spoken "yes/go"; our risk policy replaces their substring list.
11. **`applescript` / `shortcuts` tool** (mutating) + a recipe skill for Reminders, Notes, Calendar, Messages, Mail, Music; Windows twin via PowerShell/UIA.

**Q3 — companion answer surface**
12. **One persistent AnswerCard for Ask and voice key** — full streamed markdown, expand notch → panel and back, follow-up in place, a session list.
13. **State honesty** — listening → finishing → thinking (shimmer + tool names) → answer → speaking (glyph) → failed; stop freezes, hide is generation-checked, nothing vanishes early.
14. **Pointer-screen presentation** and an **Automatic** hotkey option (tap toggles, hold = push-to-talk) beside hold/double-tap-lock; fn and other modifier-only keys by physical keycode.

**Strategic**
15. **Dictate into ANY app from the companion** — clipboard lease, paste verification, three-way target assessment, failure card with the transcript, Rewrite mode on the AX selection. Only with Sky's ruling (§10 Q1): it is where FluidVoice's users suffer most, and these are the hard-won answers.

## 7. Do not borrow

The closed local LLM as the centre of the product (theuth has models and the agent); the one-tool terminal agent and prompt-taught osascript (we have tools and policy — give the agent a real tool instead); UserDefaults-JSON settings; their meeting stack (design 53); WebRTC APM (unused for dictation even there; Apple's VPIO AEC already serves talk/meetings); any LLM cleanup in front of agent-bound prompts.

## 8. Their weaknesses to design against

Device-change crashes (a custom Core Audio path is powerful and fragile); paste failures and clipboard clobbering (the lease and verification exist *because* of them); a 5 s local LLM step that destroyed the "instant" feel; the cleanup model answering the transcript; hotkey startup delay; an overlay that hides before processing ends; idle CPU; a local API with no token; no double-tap lock and no eval of dictation accuracy in the repo (the `tools/` evals are for meetings and the dictionary).

## 9. Sequencing proposed

| wave | scope | lands where | needs |
|---|---|---|---|
| A — engine | borrows 1, 2, 4, 5, 8: `voice::shape`, dictionary + project seeding, spoken end commands on partials, capture robustness, history store | `crates/voice`, `crates/voice/src/dictation.rs`, RPC events (`DictationText` gains `shaped`/`command` fields) | pure Rust, tests, no UI; one omp worker |
| B — surfaces | borrows 3, 7, 8 UI: correction suggestions in Composer/TUI/companion, opt-in polish for human-destined text, history panel | `crates/desktop/ui`, `crates/tui`, both companions | after A's PIN |
| C — companion | borrows 9–14: command mode card, approvals in card + voice approval, applescript/shortcuts tool, one AnswerCard with follow-up, state honesty, pointer screen, Automatic hotkey option | `crates/desktop/companion*`, `crates/computer`, desk prompt profile | after the in-flight 149 island fix lands; Windows twin per Sky's standing rule |
| D — strategic | borrow 15 (system-wide dictation) and 6 (faster first partials) | companion + `crates/voice` | Sky's ruling; a measured benchmark first |

## 10. Open questions for Sky

1. **System-wide dictation into any app** — should theuth become the dictation tool (replacing FluidVoice/Wispr), or stay inside its own inputs? Biggest fork; the delivery stack is the hard part and the one their users complain about most.
2. **Command mode entry** — a third voice-key mode with its own key (recommended: identity chosen at the key, mode switch while holding) vs intent detection inside the existing voice key.
3. **LLM polish** — opt-in and only for human-destined text (notes, messages, channel replies), never before agent prompts: agree?
4. **Dictionary seeding from the project** (code index, agents, knowledge) — on by default per project, or opt-in?
5. **Latency spike** — approve a measured benchmark of press → first partial on every surface and of a 320 ms / EOU first-partials path before any latency promise?

## 11. Appendices

- `A-fluidvoice-source-recon.md` — fv-theirs report (A1–A8, map to the three questions, 48-cite self-check)
- `B-theuth-voice-inventory.md` — fv-ours report (A1–A8 with exists / missing / designs, Q1–Q3, 20-cite self-check)
- `C-web-survey.md` — fv-web report (traction, praise, complaints with issue URLs, the field table, techniques, 94 sources)
- `AXES.md` — the shared axes the three children followed
