# 153 — VOICE-ANYWHERE: shaped dictation for every surface and any app, a command mode at the key, one answer card

*Status: design v0, 2026-10-09 (+01:00), written as the build contract for waves A–D.
Tree: worktree ~/Develop/theuth-153 = origin/main 6e395770 (study landed). Every path:line below was read in that worktree.
Abbreviations: COMP/ = crates/desktop/companion/Sources/, WIN/ = crates/desktop/companion-windows/src/, UI/ = crates/desktop/ui/src/,
VOICE/ = crates/voice/src/, api.rs = crates/rpc/src/api.rs. Inputs: Sky's rulings (knowledge decisions/theuth-fluidvoice-study-rulings-2026-10-09),
the study docs/studies/2026-10-09-fluidvoice-dictation-command-ask.md and its appendices, designs 54/96/130/134/145/149.
Also read: the datasets sheet `docs/design/153-datasets.md` (landed on this branch, 6b314a4b) and the mock `docs/design/assets/153-voice-anywhere.html`
(frames A–D; its ⌃⌥D / ⌃⌥C chords were placeholders — §15 fixes them). FluidVoice is cited only through the study's appendix A
(`docs/studies/2026-10-09-fluidvoice-dictation-command-ask/A-fluidvoice-source-recon.md`, below "A:L…"); techniques only, GPL-3, no code.*

## 0. Decisions in one screen

Sky's rulings R1–R6 and the orchestrator's calls D1–D18 are fixed (`/Users/Sky.Moore/Develop/theuth-153-briefs/153-RULINGS.md`); one line each,
then the writer's calls W-a… with the section that holds each.

| # | Decision (fixed) |
|---|---|
| D1 | Any-app dictation lives in the companion (macOS AX + CGEvent; Windows UIA + SendInput), fed by the engine's dictation stream; three-way target assessment → clipboard lease → paste verification → "paste not landed" card; Rewrite mode on the AX selection; direct AX insertion where the field allows; secure fields never get Return — §13 |
| D2 | `voice::shape` is a pure Rust layer in `crates/voice`, applied by the dictation actor to every surface: context-gated spoken punctuation → literal formatter → dictionary replacements (self-expansion guard) → end-command detection; per-surface config; unit tests with an examples table — §4 |
| D3 | Dictionary = packaged dataset (`153-datasets.md`) + user entries + learned corrections; matcher Double Metaphone + edit distance, known 0.70 / new 0.85, negatives veto; no seeding from the computer or the project — §5 |
| D4 | Correction learning in our own fields (dictated span vs sent text), single-word non-inflection edits, suggest after 2 in 7 days, 7-day dismiss, ≤ 3 ignores/session, PCM kept 120 s; any-app AX observation later — §6 |
| D5 | Spoken end commands on partials: "send it" (armed, sticky, near-miss len/5, settle 1.5 s + 0.35 s silence), "scratch that", "new line"/"new paragraph"; configurable; never Return into a secure field; Composer "send it" sends — §7 |
| D6 | Capture robustness: device priority + Settings, re-arm on device change, first-PCM gate 2 s + silent-device detection, 8 s standby + prewarm, mic at press, media pause/duck opt-in — §8 |
| D7 | Latency instrumentation press → first partial → final per surface in `dictation_status` + a Settings › Voice debug row; no model change — §8 |
| D8 | Polish opt-in per destination, human-destined only, never before agent prompts; `voice.polish_model` + picker; chain polish → title → session; transcript as JSON data; smallest edits; fail closed; undo from history — §9 |
| D9 | History: last N dictations (raw/shaped/polished, destination, timings, audio under a budget), local only, in the engine — §6 |
| D10 | Command mode: own key, mode switch while holding, the same desk with a command prompt profile (act → verify → one ✓/✗ line), step-typed card, turn cap, follow-ups, approvals in the card answerable by voice or keys — §10 |
| D11 | Native-app tools in `crates/computer`: `applescript`, `shortcuts` (macOS), `powershell` (Windows), mutating, under the policy; a recipe skill — §11 |
| D12 | One AnswerCard for Ask, voice key and command: streamed markdown, notch ↔ panel, follow-up in place, session list; the Ask receipt's two lines = collapsed form — §12 |
| D13 | State honesty: listening → finishing → thinking → answer → speaking → failed; stop freezes; hide is generation-checked; every state has copy — §12 |
| D14 | Pointer-screen presentation; "Automatic" hotkey option (tap < 0.4 s toggles, hold = push-to-talk) beside hold/double-tap-lock; modifier-only keys incl. fn by physical keycode on macOS — §12, §15 |
| D15 | A Windows twin for every companion/UI change (R6), named per section — §10–§13, §15 |
| D16 | Waves A engine → B surfaces → C companion → D any-app; contract-first (§14 freezes signatures), §16 ownership/proof/must-not-regress — §16 |
| D17 | Techniques only; FluidVoice cited through appendix A; no code — everywhere |
| D18 | Rejected: project/computer seeding, polish before agent prompts, a one-tool command loop, WebRTC APM, CTC boosting, a closed local LLM, a new STT model path — §17 |

