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

## 2. Where we stand

Facts relied on, re-read in this worktree. The study's appendix B inventoried main `4bd11af3`; main moved to `6e395770` with only docs, a
Windows-companion wave and the spoken-replies fix in between, so every cite below was re-opened here rather than carried.

### 2.1 What exists (the pipeline every surface shares)
- **One engine-owned streaming STT.** `Dictation` actor (`VOICE/dictation.rs:161-176`) holds one `Utterer` (`VOICE/live.rs:34-58`): Nemotron
  streaming session + TEN-VAD + gate + resampler; 560 ms windows (`VOICE/stt.rs:32-34`, `WINDOW = 8_960`); gate constants pre-roll 20 frames,
  onset 10 (160 ms), close 19 (≈300 ms), max utterance 15 s, turn 2 s, threshold 0.5 (`VOICE/vad.rs:107-117`). The task reads 20 ms chunks,
  reports RMS every 8 chunks and feeds the utterer (`VOICE/dictation.rs:558-623`); a deadline of `dictation_max_secs` (300) stops it
  (`:566-574`, `VOICE/lib.rs:443`, `:459`).
- **Two audio sources, one slot.** `AudioSource::Mic` opens the daemon's default input at 48 kHz with `Aec::None` (`VOICE/dictation.rs:56-68`,
  `:241-251`, `VOICE/mic.rs:22`, `:59-61`); `Remote { sample_rate }` takes mono i16 pushed by `dictation_audio`, rechunked to 20 ms
  (`:413-435`). One dictation per daemon (`"already dictating"`, `:232-237`); a talk leases the engine (`"talking"`, `:235-237`, `:311-325`).
  Device loss ends the dictation: the mic thread sends `Err("device lost")` once and returns (`VOICE/mic.rs:160-170`).
- **Events.** `DictationModel`, `DictationStarted`, `DictationLevel` (8 RMS values), `DictationText { id, utterance, text, final }` (append-only
  per utterance, whole text so far), `DictationFinished { reason }` (`VOICE/lib.rs:383-421`); `FinishReason` = Done | Stopped | Error
  (`:320-334`). The only shaping today: `piece()` trims a stray leading `.,?!;:` and whitespace (`VOICE/live.rs:218-233`); `append` keeps the
  model's SentencePiece spacing verbatim (`:235-244`).
- **RPC.** `dictation_prewarm` / `dictation_start(sample_rate: Option<u32>)` / `dictation_audio(id, pcm)` / `dictation_stop(id)` /
  `dictation_status()` (`api.rs:1575-1602`); a relay peer may not open the daemon mic (`:6279-6283`); a remote dictation stops when its
  connection closes (`:6289-6295`). `speak(session, text)` / `stop_speaking(id)` (`:1557-1561`). `resolve_approval(id, decision)`
  (`:640-641`) with `Decision = Allow | Deny` (`crates/tools/src/approval.rs:32-37`).
- **Config.** `voice:` section = `enabled`, `voice`, `model_dir`, `stt_model_dir`, `stt_idle_secs`, `dictation_model_dir`,
  `dictation_max_secs`, `player_buffer_ms`, `player_device` — no input device, nothing about shaping (`VOICE/lib.rs:424-464`). The plugin
  spawns `Transcriber`, `Dictation`, `Voice` and registers a prompt block (`:967-1021`); `runtime::write_nested_config_value(path, section,
  subsection, key, value)` writes `section.subsection.key` preserving the YAML (`crates/runtime/src/lib.rs:142-148`); `write_config_value` is
  how `set_title_model` persists (`crates/agent/src/supervisor.rs:3891-3895`).
- **Desktop / phone.** `UI/lib/dictation.ts`: `splice(before, after, utts)` joins utterances with spaces and adds a lead/trail space
  (`:43-53`); `applyEvent` replaces the whole utterance per `DictationText` (`:71-77`); `start()` anchors the caret and streams the browser
  mic on a relay engine (`:261-291`). Composer: Ctrl+D / Mic toggles (`UI/components/chat/Composer.tsx:418-438`), Esc discards (`:448-457`),
  the finished value lands as one undo step (`:460-462`, `:493-505`); `submit()` at `:706`. Every shadcn `Input`/`Textarea` has the mic through
  `useFieldDictation → dictation.useBox` (`UI/components/ui/dictation.tsx:56-66`). Shortcuts: `dictate` Control+D window-scope, `voice-key`
  Control+S on macOS / Control+Alt+Space elsewhere, global (`UI/lib/shortcuts.ts:50-54`). Settings has a Voice key tab
  (`UI/components/settings/SettingsDialog.tsx:66`, `:108`) and no Voice tab.
- **TUI.** In-process, same splice semantics and tests (`crates/tui/src/dictation.rs:1-3`, `:138`).
- **Companion (macOS).** Hold-to-talk machine: a press < 0.25 s is a tap, ≥ 0.25 s records, 60 s max (`COMP/CompanionCore/AskVoice.swift:35-38`,
  `:47-74`); empty text → "Nothing recognized" (`:64-67`); K1 surface keys (`:80-92`). `AskDictation` reduces the stream; `finalText` joins finals
  (`:97-105`, `:151`). `HoldToTalk` drops the text into a field via `onText` (`COMP/Companion/Voice/HoldToTalk.swift:5-8`, `:99-103`); the
  Ask bar sends after the 3 s undo hold (`COMP/Companion/AskBar/AskBarPanel.swift:212`, `COMP/Companion/AskBar/Store+Ask.swift:51-57`).
  Voice key: `PointMachine` hold 200 / double-tap 300 / min note 800 / done 6 000 (`COMP/CompanionCore/PointMachine.swift:10-22`); on `.start`
  the recorder and `dictation_start` begin at the press (`COMP/Companion/PointAndSay/PointController.swift:188-204`, `:359-368`) — the voice
  key is already at 0 ms; the Stop sequence conceals the overlays, flushes the text (cap 8 s, `:47-48`), gets `screen_context`, and
  `send_input`s or `steer`s `[voice note, N s] …` + the `[screen …]` lines to theuth's desk (`:271-308`, `:311-313`). The answer card: plain
  `Text` with `lineLimit(12)`, status "working…" / "working · ‹tool›" / "replied" / "couldn't reply — …", "O open the desk · ⎋ dismiss"
  (`COMP/Companion/PointAndSay/AnswerCard.swift:6-7`, `:24-56`, `:88`), shown near the mouse on the screen under it (`:118`, `:162-163`).
  The Ask receipt shows the reply's first two lines (`COMP/CompanionCore/AskRouter.swift:118-127`); the router always targets theuth's desk,
  no agent picker (`:34-41`, `:53-57`). The island hosts the voice view (`COMP/Companion/Island/VoiceModeView.swift:5-21`) on the notched
  screen (`COMP/Companion/Island/IslandPanel.swift:156`). Key table and per-platform defaults: `COMP/CompanionCore/KeyBindings.swift:193-245`,
  `:251-257`; the recorder refuses symbolic hot keys and bare keys (`:318-335`); `Modifiers.function` exists (`:7-15`). Settings tabs: Rail ·
  Keys · Notifications · Engine · Island · Voice key (`COMP/Companion/Settings/SettingsWindow.swift:61-66`).
- **Companion (Windows).** `RegisterHotKey` with `MOD_NOREPEAT` per anywhere row (`WIN/Theuth.Companion/Hotkeys.cs:10-11`, `:95`, `:199`);
  voice key Ctrl+Alt+Space (`WIN/Theuth.Companion.Core/KeyBindings.cs:335`); the same hold thresholds 0.25 / 60 (`WIN/Theuth.Companion.Core/AskVoice.cs:63-66`);
  `DictationLease` is the 149 fix's pure generation guard (`WIN/Theuth.Companion.Core/DictationLease.cs:1-20`); the voice key's answer is drawn
  inside the island window (`WIN/Theuth.Companion/PointAndSay/PointController.cs:419`, `:590-615`); `FirstTwoLines` mirrors the Swift receipt
  (`WIN/Theuth.Companion.Core/AskRouter.cs:168-192`).