Writer's calls (each marked "(writer's call)" where it is made):

| # | Call | Holds it |
|---|---|---|
| W-a | Shaping runs on every piece, partial and final; a partial's shaped text may differ from the final's (clients already replace the whole utterance per event) — the end-command parser is the only stage that must see partials | §4 |
| W-b | `DictationText` gains `shaped: String` and `command: Option<EndCommand>` (serde default, additive); `raw` stays in `text` | §4, §14 |
| W-c | `dictation_start` gains one trailing optional positional `opts: Option<DictationOpts { surface, destination, app }>`; `None` = today's behaviour (`Surface::Composer`, `Destination::AgentPrompt`) | §4, §14 |
| W-d | Chords: macOS ⌃⌥D dictate-anywhere, ⌃⌥C command; Windows Ctrl+Alt+D / Ctrl+Alt+C with an AltGr probe at registration that falls back to Ctrl+Shift+Alt+D / +C on layouts where AltGr+D / +C type a glyph | §15 |
| W-e | Command turn cap = 12 tool rounds per request (then the card says "stopped after 12 steps — hold to continue") | §10 |
| W-f | History: 200 entries, audio 64 MiB, raw i16 16 kHz under `<data_dir>/voice/history/`, metadata in a new `voice.db` (sqlx, migrations like `crates/artifact`) | §6 |
| W-g | D4's any-app AX observation window is designed now (§6.6) as an optional later wave, not built in D | §6 |
| W-h | Polish is an `Agents` side call (`CallKind::Polish`) charged to theuth's desk session when the destination has no session of its own | §9 |
| W-i | "Automatic" is opt-in; the default gesture stays hold ≥ 200 ms / double-tap lock | §12, §15 |
| W-j | "new line"/"new paragraph" are shaping actions (§4), not end commands; end commands are "send it", "scratch that" and their configured synonyms | §4, §7 |
| W-k | Matcher constants renamed `ACCEPT_KNOWN = 0.70`, `ACCEPT_NEW = 0.85`, `EDIT_MAX = 2` | §5 |
| W-l | The command profile is a user-line marker `[command, N s] …` plus a prompt block registered by the `computer` plugin — no new session, no per-turn system-prompt RPC | §10 |
| W-m | Any-app insertion: direct `kAXSelectedText` write only on an element assessed `editable` whose value read-back confirms within 150 ms; everything else (unknown, GPU terminals, web areas) takes the clipboard lease | §13 |
| W-n | Settings homes: desktop Settings › Voice (new tab: input device, polish model, dictionary, history, debug row); companion Settings › Dictation (new tab: any-app, keys live in Keys) | §8, §9, §13 |
| W-o | The learned-correction suggestion card is one shared component per client (`DictionarySuggestion`), shown in the surface that produced the correction | §6 |

## 1. What Sky asked for

### 1.1 The ask (the study's question, verbatim — `docs/studies/2026-10-09-fluidvoice-dictation-command-ask.md:4`)
> "do a study of dictation and FluidVoice and determine what we can learn from it and how we can incorporate those things into theuth. Specifically: better dictation (ours is already pretty good), the command mode, and a different type of dictation mode from the Ctrl+S draw/say/ask voice mode — where you can ask the agent anything with your voice anytime and actually see the output; we kind of already have that in the companion app — how can we make this better with lessons from FluidVoice?"

### 1.2 The five answers (2026-10-09 16:42; `153-RULINGS.md:9-13`, verbatim where typed)
- **R1 Dictate into any app** — picked "Yes — any app, replace FluidVoice/Wispr".
- **R2 Command mode entry** — picked "Third voice-key MODE with its own key" (dictate · ask · command as separate bindings; another mode's key while holding switches mode).
- **R3 LLM polish** — "Yes, often human destined text only but we need a setting to be able to choose what model is used for that".
- **R4 Dictionary seeding** — "Look, this isn't a coding agent, it drives harnesses that are coding agents I don't know at what rate or from where we would decide to incorporate symbols into a dictionary or something like that That could be valuable But I think probably it may be better or a nice addition on top of that to find a data set to pre-populate and package as a dictionary for technical things and commands and stuff like that But as far as like populating from stuff on the user's computer or symbols from a project or whatever it might be, we got a really think about the algorithm behind that and how and when it happens".
- **R5 Latency spike** — "I'm not sure our first partials are pretty fast already".

### 1.3 The standing Windows-twin rule (15:50; `153-RULINGS.md:14`)
> "make sure that the windows agent builds the same fixes"

Every companion or UI change in this design names its Windows equivalent in the section that makes it (D15); §15 carries both platforms' key table; §16 gives each wave a Windows twin owner.