- **Agent and tools.** `SendInput { session, text, files, delivery, id }` / `Steer` (`crates/agent/src/lib.rs:245-260`, `:268-278`); side
  calls with a role chain `Escalate → title → session`, `Title → session` (`crates/agent/src/loop_/side.rs:37-61`) and `Side::call(model, system,
  user, budget, kind)` (`:83-90`); `CallKind` lives in `crates/runtime/src/side_call.rs:19-40`. `Risk = ReadOnly | Mutating | Destructive`
  (`crates/tools/src/lib.rs:118-125`), `PolicyAction = Allow | Ask | Deny` (`:444-453`), `tools.policy` per class (`:487-495`). The eight
  `computer_*` tools through one macro (`crates/computer/src/tools.rs:66-88`, `:394-407`); `computer_type` (`:330-339`), `computer_key` with the
  refused chords (`:348-360`), `computer_open` (`:383-392`); the plugin registers its prompt block (`crates/computer/src/lib.rs:297-302`); AX
  helpers `granted()` / `uia_role()` / `hit_at` (`crates/computer/src/ax.rs:134-138`, `:163`, `:736`). The voice-note marker and prompt:
  `note_text` / `is_note` / `PROMPT` (`VOICE/lib.rs:155-179`).
- **Model files.** `scripts/fetch-models.sh` keeps a REQUIRED list current with per-file `{repo, path, revision, etag, size, sha256}` records
  (`:1-13`, `:42-60`); the models catalog is `data/models.json` merged with `<data_dir>/models.json` (`crates/models/src/lib.rs:1-2`, `:316`).
  Sqlite in a crate: `SqliteConnectOptions` + WAL + `sqlx::migrate!("./migrations")` (`crates/artifact/src/store.rs:627-640`).

### 2.2 What is missing (the gaps this design fills)
No shaping, no dictionary, no spoken end command, no history, no latency numbers, no input-device choice, no re-arm on device change, no
delivery into other apps, no rewrite, no command mode outside the agent turn, no AppleScript/Shortcuts/PowerShell tool, a thin two-line Ask
receipt and a card that cannot be followed up in place (study §4 table and appendix B §A1–A8; the cites above confirm each on this tree).
`dictation.rs:7` states "Nothing is written to disk" and 54 §12 rejected persisting dictation audio (`docs/design/54-dictation.md:573-575`) —
D9 amends both (§17, §18).

### 2.3 In flight
`fix/149-island` at `03fe260a` (island eye tint, probe sweeps; local branch, not on main) and its Windows twin `fix/149w-island`. Wave C
(§16) starts after they land; nothing here touches `IslandPanel.swift`'s placement or visibility code, and the voice view keeps being hosted
by the island (96 §0.3 rule, `docs/design/96-point-and-say.md:51-61`).

### 2.4 The latency inference — INFERENCE, measured by D7 before any promise
The study put the first partial at ≥ 0.7 s after speech start (onset 160 ms + one 560 ms window; appendix B §A1(b)). Re-reading the gate:
the onset opens the utterance at `end − (ONSET + PREROLL) × FRAME` (`VOICE/live.rs:153-157`), so 480 ms of the first window are already in
the buffer when the onset is confirmed, and `feed_full` feeds the first window once `total − fed ≥ WINDOW` (`:177-191`) — about 80 ms later.
Structural floor ≈ 160 + 80 = 240 ms after speech start, plus one encoder step and the RPC hop; the first window "often" emits nothing
(`VOICE/stt.rs:255`), so the first *word* commonly waits for the second window (+560 ms). Both numbers are [INFERENCE]; §8.6's metric replaces
them with press → first partial → final per surface, as R5 asks.
