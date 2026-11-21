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
by the island (96 §0.3 rule, `docs/design/96-point-and-say.md:62-70`).

### 2.4 The latency inference — INFERENCE, measured by D7 before any promise
The study put the first partial at ≥ 0.7 s after speech start (onset 160 ms + one 560 ms window; appendix B §A1(b)). Re-reading the gate:
the onset opens the utterance at `end − (ONSET + PREROLL) × FRAME` (`VOICE/live.rs:153-157`), so 480 ms of the first window are already in
the buffer when the onset is confirmed, and `feed_full` feeds the first window once `total − fed ≥ WINDOW` (`:177-191`) — about 80 ms later.
Structural floor ≈ 160 + 80 = 240 ms after speech start, plus one encoder step and the RPC hop; the first window "often" emits nothing
(`VOICE/stt.rs:255`), so the first *word* commonly waits for the second window (+560 ms). Both numbers are [INFERENCE]; §8.7's metric replaces
them with press → first partial → final per surface, as R5 asks.

## 3. The target experience

Four modes, one engine stream. "Dictate here" is what exists today plus shaping and end commands; "dictate anywhere" (D1) and "command"
(D10) are new modes of the companion; "ask" is the voice key and the Ask bar with the new answer card (D12). Every mode shows words as they
close (partials muted, finals solid — 54 §0.3, unchanged), shows the *shaped* text (§4), and never leaves a state without copy (D13).

### 3.1 Dictate here — Composer, TUI, companion field
Unchanged entry points: Ctrl+D / the Mic button in the Composer (`UI/components/chat/Composer.tsx:418-438`), Ctrl+D in the TUI, the mic inside
every desktop `Input`/`Textarea` (`UI/components/ui/dictation.tsx:56-66`), V / the row mic / the Ask bar hold in the companion
(`COMP/Companion/Voice/HoldToTalk.swift:5-8`). What changes: the field shows `shaped` instead of `text`; "send it" sends (Composer `submit()`,
`Composer.tsx:706`; the companion's `commitAsk` after its hold, `COMP/Companion/AskBar/Store+Ask.swift:51-57`); "scratch that" drops the last
closed utterance; a correction suggestion may appear after the send (§6).

| State | What is shown | Copy | What ends it |
|---|---|---|---|
| starting | mic button → level slot, caret hidden (54 §0.7) | tooltip "Stop dictation (Ctrl+D)" | `DictationStarted` → dictating; a refusal → idle with its toast ("already dictating", "talking", model missing) |
| dictating | muted partial, solid finals, wave; a detected end command is underlined in the partial | — | Ctrl+D / Mic → stopping; Esc → discarding; "send it" armed → settling; `dictation_max_secs` → finished (Stopped) |
| settling ("send it") | the phrase struck through, a 1.5 s ring on the stop glyph | "sending in 1.2 s · ⎋ keep editing · ⏎ send now" | silence ≥ 0.35 s after 1.5 s → stop + send; any key/typing → dictating with the phrase removed; Esc → dictating |
| stopping | last partial muted | "finishing…" | `DictationFinished` → the spliced value lands as one undo step (`Composer.tsx:493-505`) |
| discarding | — | — | `DictationFinished` → `before + after` restored (`UI/lib/dictation.ts:225`) |
| suggesting | the `DictionarySuggestion` card under the field (§6.4) | "You changed ‹heard› → ‹typed› after dictating…" | Add / Not now / Never; auto-dismiss 20 s |

### 3.2 Dictate ANYWHERE — any app, from the companion (D1)
Entry: the dictate-anywhere key (§15: ⌃⌥D / Ctrl+Alt+D), hold or lock exactly as the voice key (`PointMachine`, hold 200 / double-tap 300 /
min note 800, `COMP/CompanionCore/PointMachine.swift:10-22`), or the Automatic option (§12.6). The pill shows on the pointer's display
(mock frame A). Words are *delivered as utterances close*: each final utterance is shaped and inserted (direct AX or lease, §13) so the text
appears in the target "in the same second"; partials stay in the pill only. Release → the open utterance is flushed and delivered; "send it"
presses Return in the target when §7.4 allows it.

| State | What is shown | Copy | What ends it |
|---|---|---|---|
| assessing (≤ 150 ms, before the mic is live) | pill with the target app's icon and name | "Dictating into Mail" · "Terminal · unknown field — paste" · refusal "Nothing to type into — ‹role›" | editable/unknown → listening; certainly-not → hint 2.5 s → idle (nothing recorded) |
| listening | rec dot, level bars, muted partial, chord caption | "⌃⌥D held · shape on" · "⎋ cancel · double-tap lock · say “send it”" | release ≥ 200 ms / tap while locked → finishing; Esc → cancelled; another mode's key → §3.5 switch; focus leaves the target app → paused ("Come back to Mail to continue · ⎋") |
| delivering (per closed utterance, concurrent with listening) | the utterance turns solid in the pill as it lands | — | landed → next; not landed → the failure card (§13.5) after the gesture, the words kept |
| finishing | last partial muted | "finishing…" | `DictationFinished` + last delivery verified → done |
| settling ("send it") | as 3.1 | "sending in 1.2 s · ⎋ keep editing" | silence → deliver + Return (if allowed) → done; secure field → done without Return, caption "not sent — secure field" |
| done | pill collapses | "✓ 3 lines into Mail" (1.2 s) | timer → idle |
| failed | failure card at the pointer with the transcript | "Couldn't type into Terminal · paste not landed · Copy · Try again · Open in theuth" | Copy/Try again/Open/⎋; stays until acted on (D13) |
| rewriting | the Rewrite panel: the spoken instruction, the selection excerpt, the proposal | "Replace · Refine (hold) · ⎋" | Replace → focus restored → paste → done; ⎋ → idle |

### 3.3 Ask — the Ask bar and the voice key with the screen
Unchanged triggers (`COMP/CompanionCore/KeyBindings.swift:194-195`, `:218`): ⌃⇧Space held talks into the Ask bar, ⌃S points and says. What
changes: both end in the one AnswerCard (§12) instead of a two-line receipt (`COMP/CompanionCore/AskRouter.swift:118-127`) or the plain-text
card (`COMP/Companion/PointAndSay/AnswerCard.swift:40`).

| State | What is shown | Copy | What ends it |
|---|---|---|---|
| listening | island voice view / Ask field with live words (`VoiceModeView.swift:5-21`) | "· ⌃S held · theuth's desk" | release/lock-tap → finishing; Esc → cancelled |
| finishing | words muted; overlays concealed; screenshot taken | "finishing…" | final text + `screen_context` → thinking; empty → hint "Nothing recognized" |
| thinking | card collapsed: shimmer on "thinking", tool names as they start | "thinking · code_refs · 4 s" | `TurnFinished` → answer; error/cancel → failed |
| answer | collapsed = two lines + "⏎ expand · O open · ⎋"; expanded = markdown, tool list, follow-up field, session dots | — | 6 s `doneMs` only when nothing is speaking and the card is collapsed; expanded cards stay |
| speaking | the accent glyph (149s S4) | "click to stop" | speech ends → answer timer; Esc stops |
| failed | status in the failed colour | "couldn't reply — ‹error›" | Esc; never auto-hides |

### 3.4 Command — the third key (D10)
Entry: the command key (§15: ⌃⌥C / Ctrl+Alt+C), hold/lock as the voice key. The same desk answers under the command profile (§10); the card
is step-typed and ends in one ✓/✗ line; a pending approval appears in the card and is answered by voice ("yes"/"go"/"no") or ⏎/⎋.

| State | What is shown | Copy | What ends it |
|---|---|---|---|
| listening | pill "Command · theuth desk", live words | "⌃⌥C held" | release → heard |
| heard | the request in quotes, first step row pending | "heard → working" | `TurnStarted` → working |
| working | step rows `checked · executed · verifying · next` with the tool's purpose, tool names as they start; shimmer on the running row | "⎋ stop · hold ⌃⌥C to add" | `ApprovalRequested` → needs your ok; `TurnFinished` → done; cap (W-e) → stopped |
| needs your ok | the pending tool call in an amber box with the risk class and policy | "Run `rm -rf …`? destructive · policy: ask · Yes ⏎ No ⎋ or say “yes” / “no”" | voice/keys → `resolve_approval` → working; approval timeout (`tools.approval_timeout_secs`) → denied row |
| done | one line "✓ …" or "✗ …", the step count, "4 steps ls · mkdir/mv · ls · applescript" | "O open session · ⎋ dismiss · hold ⌃⌥C follow up" | 6 s when collapsed; a follow-up hold reuses the card |
| stopped | last rows frozen, "stopped after 12 steps" | "hold ⌃⌥C to continue" | hold → working on the same thread; ⎋ |

### 3.5 The key gestures (both platforms; §15 has the chords)
- **Hold ≥ 200 ms** = talk, release = act; **tap then press within 300 ms** = lock hands-free, next tap = act; **Esc** cancels between press and
  send; notes < 800 ms are discarded with the hint — the 96/149 machine, untouched (`crates/desktop/src/point/machine.rs:5-18`, the Swift port
  `COMP/CompanionCore/PointMachine.swift:10-22`, the C# port `WIN/Theuth.Companion.Core/PointMachine.cs`).
- **Mode switch while holding (R2).** Pressing another mode's key while a gesture is live switches the mode of *that* gesture (dictate ↔ ask ↔
  command) — the recording continues, the pill relabels, the destination changes at release. Implemented as a `PointMachine.Event.switchMode`
  carried by the controller (§14.4); technique A:L173.
- **Automatic (opt-in, W-i).** Tap < 0.4 s toggles (press again to stop), a longer press is push-to-talk; replaces double-tap lock while chosen
  (§12.6; technique A:L170).
- **Chord protection.** A modifier-only chord whose hold saw another key is not a release (A:L172); repeats never toggle (the 149f `VoiceKeyGate`
  already filters auto-repeat, `docs/design/149-as-built-voice-key-fix.md:82-88`).
- **Composer and TUI** keep Ctrl+D / Esc; **"send it"** is the only spoken way out (§7).

## 4. The shaping layer `voice::shape`

A pure module in `crates/voice` (`VOICE/shape/{mod,punct,literal,dictionary,end,tests}.rs`), no I/O, no clock, no actor; the dictation task
calls it once per piece. Techniques after A:L60-71 (order, spacing classes, context gates, clean-up of the model's own punctuation,
terminal-autocomplete spacing), written from scratch.

### 4.1 Contract (frozen; §14 repeats the types)
```rust
/// Per-surface switches; resolved once at `StartDictation` from `voice.shape.<surface>` (§4.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ShapeConfig { pub punctuation: bool, pub literals: bool, pub numbers: bool, pub dictionary: bool,
                         pub end_commands: bool, pub terminal_spacing: bool }
/// What the surface receives beside the raw text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Shaped { pub text: String, pub command: Option<EndCommand>, pub actions: Vec<Action>,
                    /// Ranges of `text` the dictionary rewrote (for the muted/underline render and §6's diff).
                    pub replaced: Vec<Replaced> }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum EndCommand { /// "send it": armed; `near_miss` when accepted by tolerance at the final.
                      Send { near_miss: bool }, /// "scratch that": drop the previous closed utterance.
                      ScratchThat }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Action { NewLine, NewParagraph, Tab }
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Replaced { pub start: usize, pub end: usize, pub heard: String, pub rule: RuleId }
/// Spoken end phrases, configurable (§7); `literal` escapes the next phrase.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EndPhrases { pub send: Vec<String>, pub scratch: Vec<String>, pub literal: String }

pub struct Shaper<'a> { cfg: ShapeConfig, phrases: &'a EndPhrases, dict: &'a Dictionary, app: Option<&'a AppProfile> }
impl Shaper<'_> {
    /// `prev`: the surface's text before this utterance (context for the first token); `raw`: the model's text so far;
    /// `is_final`: the VAD closed it. Deterministic; `shape(.., raw, true)` is the text delivered.
    pub fn shape(&self, prev: &str, raw: &str, is_final: bool) -> Shaped;
}
```
`Dictionary` is §5's matcher (read-only snapshot, `Arc`); `AppProfile` is §13's per-app row (bundle id / exe, `terminal: bool`, polish). The
function never panics on any UTF-8 input and is `O(tokens × rules)`; the dictionary stage is the only one that may cost more than a few µs
(§5.4 bounds it).

### 4.2 Stages, in order (each one a function over the token list; off switches skip the stage)
1. **Tokenise** the raw text into words and the model's own punctuation marks (kept as separate tokens with an `asr: true` flag). The
   model's casing is kept.
2. **Spoken punctuation** (`punctuation`). A phrase → symbol table with a *spacing class* per symbol: right-attached `, . ? ! : ;` ("comma",
   "period"/"full stop", "question mark", "exclamation mark"/"bang", "colon", "semicolon"); left-attached openers `( [ { " “ $ €`; right-attached
   closers `) ] } " ”`; spaced operators `+ = & | < > —` ("plus", "equals", "ampersand", "pipe", "less than", "greater than", "em dash");
   tight joiners `- _ / . @ # ~ ` ("dash"/"hyphen", "underscore", "slash", "dot"/"point", "at sign", "hash"/"pound", "tilde", "backtick").
   Actions "new line", "new paragraph", "tab" become `Action`s and a `\n` / `\n\n` / `\t` in `text` (W-j). "literal ‹phrase›" writes the phrase.
3. **Context gates** on the ambiguous words — the rule that makes "dot" safe in prose:
   - "dot"/"point" → `.` only when the left token is alphanumeric and the right token is a known TLD or file extension (from the packaged
     lists, §5), or a version/number continues (`two point five`), or the right token is a lowercase identifier ≤ 24 chars with no space
     before the next punctuation (`deploy dot S H` → `deploy.sh` via the spelled-letters rule below). Otherwise the word stays.
   - "slash" → `/` only in path context: left or right token is path-like (`~`, `.`, `..`, a known command, a dictionary term tagged `path`,
     or already contains `/`), or the utterance starts with "slash ‹known command›" (stage 4). "forward slash" always converts; "back slash" → `\`.
   - "dash"/"hyphen" → `-` between two alphanumerics with no pause token between; "dash dash ‹words›" → `--‹words-joined›` (stage 4).
   - "underscore" → `_` joins its neighbours; "double underscore" → `__`.
   - operators ("plus", "equals", "minus", "times") → symbols only when both neighbours are short operands (digits, single letters, identifiers
     ≤ 12 chars); "two plus two equals four" → `2 + 2 = 4`; "the plus side" stays prose.
   - Spelled letters: a run of ≥ 2 single uppercase letters as the model writes them (`S H`, `T D T`) joins to `SH`, `TDT`; lowercase after
     "dot" (`sh`).
4. **Literal formatter** (`literals`): `/commands` — "slash ‹token›" at the utterance start or after a lead-in ("type", "run", "send") when
   `token` is in the surface's command list (`DictationOpts.commands`, the Composer's slash menu; the TUI's; `AppProfile.commands` for a
   terminal agent) → `/token`, else stays; `@mentions` — "at ‹name›" when `name` is an agent name (`agents_list`) or a dictionary term tagged
   `mention` → `@name`, possessives kept ("at theuth's" stays); paths — `~`, `.`, `..` + "slash" chains join without spaces; flags — after `--`
   or `-` the next 1–3 lowercase words join with `-` until a pause token, a function word ("and", "then", "to") or punctuation; identifiers —
   "camel case ‹words›" → `fooBar`, "snake case" → `foo_bar`, "kebab case" → `foo-bar`, "pascal case" → `FooBar`, "constant case" → `FOO_BAR`,
   "all caps" → `FOO BAR`, "dotted" → `foo.bar`; the formatter consumes words until a pause token or ≤ 4 words.
5. **Numbers** (`numbers`): cardinals/ordinals ≤ 6 digits ("twenty three" → `23`, "third" stays unless preceded by "number"), "point"
   between number words → decimal, times ("six pm" → `6 pm` unchanged; "eighteen hundred" stays), units untouched. Inverse text
   normalisation beyond this is not in scope (the datasets sheet §5 names NeMo ITN as a later port).
6. **Model-punctuation clean-up**: an `asr: true` mark adjacent to a converted symbol of the same class is dropped (no `, ,`); a trailing
   model period after an action or an end phrase is dropped; the existing leading-mark trim stays in `piece()` (`VOICE/live.rs:221-233`).
7. **Dictionary replacements** (`dictionary`, §5): over word n-grams 1–3, longest match first; a rule never re-expands a completed occurrence of
   its own output (Git → GitHub never gives GitHubHub — A:L90's guard, implemented as "skip spans produced by this rule in this pass");
   `Replaced` spans recorded.
8. **End-command detection** (`end_commands`, §7): the trailing ≤ 3 tokens against `EndPhrases`; "literal send it" writes the words.
9. **Terminal-autocomplete spacing** (`terminal_spacing`): when the surface is the TUI or an any-app `AppProfile.terminal`, a final utterance
   ending in a `/command` or `@mention` gets no trailing space (so the CLI's completion menu stays open — A:L71); otherwise the splice adds
   its usual separator (`UI/lib/dictation.ts:49-51`).

### 4.3 Examples table (the unit-test fixture, `VOICE/shape/tests.rs`; surface Composer unless noted)
| # | Spoken (model text) | Written (`shaped.text`) | Stage / gate |
|---|---|---|---|
| 1 | `Hi Dana comma the deploy is done period` | `Hi Dana, the deploy is done.` | 2 right-attached |
| 2 | `open paren see below close paren` | `(see below)` | 2 openers/closers |
| 3 | `the plus side of this` | `the plus side of this` | 3 operator gate fails (prose) |
| 4 | `two plus two equals four` | `2 + 2 = 4` | 3 operands short, 5 numbers |
| 5 | `send it to theuth dot dev` | `send it to theuth.dev` | 3 "dot" + TLD |
| 6 | `connect the dots here` | `connect the dots here` | 3 no gate ("dots" is not the word) |
| 7 | `deploy dot S H` | `deploy.sh` | 3 spelled letters + extension |
| 8 | `version two point five` | `version 2.5` | 3 "point" in a number |
| 9 | `slash commit` | `/commit` | 4 known command at start |
| 10 | `I'll slash the budget` | `I'll slash the budget` | 3 no path context |
| 11 | `tilde slash develop slash theuth` | `~/develop/theuth` | 3 path chain |
| 12 | `dash dash dry run flag on deploy dot S H` | `--dry-run flag on deploy.sh` | 4 flags (2 words joined), 3 |
| 13 | `dash V` | `-V` | 4 short flag |
| 14 | `a well dash known bug` | `a well-known bug` | 3 dash between alphanumerics |
| 15 | `snake case max open secs` | `max_open_secs` | 4 identifier |
| 16 | `camel case on text delta` | `onTextDelta` | 4 identifier |
| 17 | `pascal case answer card` | `AnswerCard` | 4 identifier |
| 18 | `underscore underscore init underscore underscore` | `__init__` | 3 underscore joiner |
| 19 | `at theuth can you check` | `@theuth can you check` | 4 mention (agent name) |
| 20 | `at theuth's desk` | `at theuth's desk` | 4 possessive guard |
| 21 | `new line thanks comma Sky` | `\nThanks, Sky` | 2 action + sentence case after `\n` |
| 22 | `first paragraph new paragraph second` | `first paragraph\n\nsecond` | 2 action |
| 23 | `twenty three files` | `23 files` | 5 numbers |
| 24 | `the third option` | `the third option` | 5 ordinals stay |
| 25 | `literal send it` | `send it` | 8 escape |
| 26 | `ship it tonight send it` | `ship it tonight` + `command: Send` | 8 end command |
| 27 | `ship it tonight sent it` (final) | `ship it tonight` + `Send { near_miss: true }` | 8 tolerance len/5 after arming on a partial |
| 28 | `hello, comma world` | `hello, world` | 6 clean-up (model comma beside spoken comma) |
| 29 | `parakeet tee dee tee model` | `Parakeet TDT model` | 7 dictionary (learned) |
| 30 | `git hub actions` | `GitHub actions` | 7 dictionary (packaged); 7 guard: a second pass leaves `GitHub` |
| 31 | `slash compact` (TUI, final) | `/compact` with no trailing space | 9 terminal spacing |
| 32 | `pipe grep voice` (any-app terminal) | `| grep voice` | 2 spaced operator in a terminal profile |
| 33 | `` backtick cargo test backtick `` | `` `cargo test` `` | 2 tight joiner pair |
| 34 | `hash one five three` | `#153` | 2 hash + 5 numbers |
| 35 | `so yeah period` (partial) | `so yeah.` | W-a: identical on partial and final here; #27 differs |

### 4.4 Partial vs final (W-a, writer's call)
Shaping runs on **every** piece. A partial is muted text the user cannot edit (54 §0.7); its shaped form may change at the final (#27, or
"dot" resolving when the TLD arrives) and every client already replaces the whole utterance per event (`UI/lib/dictation.ts:73-76`,
`COMP/CompanionCore/AskVoice.swift:118`, `crates/tui/src/dictation.rs:87`). Rules that need the *next* token (dot/slash gates, flag joins)
resolve conservatively on a partial (leave the word) and finally at `is_final`. End-command detection must run on partials (D5: arming,
§7). Delivery into other apps happens only on finals (§3.2), so nothing is ever typed and retracted.

### 4.5 Per-surface config and where it lives
```yaml
voice:
  shape:            # preset per surface: full | plain | off   (plain = punctuation + literals + numbers, no dictionary/end commands)
    enabled: true   # 201: false bypasses the shaper on every surface (model text verbatim, no end commands, nothing learned); wins over the presets
    composer: full
    tui: full
    companion: full
    phone: full
    any_app: full
  end_commands:
    send: ["send it"]
    scratch: ["scratch that"]
    literal: "literal"
```
Written by `set_shape(surface, preset)` through `runtime::write_nested_config_value(path, "voice", "shape", surface, preset)`
(`crates/runtime/src/lib.rs:142-148`); read into `VoiceConfig.shape: ShapePresets` (`#[serde(default)]`, `VOICE/lib.rs:424-448` gains the
field). `Surface` travels in `DictationOpts` at `dictation_start` (W-c); `None` → `Composer` (today's callers change nothing). The TUI
passes `Tui`, the companion `Companion` for its fields, `AnyApp` with the `AppProfile` for §13, the phone webview `Phone`.

### 4.6 Where the actor applies it
`Task` (`VOICE/dictation.rs:537-543`) gains `shaper: ShaperOwned` (config + `Arc<Dictionary>` snapshot + `EndPhrases` + optional
`AppProfile`) resolved in `begin` (`:231-309`) from the `StartDictation.opts`; `Task::texts` (`:625-633`) becomes: for each `live::Text`,
`let s = self.shaper.shape(&self.prev_text, &t.text, t.is_final)`, then `Progress::Text { utterance, text: t.text, shaped: s.text, command:
s.command, is_final }` (`:142-146` gains the fields); `Report` → `VoiceEvent::DictationText` (`:512-521`) carries them. `prev_text` is the
finals so far joined (the task has them; the surface's `before` is not known to the engine — the first token of a dictation is shaped as
sentence start). "scratch that" is executed by the actor: the previous closed utterance is re-sent as `DictationText { text: "", shaped: "",
final: true }` so every client's per-utterance replace empties it, and §6's history marks it. The dictionary snapshot is taken per dictation;
a user entry added mid-dictation applies from the next one.

### 4.7 Unit-test list (`cargo test -p voice shape::`)
`examples_table_rows_1_to_35` (one `#[test]` per row via a table macro); `spacing_classes_never_double_space`; `dot_gate_tld_list_and_extensions`;
`slash_gate_needs_path_context`; `flag_join_stops_at_function_word`; `identifier_formatters_consume_at_most_four_words`; `numbers_cap_six_digits`;
`model_punctuation_cleanup_only_adjacent`; `dictionary_self_expansion_guard`; `end_command_arms_on_partial_and_sticks`; `near_miss_tolerance_len_over_5_min_5_chars`
("end" never passes as "send"); `literal_escapes_end_phrase_and_symbol`; `terminal_spacing_only_on_terminal_surfaces`; `partial_then_final_is_idempotent_on_plain_prose`;
`shape_is_total_over_fuzzed_utf8` (proptest, 10 k cases, no panic); `presets_full_plain_off_resolve`; `scratch_that_drops_previous_final_only`.

## 5. Dictionary

Three layers behind one matcher: the **packaged** technical dataset (shipped, read-only), **user** entries (Settings, `dictionary_put`) and
**learned** entries (§6's Add). No seeding from the user's computer or project (D3, R4; §5.7). Summary of the datasets sheet
`docs/design/153-datasets.md` (fv-datasets, 2026-10-09; every source cited there) follows — it is the source of record for licences.

### 5.1 Packaged dataset — sources, counts, licences (`153-datasets.md:7-26`, `:122-140`)
| Layer | Source | What we take | Count | Licence | Sheet |
|---|---|---|---|---|---|
| spoken → written core | talonhub/community (pushed 2026-10-08) | symbols with dictation/command forms (≈45), 17 casing formatters, 436 abbreviations, number words, homophones, file extensions, TLDs, per-language keywords | ≈500 rules + lists | MIT | `:9-15`, `:41-46` |
| commands and tools | Homebrew `formula.json` + `cask.json`; tldr-pages names; oh-my-zsh plugin names | names, aliases, oldnames (brew); page names (tldr); 371 plugin names | brew thousands (not verified); tldr > 1 000; omz 371 | BSD-2 (brew API); CC BY 4.0 pages / MIT scripts (tldr); MIT (omz) | `:17-19`, `:59-61` |
| languages | GitHub linguist `languages.yml` | names + aliases | ≈830 | MIT | `:20`, `:62` |
| packages | hugovk top-pypi-packages | names | 15 000 | no licence file; bare names are facts | `:21`, `:63` |
| cloud | botocore `data/` service ids; Kubernetes OpenAPI kinds; Terraform registry providers | ids / kinds / `namespace/name` | ≈430 AWS; k8s/TF not counted | Apache-2.0; Apache-2.0; registry terms not checked | `:22`, `:64-66` |
| AI vocabulary | models.dev `api.json`; Hugging Face top-N; a **hand-curated** harness list (Claude Code, Codex, Cursor, OpenCode, Gemini CLI, omp, theuth's agent names) | provider/model ids and names; org names | a few thousand (not counted) | MIT; HF terms not checked; ours | `:23`, `:67-68` |
| phonetics | CMUdict (ARPAbet) for the spoken-form generator and variant seeding | pronunciations of the canonical terms only | subset | CMU BSD-style (notice kept) | `:24`, `:85` |
**Kept out** (share-alike or unclear, `:28-35`): Wikipedia lists, Stack Overflow tags, the Kaggle TN corpus (CC BY-NC-SA), GPL/AGPL/LGPL app
tables (FluidVoice, VoiceInk, Whispering, Caster/dragonfly), Jargon File, Microsoft Terminology. Approximate size 30–40 k canonical terms
(`:26`). Attribution: `crates/voice/data/THIRD_PARTY.md` carries each licence/NOTICE (`:124-130`).

### 5.2 Packaging — a data file with a sha256 record, `fetch-models.sh` style
`scripts/build-dictionary.sh` (offline, run by the maintainer, not at build) fetches the sources above, normalises to one JSON Lines list
`{canonical, kind, aliases[], spoken[], tags[]}`, derives spoken forms (the talon `create_spoken_forms` idea — split case, expand digits and
symbols — ported to Rust in `VOICE/dictionary/spoken.rs`, `153-datasets.md:46`), and writes `crates/voice/data/dictionary.v1.json.gz`
(`PackagedSet { version, built_at, terms: Vec<Term>, symbols, formatters, abbreviations, tlds, extensions }` as serde_json, gzip through
`flate2` — both workspace dependencies already, `Cargo.toml:186`, `:242`; ≈ 3–5 MB) plus `dictionary.json` `{ "file": "dictionary.v1.json.gz",
"size": N, "sha256": "…", "sources": [...] }` — the record shape of `scripts/fetch-models.sh:11` (`{repo, path, revision, etag, size, sha256,
fetched_at}`). The file is `include_bytes!`'d (no download, no REQUIRED-list change; a later `v2` may move to the models bucket when it
outgrows the binary — then it joins REQUIRED with its sha256). At load the hash is verified once (proof row 3); the loader is ~40 lines.

### 5.3 Entries, scopes, variants, negatives
```rust
pub struct Entry { pub id: EntryId, pub canonical: String, pub kind: Kind /* Term | Symbol | Formatter | Abbrev | Mention | Command | Path */,
                   pub spoken: Vec<String>, pub variants: Vec<Variant>, pub scope: Scope, pub source: Source /* Packaged | User | Learned */,
                   pub tags: Vec<String> /* "tld", "ext", "mention", "path", "terminal" … */ }
pub struct Variant { pub heard: String, pub metaphone: (String, String), pub seen: u32, pub last_ms: u64 }
pub enum Scope { Global, Surface(Surface), App(String) }          // per-term scope; an any-app term can be limited to one app
pub struct Negative { pub heard_lc: String, pub canonical: String }  // "this heard form must NOT become canonical" (Never, or an unchanged delivery)
```
User entries come from Settings › Voice › Dictionary (desktop) / Settings › Dictation (companion) and `dictionary_put`; learned entries from
§6.3's Add carry the heard variant and `source: Learned`. A **negative** is written by "Never for this" and — the cheap acoustic-free version
of A:L89 — by any delivered utterance the user *sent unchanged* that contained the heard form (the pair is then vetoed for that surface).

### 5.4 Matcher algorithm (`VOICE/dictionary/matcher.rs`, pure; W-k constants)
1. **Candidates**: for every 1–3-token window of the utterance, look up (a) exact `spoken`/`heard` forms (hash map), (b) Double Metaphone
   primary/alternate codes of the window (`rphonetic`, Apache-2.0 — `153-datasets.md:87`) in a code → entries map. ≤ 32 candidates per window.
2. **Score**: `s = 1 − lev(norm(heard), norm(window)) / max(len)` using `strsim` (`:89`) over lowercase, digit-spelled text; a phonetic hit with
   `lev ≤ EDIT_MAX = 2` adds 0.1. `norm` folds the spelled-letters rule (§4.2 stage 3) so `tee dee tee` ≈ `tdt`.
3. **Accept**: `s ≥ ACCEPT_KNOWN = 0.70` when the entry already has this `heard` as a variant (or is a user entry), else `s ≥ ACCEPT_NEW = 0.85`;
   ties → longest window, then `source` order User > Learned > Packaged, then catalog order.
4. **Veto**: a `Negative` for `(heard_lc, canonical)` on this scope refuses regardless of score — negatives never *create* a match (A:L89).
5. **Guard**: a window that lies inside a span produced by *this* rule in this pass is skipped (self-expansion, §4.2 stage 7); possessives on
   the heard form are carried to the output (`kubernetes's` → `Kubernetes's`).
6. **Scope**: `Global` always; `Surface(s)` only when `DictationOpts.surface == s`; `App(id)` only when `AppProfile.id == id`.
7. **Bound**: the packaged set is indexed at load (two hash maps + an `fst` set for prefix checks, `:91`); a 40-token utterance costs ≤ 120
   window lookups — microseconds. No fuzzy scan over 30 k terms per token.
8. **Casing**: the canonical's casing wins (`GitHub`, `PostgreSQL`); the model's sentence-initial capital is re-applied if the match starts the
   utterance and the canonical is all-lowercase (`npm` stays `npm` — tag `keep_case`).
Tests: `match_thresholds` (proof row 4), `phonetic_hit_needs_edit_le_2`, `scope_filters`, `negative_vetoes_never_creates`, `possessive_carried`,
`casing_rules`, `packaged_index_builds_under_200ms` (`#[ignore]` perf), `learned_variant_lowers_threshold_to_known`.

### 5.5 Storage
Packaged: in the binary (§5.2). User + learned + negatives: `voice.db` (§6.5) tables `dictionary_entries (id, canonical, kind, spoken_json,
scope, source, tags_json, created_ms)`, `dictionary_variants (entry_id, heard, metaphone1, metaphone2, seen, last_ms)`, `dictionary_negatives
(heard_lc, canonical, scope, created_ms, PRIMARY KEY(heard_lc, canonical, scope))`. The actor holds an `Arc<Dictionary>` snapshot rebuilt on
every write (cheap: the user layer is small) and handed to each new dictation (§4.6). Export/import: `dictionary_list` → JSON in Settings
("Export my words"), `dictionary_put` per row on import.

### 5.6 API
`dictionary_list(scope)`, `dictionary_put(entry)`, `dictionary_delete(id)`, `dictionary_suggestion_answer(id, answer)` (§14.2); the actor
message `DictionaryOp { List | Put(Entry) | Delete(EntryId) | Negative(Negative) }`; `voice_status` reports `dictionary: { packaged_version,
packaged_terms, user_entries, learned_entries }` for the Settings header line ("32 410 technical terms · 14 of yours · 6 learned").

### 5.7 REJECTED: project/computer seeding (D18) — and the questions an algorithm must answer first
Seeding from the code index, agent names, knowledge titles, installed apps or the shell history is **not** built. R4's reason stands: theuth is
not a coding agent; it drives harnesses whose vocabularies vary by project, and a wrong seed ("Git" → a project symbol) misfires on every
surface. What would reopen it — one page answering, for each proposed source:
| Question | What it must say |
|---|---|
| **source** | which store (code index symbols? `agents_list` names? knowledge titles? app names?) and why its terms are *spoken* often enough to matter |
| **rate** | how many terms per project, how often re-derived, the cap, and the cost of a false expansion per 1 000 words |
| **trigger** | when a term enters (first open? after N mentions in sessions? only after one correction, i.e. §6's path with the project as a *hint*?) |
| **veto** | how a term leaves (negatives as §5.3? per-project scope only? user review before activation?) |
Until then the only project-aware path is the one already here: `@mentions` resolve against `agents_list` at shaping time (§4.2 stage 4) — a
lookup, not a seed.

## 6. Correction learning and history

### 6.1 Observation in our own fields (D4) — no accessibility tree needed
The surfaces we own know exactly what was dictated and what was sent. Each keeps, per dictation id, the shaped spans it inserted (the
`Splice` region: `UI/lib/dictation.ts:43-53`; `AskDictation.finalText`, `COMP/CompanionCore/AskVoice.swift:151`; the TUI's `splice`,
`crates/tui/src/dictation.rs:138`) and, at **send** (Composer `submit()`, `Composer.tsx:706`; the companion's `commitAsk`,
`COMP/Companion/AskBar/Store+Ask.swift:64`; the TUI's send), reports `dictation_correction(id, sent_text)` (§14.2) with the text actually
sent. The engine diffs the dictated region against the corresponding slice of `sent_text` (anchored by the surrounding unchanged words, ±8
tokens — the grounding idea of A:L87, over text instead of audio). No report when the field was cleared or the dictation was discarded.

### 6.2 Policy constants (`VOICE/history/learn.rs`, pure, tested)
| Constant | Value | Rule |
|---|---|---|
| `SINGLE_WORD` | both sides one token, ≤ 40 chars, ≥ 2 alphanumerics, a letter on each side | multi-word edits are grammar, not vocabulary |
| `INFLECTIONS` | `'s s es ies ed ing` | an edit that only adds/removes one of these is rejected (A:L85's rule, our list) |
| case-only edits | allowed | brand capitalisation ("github" → "GitHub") is a valid rule |
| `SUGGEST_AFTER` | 2 occurrences in 7 days (same heard → typed pair, case-insensitive on heard) | one slip is noise |
| `DISMISS_COOLDOWN` | 7 days per pair | "Not now" is respected |
| `IGNORES_PER_SESSION` | 3 | after three ignored cards in one engine run, no more cards until restart |
| `NEVER` | permanent per pair | "Never for this" writes a negative (§5.3) |
| `PCM_RETENTION` | 120 s per dictation, max 15 min total | the audio of the corrected utterance can be played in the card ("▶ play what you said") and is the ground truth for §5's acoustic variant |

### 6.3 Candidate → suggestion flow
1. `dictation_correction` lands → `learn::candidates(dictated_spans, sent_text) -> Vec<Candidate { heard, typed, utterance, offset }>`.
2. Each candidate passes `SINGLE_WORD` and `INFLECTIONS`; survivors are appended to `corrections` (§6.5) with the dictation id.
3. `learn::due(pair, history, now)` → when `SUGGEST_AFTER` holds and no cooldown/never row exists and the session's ignores < 3, the engine
   publishes `VoiceEvent::DictionarySuggestion { id, heard, typed, occurrences, surface, has_audio }`.
4. The surface that produced the last occurrence shows the card (W-o); the answer goes back as `dictionary_suggestion_answer(id, Add | NotNow |
   Never)`. `Add` writes a user entry with the heard variant (§5.3); `NotNow` writes the cooldown; `Never` a negative.

### 6.4 The suggestion card (one component per client; mock frame D)
```
┌ Dictionary · 2 times this week ──────────────────────────────────┐
│ You changed “parakeet tee dee tee” → “Parakeet TDT” after        │
│ dictating. Heard the same way twice (Composer, 7 days).          │
│ Add it as a replacement?                                          │
│ [ Add ]  [ Not now ]  [ Never for this ]        ▶ play what you said │
└───────────────────────────────────────────────────────────────────┘
```
Desktop: `components/ui/DictionarySuggestion.tsx` under the Composer / field; TUI: a one-line prompt under the composer (`a`dd · `n`ot now ·
`v`never · `p`lay); companion macOS: a `GlassPanel` card at the Ask bar's anchor; Windows: a `ToolWindow` twin. Auto-dismiss 20 s counts as
"Not now" without the cooldown. "play" fetches `dictation_audio_get(id, utterance)` (§14.2) — 16 kHz i16 — and plays it locally.

### 6.5 History store schema (D9, W-f) — `<data_dir>/voice.db`, sqlx, migrations under `crates/voice/migrations/` (the `crates/artifact`
pattern, `crates/artifact/src/store.rs:627-640`)
```sql
CREATE TABLE dictations (
  id TEXT PRIMARY KEY,            -- DictationId (uuid v7)
  started_ms INTEGER NOT NULL, finished_ms INTEGER,
  surface TEXT NOT NULL,          -- composer | tui | companion | phone | any_app
  destination TEXT NOT NULL,      -- agent_prompt | document | message_reply | any_app:<bundle or exe>
  app TEXT,                       -- any-app: bundle id / exe; else NULL
  raw TEXT NOT NULL, shaped TEXT NOT NULL, polished TEXT,   -- finals joined; polished NULL unless §9 ran
  delivered TEXT,                 -- what actually went out (polished ?? shaped), after end-command edits
  press_ms INTEGER, first_partial_ms INTEGER, first_final_ms INTEGER, final_ms INTEGER,  -- §8.6 (relative to press)
  reason TEXT NOT NULL,           -- done | stopped | error:<message>
  audio_bytes INTEGER NOT NULL DEFAULT 0, audio_path TEXT   -- <data_dir>/voice/history/<id>.pcm16, NULL when evicted
);
CREATE TABLE corrections (
  id INTEGER PRIMARY KEY, dictation_id TEXT NOT NULL REFERENCES dictations(id) ON DELETE CASCADE,
  utterance INTEGER NOT NULL, heard TEXT NOT NULL, typed TEXT NOT NULL, at_ms INTEGER NOT NULL
);
CREATE TABLE suggestion_state (          -- cooldowns and nevers, keyed by the pair
  heard_lc TEXT NOT NULL, typed TEXT NOT NULL, state TEXT NOT NULL,   -- cooldown | never | added
  until_ms INTEGER, PRIMARY KEY (heard_lc, typed)
);
CREATE INDEX dictations_started ON dictations(started_ms);
```
Budget: `HISTORY_ROWS = 200` (older rows deleted, cascade), `AUDIO_BUDGET = 64 MiB` (oldest `audio_path` files unlinked first, row kept with
`audio_path NULL`), PCM kept ≥ 120 s regardless of budget for the newest dictation. The task streams the 16 kHz f32 → i16 PCM to the file
while dictating (the only new disk write in the dictation path; `dictation.rs:7`'s "Nothing is written to disk" is amended, §17); a private
session (`SessionResources.is_private`, the pattern at `crates/agent/src/loop_.rs:5302`) or `voice.history.enabled: false` writes neither
row nor audio.

### 6.6 Any-app observation (W-g: designed, optional, not in wave D)
For a delivery into another app the companion may watch the target element for 30 s after the paste (AX `kAXValueChanged` /
`kAXSelectedTextChanged` notifications, 20 verification reads × 50 ms, a 3 s inactivity stop, fields > 100 000 chars ignored — A:L83) and
report `dictation_correction` the same way. Gated by `AppProfile.learn: bool` (default false) and by Accessibility being granted. Windows twin:
UIA `TextPattern` `TextChanged` event on the focused element. Ships only after wave D's delivery numbers are in (§16).

### 6.7 Undo polish, budget, privacy
- **Undo polish** (D8): `dictation_history(limit)` returns rows with `raw/shaped/polished`; the surface's history row offers "use the raw text" —
  the Composer re-splices, any-app re-delivers through the same lease (§13.3). The companion's hold pill gets "⎋ undo polish" for 3 s after a
  polished delivery (the existing `UndoQueue` hold, 149 §5.4 A).
- **Settings › Voice › History**: rows (time · surface → destination · the delivered text, raw on hover · timings · ▶ when audio exists · ✕),
  "Clear history", "Keep audio" (default on, 64 MiB), "Learn from my corrections" (default on), "Off" switch for the whole store.
- **Privacy**: local only; nothing leaves the machine; the agent never reads the store; the Telegram/Slack/Signal channels and the mesh have no
  RPC to it; a relay peer gets `dictation_history` refused like the daemon mic (`api.rs:6279-6283`'s rule).

## 7. Spoken end commands

### 7.1 Parser on partials (`VOICE/shape/end.rs`, a state machine the shaper feeds per piece)
```
Idle ──(tail of a partial matches a send phrase exactly)──▶ Armed{utt, at_ms}
Armed ──(later partial of the same utterance drops the phrase: re-decode "send it"→"sent it"/"send")──▶ Armed   (sticky: once armed, stays)
Armed ──(final: phrase present, or a near miss within max(1, len/5) edits for phrases ≥ 5 chars; nothing for shorter)──▶ Send{near_miss}
Armed ──(final without the phrase and beyond tolerance, or the user typed / pressed a key)──▶ Idle
Any   ──("literal" precedes the phrase)──▶ Idle (the words are written)
```
The arming is per utterance; the phrase and the words after it are removed from `shaped.text`. Technique after A:L95 (arming, stickiness,
tolerance); constants ours: `ARM_ON_PARTIAL = true`, `NEAR_MISS_MIN_LEN = 5`, `TOLERANCE = max(1, len / 5)`.

### 7.2 Phrases (configurable, `voice.end_commands`, §4.5)
| Command | Default phrases | Effect |
|---|---|---|
| `Send` | "send it" | §7.4 per surface |
| `ScratchThat` | "scratch that" | the previous *closed* utterance is emptied (actor re-sends it empty, §4.6); on a partial it only arms |
| literal | "literal" | the next phrase or symbol word is written as said |
| actions | "new line", "new paragraph", "tab" | shaping actions, not end commands (W-j) |

### 7.3 Settle and silence (D5)
When `Send` arms on a partial the surface shows "sending in 1.5 s"; the actor starts `SETTLE = 1.5 s` and stops the dictation itself when,
after the settle, `SILENCE = 0.35 s` of frames below the gate threshold have passed (the `Gate` already counts silent frames,
`VOICE/vad.rs:111-112`). Speech inside the window resets the settle; the user typing or pressing a key (surfaces call `dictation_stop`
themselves) ends it. The event `DictationText { command: Send }` is what the surface acts on after `DictationFinished`.

### 7.4 Per-surface actions
| Surface | "send it" | "scratch that" | Secure field |
|---|---|---|---|
| Composer | the spliced value lands, then `submit()` (`Composer.tsx:706`) — a steer if a turn runs, exactly like ⏎ | empties the utterance; the splice re-renders | n/a |
| TUI | same: splice then the send key's path | same | n/a |
| Companion Ask bar / V / row mic | `setText` then `commitAsk` with **no 3 s hold** (the phrase was the confirmation; `COMP/Companion/AskBar/Store+Ask.swift:51-57`) | same | n/a |
| Voice key / command | release already sends; "send it" while locked = the tap that sends | same | n/a |
| Any-app (§13) | deliver the last utterance, then press Return **only** if the target is `editable` and not `AXSecureTextField` / UIA `IsPassword`, focus is still the captured element, and no physical modifier is down; else deliver without Return and caption "not sent — secure field" / "not sent — focus moved" | re-deliver the utterance as empty = not possible after paste → the card offers "undo last" (⌘Z/Ctrl+Z into the target, once, only if focus unchanged) | never Return |

### 7.5 Tests
`end::arms_on_partial`, `end::sticky_across_redecode`, `end::near_miss_only_when_armed_and_len_ge_5`, `end::end_is_not_send`,
`end::literal_disarms`, `end::scratch_arms_then_fires_on_final`, `dictation::send_it_settle_then_silence_stops` (actor test with a synthetic
remote feed: phrase in a partial, 1.5 s, 0.35 s of zeros → `DictationFinished { Done }` and the `Send` command on the last text event),
`dictation::typing_cancels_settle` (a `StopDictation` mid-settle wins), Swift `AskVoiceMachineTests.sendItSkipsTheHold`, TS
`Composer.test.ts: send it submits once`.

## 8. Capture robustness and latency instrumentation

### 8.1 Input device priority list + Settings (D6)
Today `Capture::open()` opens the default input (`VOICE/mic.rs:54-61`) and config has only an *output* device (`VOICE/lib.rs:446-447`). New:
```yaml
voice:
  input_devices: ["Shure MV7", "MacBook Pro Microphone"]   # ordered; [] = system default only (today's behaviour)
```
`Capture::open_on(prefs: &[String]) -> (Capture, Receiver<Chunk>, DeviceName)` walks the list in order, skipping names the host does not
enumerate, then the system default, then the first input. Clamshell: the built-in microphone stays in the list but is not special-cased —
a lid-closed MacBook mic opens and delivers near-silence, which §8.3's silent-device detector rejects within 480 ms and the walk moves on
(technique A:L37 does it by display state; ours needs no display query). `voice_status`/`dictation_status` gain `input_device: Option<String>`
(the name that opened). Settings › Voice (W-n) lists the devices (`list_input_devices()` RPC, §14.2) as a reorderable list with "System
default" pinned; `set_input_devices(names)` writes the key. The companions keep their own capture (`VoiceRecorder`'s `AVAudioEngine` input;
the Windows `AskAudio`) and read the same preference from the engine (`voice_status.input_devices`) to pick `AVCaptureDevice` /
`MMDeviceEnumerator` endpoints by name, so one choice serves both paths.

### 8.2 Re-arm on device change (state machine in `VOICE/dictation.rs`, replacing "device lost ends it")
```
Running ──(mic thread sends Err("device lost") — VOICE/mic.rs:160-170)──▶ ReArming{attempt: 1, since}
ReArming ──(open_on(prefs) ok, first PCM within 2 s)──▶ Running   (the utterer keeps its state; the open utterance is closed first: live.rs:249-254)
ReArming ──(open fails)──▶ wait 300 ms, attempt + 1 ──▶ ReArming        (≤ 3 attempts)
ReArming ──(3 failures or 5 s)──▶ Finished{Error("microphone: device lost")}   (today's outcome, later)
```
Events: `DictationDevice { id, device: Option<String>, state: Changed | Lost }` so surfaces show "mic changed → ‹name›" in the level slot
instead of ending; the Composer keeps its region. Several device-list notifications collapse into one re-setup (A:L34). Remote sources are
untouched (their capture is the client's).

### 8.3 Readiness gate + silent-device detection
`Task::run` (`VOICE/dictation.rs:558-610`) reports `Started` only after the **first PCM chunk** arrives; a capture that yields nothing for
`READY_TIMEOUT = 2 s` ends `Error("microphone: no audio from ‹device›")` and tries the next device in the list once (A:L35's gate). A device
that delivers but is dead is detected by `SILENT_WINDOWS = 3` consecutive 160 ms level windows with RMS ≤ 1e-5 and peak ≤ 5e-5 at the start
of a dictation (A:L27's thresholds, reused as constants) → the same next-device step, then `DictationDevice { Lost }` + the pill copy "No sound
from ‹name› — check the input". After the first non-silent window the detector is off (silence mid-dictation is the user).

### 8.4 Standby and prewarm
The model is resident already (54 §0.5, `VOICE/dictation.rs:9-11`); what costs time is the mic open (tens of ms, 54 §0.5) and the first
resampler block. New: after a dictation ends the `Capture` is kept open for `STANDBY = 8 s` (OS indicator stays lit for 8 s — Settings ›
Voice "Keep the mic warm after dictating (8 s)" default on; off = today) so a quick re-press starts at 0 ms; `Prewarm` additionally primes the
resampler for the last-used rate. The companion path is unchanged by this (its mic is its own).

### 8.5 Mic at press in the companion — and how tap/hold still works
The voice key already starts the recorder at the press (`COMP/Companion/PointAndSay/PointController.swift:188-204`, `:359-368`); the 250 ms
wait is the **hold-to-talk** machine's (`AskVoiceMachine.holdThreshold = 0.25`, `COMP/CompanionCore/AskVoice.swift:36`): Ask bar, panel V, row
mic. Change: `AskVoiceMachine` emits `.openMic` on `keyDown` and `.startRecording` (= `dictation_start`) at 250 ms as today; `HoldToTalk` /
`AskBarController` buffer the recorder's chunks in `pending` (exists: `HoldToTalk.swift:32`, `:143`) and flush them after `started`; a `.tap`
(release < 250 ms) → `recorder.cancel()` and the buffer is dropped — no `dictation_start` is ever sent for a tap, so the engine's one slot is
untouched. The voice key's 800 ms discard (`PointMachine.minNoteMs`) is unchanged. Windows twin: `AskVoice.cs` the same two effects; `AskAudio`
buffers. Cost: the OS mic indicator lights on every tap of the panel key; Settings › Dictation "Open the mic on key-down" default on.

### 8.6 Media pause/duck (opt-in)
`voice.duck_media: off | duck | pause` (default off), executed by the companion (the engine's VPIO ducking, `VOICE/mic.rs:233-239`, also
turns on AEC/NS/AGC and would change the dictation audio — not used). `duck`: system output volume −50 % for the dictation
(`kAudioHardwareServiceDeviceProperty_VirtualMainVolume`; Windows `IAudioEndpointVolume`), restored at the end. `pause`: one media
Play/Pause key (`NX_KEYTYPE_PLAY`; `VK_MEDIA_PLAY_PAUSE`) at press and again at the end — macOS offers no public "is something playing", so
the setting is labelled "may un-pause". One reconciler owns the intent (A:L38): a release before the key went out cancels it; an older
dictation's end never resumes a newer one's media.

### 8.7 The metrics (D7)
Recorded by the engine per dictation, relative to `press_ms` which the client passes in `DictationOpts.press_ms` (its monotonic clock
converted: the client sends `press_ago_ms = now − press` at `dictation_start`; the engine stores `press = recv − press_ago_ms`):
`first_pcm_ms`, `first_partial_ms`, `first_final_ms`, `final_ms` (the `DictationFinished` publish). `dictation_status` gains
`last: Option<DictationTimings { surface, press_to_first_pcm_ms, press_to_first_partial_ms, press_to_first_final_ms, stop_to_final_ms,
device }>` and the rows persist in `dictations` (§6.5). Where they show: **Settings › Voice › Debug** row "Last dictation: Composer · mic 42 ms
· first words 310 ms · final 890 ms · stop → final 420 ms" with a "Copy last 20 as CSV" button; the companion's Settings › Dictation shows the
same line for its surfaces; the TUI's `/voice` command prints it. Tests: `dictation::timings_recorded_with_synthetic_feed` (remote tone,
asserts the four fields are monotonic and `first_partial ≥ first_pcm`). No model change (R5).

## 9. Polish

### 9.1 Destinations (D8) — who may be polished
| Destination (`Destination` in `DictationOpts`) | Polish allowed | Default | Where it is set |
|---|---|---|---|
| `AgentPrompt` — Composer, TUI, Ask bar, voice key, command, per-row mic, voice notes `[voice note, N s]` (`VOICE/lib.rs:157-162`, `Composer.tsx:945`) | **never** (the agent reads raw text fine) | — | not configurable |
| `Document` — notes / document editors, the knowledge editor, any `Textarea` whose owner declares it | yes | off | Settings › Voice › Polish › "Documents and notes" |
| `MessageReply` — channel replies the user types into theuth for Slack/Telegram/Signal (the Communication surfaces) | yes | off | "Messages to people" |
| `AnyApp(bundle/exe)` — §13 | per app profile | off; the profile table offers "polish" per app, with a "non-agent apps" group default | companion Settings › Dictation › Apps |
Rewrite mode (§13.6) is a model call by definition and uses the same model and envelope. **"Voice notes" in D8** names the notes/documents
destination; 45's `[voice note]` is an agent prompt in this tree and is never polished (§18 Q3).

### 9.2 `voice.polish_model` and the picker
`voice.polish_model: Option<String>` (model id; `None` = chain below). RPCs `polish_model()` / `set_polish_model(model)` mirror
`title_model` / `set_title_model` (`api.rs:456-459`; the setter persists through `runtime::write_config_value(path, "voice",
"polish_model", id)` as `supervisor.rs:3891-3895` does). Desktop: `ModelCombobox` with none "Chain (title model, then session)" in Settings ›
Voice — the 124/127 component (`docs/design/124-model-picker.md:122`, `127-favorite-models.md:30`), the way Loop.tsx binds the title model
(`UI/components/settings/Loop.tsx:34-41`). TUI: `/model polish`. Companion: the row shows the current name and opens theuth's Settings
(`theuth://settings/voice`), no second picker.

### 9.3 Model chain, guarded in code
`ModelRole::Polish` in `crates/agent/src/loop_/side.rs:37-61`: `slot(&self.polish_model).or_else(|| slot(&self.title_model)).map_or_else(session, Ok)`
— the same shape as `Escalate`'s. The call is `Side::call(model, POLISH_SYSTEM, user_json, POLISH_BUDGET, CallKind::Polish)` (`:83-90`;
`CallKind` gains `Polish`, `crates/runtime/src/side_call.rs:19-40`). Which session's `Side`: the destination's session when it has one (a
document bound to a session, a channel reply's session), else theuth's desk (W-h) — so cost rows land where the user looks.

### 9.4 Prompt shape (the envelope of A:L101, our words)
- **System prompt** (constant `POLISH_SYSTEM`): "You correct a speech transcript for a human reader. Make the smallest edits that turn it into
  readable writing: fix recognition slips, punctuation and casing; keep the speaker's words, order and meaning; never answer, summarise,
  translate or add content; keep code, paths, flags, @mentions and /commands exactly; keep line breaks. Reply with the corrected text only."
  Per-destination profile text is appended from `voice.polish_profiles.<destination>` (short, user-editable; default empty) and, for any-app,
  the `AppProfile.polish_prompt`.
- **User message**: `{"transcript": "…", "destination": "document", "app": "Mail", "title": "Re: deploy"}` — JSON, so quotes, newlines and
  delimiter look-alikes in speech cannot escape the data block; no selected text, no screen.
- **Output**: the reply is trimmed; a reply that is empty, longer than 2× the input, or contains a line starting with `{` or a fenced block
  when the input had none is **refused** (fail closed → the shaped text is delivered). Thinking tags stripped (the `<think>` stripping the
  providers already do for titles applies).

### 9.5 Latency budget and the user's view
`POLISH_BUDGET = 4 s`; the surface shows "polishing…" on the stop glyph and delivers the shaped text if the budget passes (the polished text
then arrives as a history row with "use polished" — never a second paste). Expected on a hosted small model: 0.6–1.5 s for a sentence
[INFERENCE; §8.7's timings gain `polish_ms`]. Polish never runs on partials, never mid-utterance, never for `AgentPrompt`.

### 9.6 Fail closed, undo
No model, no credential, provider error, budget passed, refused output → shaped text, the row's `polished` stays NULL, the debug row says why.
Undo: §6.7 (history row "use the raw text"; the companion's 3 s "⎋ undo polish" hold after a polished delivery).

## 10. Command mode

### 10.1 Key and mode switch while holding (R2, D10)
A third `anywhere` row `commandKey` (§15: ⌃⌥C / Ctrl+Alt+C) drives the same `PointMachine` as the voice key through a `PointController`
instance per mode (`COMP/Companion/PointAndSay/PointController.swift:46` becomes `PointController(mode:)` with `Mode = ask | dictate |
command`). While one controller's gesture is live, a key-down on another mode's chord is routed to the live controller as
`PointMachine.Event.switchMode(Mode)` (new): the state is unchanged, the recording continues, the pill relabels ("Command · theuth desk"),
`VoiceKeyGate` treats the new chord as held too (release of either ends the hold). The §14.4 Swift/C# machines get the event and one test
each. The Tauri shell (no companion) does not get command mode (96 §0.1's ⌃S path stays as it is; `crates/desktop/src/point/machine.rs`
untouched).

### 10.2 The desk session and the command prompt profile (W-l)
The request goes to theuth's desk exactly as the voice key does (`PointController.swift:287-297`: `resolveDesk`, `send_input` or `steer`),
with the user line
```
[command, 4 s] move today's screenshots into Desktop/Shots and make a reminder to review them at six
[screen 1/2 …]            ← only when "Include the screen" is on, as the voice key
```
and the prompt block registered by the `computer` plugin (`crates/computer/src/lib.rs:297-302` gains a third paragraph) tells the model what
the marker means:
> "## Command mode — A user message that starts with `[command, N s]` was spoken to the command key: the user wants an action on this
> computer, not a conversation. Act, then verify, then answer with ONE line that starts with ✓ or ✗ and states the outcome (what changed,
> where). No greetings, no plan prose, no questions unless you cannot proceed. Before a destructive or irreversible step, show the exact
> target in your tool call's `purpose`. Every tool call carries a `purpose` string of the form `checking: …`, `executing: …` or
> `verifying: …` (the card shows it). Prefer the native-app tools (`applescript`, `shortcuts`, `powershell`) for Reminders, Notes, Calendar,
> Messages, Mail and Music; prefer `shell` for files. Stop after the ✓/✗ line."
`purpose` is an optional string every tool call may carry — `ToolCx`/the registry already pass arguments through; the `purpose` key is read
by the card from `ToolCallStarted`'s arguments (the companion decodes the tool name today, `WIN/Theuth.Companion/PointAndSay/PointController.cs:607-610`;
Swift `:495`). No engine change is needed for it beyond the prompt; if a provider strips unknown keys, the card falls back to the tool name.

### 10.3 Step-typed card anatomy (collapsed at the notch / pill; expanded = §12 panel)
```
┌ Command · theuth desk                                ⌃⌥C  ⎋ ┐
│ “move today's screenshots into Desktop/Shots and make a      │
│  reminder to review them at six”                              │
│  checked    ls ~/Desktop/*.png · 14 files                     │  ← purpose "checking: …" (green when its call finished)
│  executed   mkdir -p ~/Desktop/Shots && mv …                  │  ← "executing: …"
│  verifying  ls ~/Desktop/Shots | wc -l            ▒▒▒▒ shimmer│  ← the running row
│  next       applescript · Reminders “Review shots” 18:00      │  ← a call started, not yet returned
│ ⎋ stop · hold ⌃⌥C to add                                      │
└──────────────────────────────────────────────────────────────┘
```
Rows come from `ToolCallStarted` (type from the `purpose` prefix, else "ran"), turn green/red on `ToolCallFinished` (ok / error output), the
text is `purpose` after the colon (≤ 60 chars, tool name as fallback). Thinking text is not shown (the shimmer is); the final assistant text
is the ✓/✗ line. Done:
```
┌ Command · theuth desk · done · 6 s                      O  ⎋ ┐
│ ✓ 14 screenshots moved to ~/Desktop/Shots; reminder          │
│   “Review shots” set for 18:00.                               │
│ 4 steps  ls · mkdir/mv · ls · applescript                     │
│ O open session · ⎋ dismiss · hold ⌃⌥C follow up               │
└──────────────────────────────────────────────────────────────┘
```
A reply that does not start with ✓/✗ is shown as is with the status "replied" (the model ignored the profile; nothing breaks).

### 10.4 Turn cap (W-e) and failure
`COMMAND_MAX_STEPS = 12` tool rounds per request, counted by the card from `ToolCallStarted`; at 12 the companion sends `cancel(desk,
"root")` (`api.rs:254-255`; `CancelScope::Root`, `crates/agent/src/lib.rs:392-398`) and the card freezes with "stopped after 12 steps — hold ⌃⌥C to continue" (a hold continues the thread: the next `[command]` line
says "continue"). `TurnFinished { stop_reason: "error" | "cancelled" }` → "✗ couldn't finish — ‹message›" in the failed colour; an approval
denied by timeout → the row says "denied (no answer in N s)" and the model's own ✗ line follows. The card never auto-hides in a failed state
(D13).

### 10.5 Follow-ups
Hold the command key while the card is up → the new words go to the same desk (`steer` if the turn runs) with `[command, N s]`; the card
appends a new request block under the previous ✓ line (session dots in the header show the thread position, §12). Typing into the card's
follow-up field (expanded) sends through `send_input` with the same marker.

### 10.6 Approvals in the card + spoken approval grammar
An `ApprovalRequested` for the desk session (the stream the companion already subscribes to for cards) renders inside the command card:
```
│ ⚠ Run `rm -rf ~/Develop/theuth/build` ?   destructive · policy: ask │
│   checked  build/ exists · 1.2 GB · 3 days old                       │
│   [ Yes ⏎ ]  [ No ⎋ ]      or say “yes” / “no”                       │
```
Keys: ⏎ → `resolve_approval(id, Allow)`, ⎋ → `Deny` (`api.rs:640-641`); the Attention panel's own approval card keeps working (same id;
`true` only for the first answer). **Voice:** while the box is up the companion opens a short dictation (`dictation_start` with
`Surface::Companion`, `end_commands` off) and matches the first final utterance against the grammar `yes | yeah | go | go ahead | do it |
approve` → Allow, `no | nope | stop | cancel | deny` → Deny, anything else → the words are ignored and the box stays ("didn't catch that —
⏎ or ⎋"); 8 s with no match closes the listen (the keys still work). Near-miss tolerance as §7.1 for phrases ≥ 5 chars. Talk (145 D6,
`docs/design/145-talk-presence.md:46`) keeps announcing approvals without answering them — only the command card listens, and only while
its box is shown (§18 Q4). The risk policy itself is unchanged (`tools.policy`, `crates/tools/src/lib.rs:487-495`).

### 10.7 Windows twin
`PointController.cs` gains the mode and the `SwitchMode` event; the step rows render in the island's voice view (`island.SetVoice(View(...))`,
`WIN/Theuth.Companion/PointAndSay/PointController.cs:419`) and in the §12 panel; the approval box uses the existing `Decision = Allow | Deny`
path of the Attention panel (134 D5); the spoken-approval dictation is the same `dictation_start` call; chords in §15.

## 11. Native-app tools

### 11.1 Tool signatures (`crates/computer/src/native.rs`, registered beside `tools::all`, `crates/computer/src/tools.rs:394-407`)
```rust
/// macOS: run AppleScript source (`osascript -l AppleScript`), 30 s default timeout.
pub struct AppleScriptArgs { pub script: String, #[serde(default)] pub timeout_secs: Option<u64> }
computer_tool!(AppleScriptTool, "applescript", Risk::Mutating, AppleScriptArgs,
  "Run an AppleScript on this Mac (Reminders, Notes, Calendar, Messages, Mail, Music, Finder, System Events). Return value and errors come back as text. Mutating: it can send messages and change data — state the target in `purpose`.", …);
/// macOS: run a named Shortcut (`shortcuts run <name> [--input-path f]`), text input, text/file output.
pub struct ShortcutsArgs { pub name: String, #[serde(default)] pub input: Option<String>, #[serde(default)] pub timeout_secs: Option<u64> }
computer_tool!(ShortcutsTool, "shortcuts", Risk::Mutating, ShortcutsArgs, "Run a Shortcut by name; `shortcuts list` is read-only through `applescript`-free `name: \"__list__\"`. …", …);
/// Windows: PowerShell 7 (`pwsh -NoProfile -NonInteractive -Command`), else Windows PowerShell; 30 s default.
pub struct PowerShellArgs { pub script: String, #[serde(default)] pub timeout_secs: Option<u64> }
computer_tool!(PowerShellTool, "powershell", Risk::Mutating, PowerShellArgs, "Run PowerShell on this PC (Outlook/Calendar via COM, Notepad, Explorer, Windows Settings). …", …);
```
Registered per platform (`cfg(target_os)`): macOS gets `applescript` + `shortcuts`, Windows `powershell`, Linux none. `timeout_secs` capped
at 120; output = stdout + stderr, truncated at 16 KiB with "…(N more bytes)"; exit code ≠ 0 → `ToolOutput::err`. The process runs detached from
the engine's stdin, inherits the user's environment, no shell wrapper (`osascript`/`shortcuts`/`pwsh` are exec'd directly with argument
vectors — no string interpolation into a shell).

### 11.2 Risk class and policy
`Risk::Mutating` for all three (`crates/tools/src/lib.rs:118-125`): they change state in apps but are scoped to what the script names; the
default policy row for `mutating` applies (`PolicyAction::Ask` by default, `:444-453`) — so the first run asks, in the command card (§10.6).
A script containing `do shell script`, `rm `, `Remove-Item -Recurse`, `Format-`, `Stop-Process`, `shutdown`, `osascript -e 'tell application
"System Events" to keystroke'` is re-classed **Destructive** before the policy check (a small, visible list in `native.rs` — a *reclassifier*,
not a deny-list; the policy still decides). `tools.gate`-style per-tool override is the existing `mcp/config.yaml` mechanism's sibling:
`computer.native: off | mutating | destructive` in the `computer:` section turns them off or raises them.

### 11.3 Output shape
`ToolOutput::ok(text)` with `text` = the script's return value (AppleScript's result line; Shortcuts' output; PowerShell's stdout), followed
by a trailer line `exit 0 · 1.2 s` the model can quote; errors `ToolOutput::err("osascript: ‹stderr line›")`. Nothing structured: the recipe
skill tells the model to make scripts return one line.

### 11.4 The recipe skill (`skills/native-apps/SKILL.md`, loaded by the desk; techniques only — the recipes are written from Apple's and
Microsoft's documented dictionaries, not from A:L143's prompt)
Sections: Reminders (create in a list, due date, list today's), Notes (new note with body, append to a note by name, search), Calendar
(create event with alarm, list today's), Messages (send iMessage to a contact — Destructive by the reclassifier because it sends), Mail
(compose draft, send, search subject), Music (play/pause, play a playlist, now playing), Finder/System (reveal path, open folder, set
clipboard), Shortcuts (run by name, pass text), Windows: Outlook COM (`New-Object -ComObject Outlook.Application` — mail draft, appointment,
task), Windows Settings (`Start-Process ms-settings:`), Explorer, Notepad. Each recipe: the one-line goal, the script, what it returns, the
verify step (read back), the pitfalls (Reminders' list indexing, Calendar's time zone, Mail's draft vs send).

### 11.5 Tests
`native::registers_per_platform` (macOS 2, Windows 1, Linux 0); `native::reclassifier_marks_destructive_patterns`;
`native::timeout_kills_the_child` (a `delay 10` script with `timeout_secs: 1` → err "timed out after 1 s"); `native::output_truncated_at_16k`;
`native::exec_without_shell` (an argument containing `; echo pwned` is passed verbatim to `osascript` and comes back as a syntax error, not
executed); a macOS-only `#[ignore]` live test `applescript_returns_result_line` (`return "ok"`); Windows `powershell_returns_stdout`. Policy
integration: `tools::policy_asks_for_mutating_native_tool` over the registry test harness.

## 12. The answer surface

### 12.1 One AnswerCard (D12)
Today: the voice key's `AnswerCardView` — plain `Text(state.text).lineLimit(12)`, "O open the desk · ⎋ dismiss"
(`COMP/Companion/PointAndSay/AnswerCard.swift:24-56`) — and the Ask bar's receipt row with `firstTwoLines` (`COMP/CompanionCore/AskRouter.swift:118-127`).
New: one `AnswerCard` (the class name stays; `AnswerCardState` grows) used by the Ask bar (replacing the receipt row's *display*; the
`Receipt` model and `activity_list` merge stay for history), the voice key and command mode. One instance in the Store (`store.answer`), one
generation counter, one panel.

### 12.2 Anatomy
Collapsed (today's two-line receipt, 440 pt wide as `AnswerCard.width`, `AnswerCard.swift:88`):
```
┌ theuth desk · replied just now                              🔈  ⎋ ┐
│ The failing test is `vad::gate_onset`; the 160 ms onset frame      │
│ count changed in vad.rs:112 …                                      │
│ ⏎ expand · O open · ⎋                                               │
└────────────────────────────────────────────────────────────────────┘
```
Expanded (⏎, click, or any scroll; 440 × ≤ 70 % of the screen's visible height, scrolls inside):
```
┌ theuth desk · 2 of 3  ● ● ○                                 🔈  ⎋ ┐
│ “why does the onset test fail”                                      │
│ The failing test is `vad::gate_onset`. The onset run is now 8       │
│ frames (160 ms) but the fixture feeds 7.                             │
│  • fix the fixture → vad.rs:112                                      │
│  • or keep 7 and lower ONSET_MS to 140                               │
│ Recommend the first; it matches design 54 §0.3.                      │
│ [read_file] [code_symbol] [cargo test -p voice]          ← tools, as they ran │
│ ┌ follow up… ───────────────────────────────────────────┐  ⏎        │
│ └───────────────────────────────────────────────────────┘           │
│ hold ⌃⇧Space to follow up · or type ⏎ · O open · ⎋                  │
└────────────────────────────────────────────────────────────────────┘
```
- **Markdown**: SwiftUI `Text(AttributedString(markdown:))` for inline marks, plus a tiny block renderer for lists, headings and fenced code
  (monospace block, horizontal scroll) — the Windows twin uses the `MarkdownTextBlock` of the Community Toolkit already referenced by 134, or a
  `RichTextBlock` fallback. Streaming: the text re-renders per `TextDelta` with the caret of the last line shimmering.
- **Tool names** as chips in the order they started; a running one pulses; clicking a chip opens the desk session at that call (`theuth://c/<desk>`).
- **Follow-up in place**: the field sends `send_input`/`steer` with the same marker family as the surface that opened the card (none for Ask,
  `[voice note, N s]` for the voice key's hold, `[command, N s]` for command); the card appends a new question block and the dots advance.
- **Session list**: the header dots are the *thread* positions (requests in this card since it opened); ◀ ▶ / J K move; the list view (⌘L)
  shows the last 20 answers from history (`activity_list` 24 h, as the receipts do today, `COMP/Companion/AskBar/Store+Ask.swift:183-187`).
- **Collapsed form = the receipt's two lines** (`firstTwoLines` reused), then the key caps.

### 12.3 Expand notch ↔ panel and back
The card is one `KeyPanel` (`GlassPanel.make`, `AnswerCard.swift:84-86`) that **resizes** between the two anatomies with a 180 ms animation;
it does not move surfaces: at the notch (the island's voice view, 96 §0.3) the collapsed form is the island's content, and expanding detaches
it into the panel *at the pointer's screen* (D14) — the island returns to its meeting content or hides (as 149 §5.4 D step 6). Collapse (⏎
again, or ⎋ once while expanded) folds back to the collapsed card at the same place; the island is not re-entered (one surface at a time;
technique A:L183, our placement). Windows: the card is a `ToolWindow` sibling of the island; the island keeps today's voice view for the
collapsed form (`PointController.cs:419`), the expanded form opens the window.

### 12.4 State honesty (D13) — the table every state passes
| State | Enter on | Shows | Copy | Leaves on | Never |
|---|---|---|---|---|---|
| listening | press | rec dot, words | "· ⌃S held · theuth's desk" | release/lock/Esc | — |
| finishing | release | muted words, concealed overlays | "finishing…" | final text (cap 8 s, `PointController.swift:47-48`) / "Nothing recognized" | hides before the text is in |
| thinking | `TurnStarted` | shimmer "thinking", tool chips as they start, elapsed s | "thinking · code_refs · 4 s" | `TurnFinished`/error | looks frozen (shimmer + elapsed tick every second) |
| answer | first `TextDelta`/`TurnFinished` | streamed text | "replied" | 6 s `doneMs` **only if** collapsed and not speaking | vanishes while speaking or expanded |
| speaking | `Speaker.surface == ours` (149s) | glyph | "click to stop" | speech end / Esc / new gesture | — |
| failed | `TurnFinished { error\|cancelled }`, send error | failed colour | "couldn't reply — ‹why›" | ⎋ only | auto-hides |
**Stop freezes**: Esc during thinking sends `cancel(desk, "root")` and the card shows the frozen partial answer with "stopped" — it does not
disappear. **Hide is generation-checked**: `AnswerCard.hide(generation:)` ignores a hide whose generation is older than the latest `begin`
(a new gesture while a 6 s timer runs), the rule the 149f fix applied to `dictation_start` replies (`docs/design/149-as-built-voice-key-fix.md:90-94`).
Nothing vanishes before the work is done: `doneMs` arms only after `TurnFinished` *and* speech ended *and* collapsed.

### 12.5 Pointer-screen presentation (D14)
Already true for the pill and the card (`PointController.swift:200`, `:306`; `AnswerCard.swift:118`, `:162-163`). Extend to the island-hosted
voice view: when the pointer's screen is not the island's (`IslandPanel.swift:156` picks the notched screen), the voice mode renders in a
second lightweight `VoicePill` at the pointer and the island stays with its meeting content (W2's pre-W3 fallback host, `VoiceModeView.swift:6-7`,
kept as the multi-display path). Windows: `Monitor.cs` already resolves the monitor under the cursor for overlays; the voice view follows it.

### 12.6 "Automatic" hotkey option and modifier-only keys
Settings › Voice key (and › Dictation, › Command) gain a gesture picker: **Hold · double-tap locks** (default, W-i) · **Automatic** — tap < 0.4 s
toggles (press again to stop), longer = push-to-talk; no double-tap lock in this mode (technique A:L170). `PointMachine` gets `Gesture = .holdLock |
.automatic` with two more transitions (`armed → recordingToggled` on release < 400 ms; `recordingToggled → sending` on the next press) and the
ported tests. **Modifier-only keys incl. fn (macOS)**: a chord with no key (`Chord.keyCode == nil`, `Modifiers.function` exists,
`COMP/CompanionCore/KeyBindings.swift:7-15`) cannot be a Carbon hot key; the companion installs an `NSEvent` global `flagsChanged` monitor and
tracks physical key codes per modifier group (fn 63; ⌘ 55/54, ⌥ 58/61, ⌃ 59/62, ⇧ 56/60 — A:L171) so left/right are distinct; the row is enabled
only after macOS grants Input Monitoring [INFERENCE: a global key monitor needs `kTCCServiceListenEvent`; §18 Q1], with the hint "set
System Settings › Keyboard › Press 🌐 key to: Do Nothing". Chord protection: another key during the hold → the release is ignored (A:L172).
The recorder's `check` gains `.needsInputMonitoring` beside `.reservedByMacOS` (`KeyBindings.swift:318-335`). Windows: `RegisterHotKey`
cannot bind a modifier alone; a low-level keyboard hook (`WH_KEYBOARD_LL`) is the twin, opt-in the same way (no permission dialog on Windows).

### 12.7 Windows twin
`AnswerCardWindow : ToolWindow` (new) with the two anatomies; the island voice view = collapsed form; `MarkdownTextBlock`; the same
generation-checked hide; `AskRouter.FirstTwoLines` (`WIN/Theuth.Companion.Core/AskRouter.cs:168-192`) reused; the Automatic gesture in
`PointMachine.cs`; the hook-based modifier-only option.

## 13. Any-app dictation

### 13.1 Target assessment (three-way; refuse only when certain)
At the press (before the mic is live) the companion reads the focused element: macOS `AXUIElementCreateSystemWide` → `kAXFocusedUIElement`
(`crates/computer/src/ax.rs` already hit-tests and reads roles for the pointer, `:736`, `:126`; the companion does its own read in Swift
because the engine's AX grant is theuth.app's, 149 §9); Windows `IUIAutomation.GetFocusedElement` (`ax.rs:163`'s `uia_role` mapping reused
for the names).
| Verdict | macOS roles / attributes | Windows UIA | Then |
|---|---|---|---|
| editable | `AXTextField`, `AXTextArea`, `AXComboBox`, `AXSearchField`; or `kAXValue` settable (`AXUIElementIsAttributeSettable`) | `ControlType.Edit`/`Document` with `ValuePattern` not read-only or `TextPattern` | direct insertion (W-m) when `kAXSelectedText` is settable and the read-back confirms; else lease |
| unknown | `AXGroup`, `AXWebArea`, `AXWindow`, `AXScrollArea`, `AXUnknown`, no focused element readable (GPU terminals, VMs, Electron web views, games) | `Pane`, `Custom`, `Document` without patterns, elevated windows (UIPI) | clipboard lease + verification (paste works where AX does not) |
| certainly not | `AXButton`, `AXMenu*`, `AXStaticText`, `AXSlider`, `AXCheckBox`, `AXRadioButton`, `AXImage`, `AXToolbar`, `AXTable` row | `Button`, `MenuItem`, `Text`, `Slider`, `CheckBox`, `Image`, `ListItem` | refuse: hint "Nothing to type into — ‹role›" 2.5 s, no recording (the words are not lost because none were spoken) |
Secure: `AXSecureTextField` / UIA `IsPassword` → editable, delivered by lease, **never Return** (§7.4). The focused element is captured once
(`pid`, element ref, window id); every delivery re-checks focus and goes `paused` if it moved (§3.2).

### 13.2 Direct insertion (W-m)
Only on `editable`: write `kAXSelectedText` (replaces the selection or inserts at the caret), then read `kAXValue`/`kAXNumberOfCharacters`
back within 150 ms; a mismatch (some apps accept the write and ignore it) falls through to the lease for *this and every later utterance* of
the gesture. Windows: `ValuePattern.SetValue` is a whole-value replace and is **not** used for insertion; `TextPattern` has no insert — so
Windows direct insertion = `SendInput` with `KEYEVENTF_UNICODE` for ≤ 200 chars per utterance (chunked), else lease. No CGEvent typing on
macOS (CGEvent unicode posting needs the Accessibility grant too and is slower than a paste for a sentence — A:L113 uses it first for
terminals; our terminals are "unknown" and take the lease).

### 13.3 Clipboard lease (sequence; technique A:L117-119)
```
1 snapshot    every NSPasteboardItem type → data (cap 32 MiB per representation; larger → the restore keeps only the text types)   | Win: OpenClipboard, EnumClipboardFormats, GetClipboardData per format (CF_UNICODETEXT, CF_HDROP, CF_DIB, registered formats by name)
2 write       one item: public.utf8-plain-text + org.nspasteboard.TransientType + .ConcealedType + .AutoGeneratedType + io.theuth.dictation-session = <gesture id> | Win: CF_UNICODETEXT + "ExcludeClipboardContentFromMonitorProcessing" + "CanIncludeInClipboardHistory"=0 + "CanUploadToCloudClipboard"=0 + a registered "theuth.dictation-session" format
3 paste       CGEvent ⌘V to the captured pid (post to the app's event tap, not the session) after releasing our own physical modifiers | Win: SendInput Ctrl+V to the foreground thread; our hotkey modifiers released first (keybd_event up for Ctrl/Alt)
4 return      the caller continues (next utterance may queue; deliveries are serialised through one lease slot)
5 settle      at 500 ms: if the pasteboard still holds our session id → restore the snapshot (changeCount re-read first); else (the user copied) do nothing | same by the registered format
6 verify      §13.4 in parallel with 5
```
The user's clipboard is back within 0.5 s; clipboard managers skip the item (the nspasteboard conventions); a paste that lands after 500 ms
pastes the *restored* clipboard — the verification catches that as "not landed" and the card offers the words. Setting "Leave the
dictated text on the clipboard" (default off) skips 5.

### 13.4 Paste verification
Reads `kAXValue` (≤ 60 000 chars), `kAXNumberOfCharacters` and `kAXSelectedTextRange` before the paste and at **150 / 350 / 1000 ms**
after; focus moved → `unknown` (no card); user typed meanwhile → `userActed` (no card); all three unchanged at 1000 ms → **notLanded**
(technique A:L128). Windows: `TextPattern.DocumentRange.GetText(60000)` + `GetSelection`; elements without `TextPattern` → `unknown`. Direct
insertions verify once at 150 ms (that is their read-back). Only lease deliveries run the full triple and only while the setting "Check that
the text landed" is on (default on).

### 13.5 The failure card
```
┌ Couldn't type into Terminal · paste not landed                   ⎋ ┐
│ git rebase -i HEAD~3 then force push with lease                    │
│ [ Copy ]  [ Try again ]  [ Open in theuth ]                         │
│ checked at 150 · 350 · 1000 ms — field unchanged · clipboard restored │
└────────────────────────────────────────────────────────────────────┘
```
Kinds (typed, logged, counted in the debug row): `noEditableTarget`, `accessibilityNotTrusted`, `clipboardSnapshotFailed`,
`clipboardWriteFailed`, `pasteCommandFailed`, `targetUnavailable` (app quit), `restoreFailed`, `pasteNotLanded`. Copy puts the shaped text on
the clipboard *without* markers; Try again re-runs the lease once focus is back in the same element; Open in theuth puts the text in the
Composer of theuth's desk (`theuth://compose?text=`). The card stays until acted on (D13); the words are also in history (§6.5).

### 13.6 Rewrite on the selection
Entry: the dictate-anywhere key while the focused element reports a non-empty `kAXSelectedText` (fallback: `kAXSelectedTextRange` + value
slice; Windows `TextPattern.GetSelection`) → the pill says "Rewrite · 312 chars selected · say what to change". The spoken instruction is the
**user message**; the selection goes into the **system prompt** through a template (`voice.rewrite_template`, default "The user selected the
following text in {app}; apply their spoken instruction to it and reply with the replacement text only:\n\n{selection}"); the model is
§9's chain (`ModelRole::Polish`), the call `CallKind::Polish` on theuth's desk; multi-turn: holding again while the panel is up refines
("shorter", "more formal") with the previous reply as the assistant turn. The panel shows the proposal; **Replace** restores focus to the
captured element first (hiding alone races the paste — A:L165), then delivers through §13.2/13.3 so the selection is replaced; ⎋ discards.
Per-app profile may pin a tone (`AppProfile.rewrite_prompt`). Rewrite is never automatic and never runs on an `AgentPrompt` surface.

### 13.7 Permissions
macOS: **Accessibility** for the companion (`AXIsProcessTrustedWithOptions` prompt once; theuth.app's grant does not cover the login item —
149 §9's rule, `docs/design/149-companion-native.md:922-924`) — without it: lease-only delivery works (paste needs no grant), assessment and
verification are `unknown`, the pill says "Accessibility off — pasting blind", Settings › Dictation shows the grant button. Input Monitoring
only for §12.6's modifier-only keys. Windows: UIA needs no grant; UIPI blocks reads/`SendInput` into elevated windows → `unknown` and the
lease may fail with `pasteCommandFailed` ("elevated window") — the card says so.

### 13.8 What we refuse
Certainly-not targets (§13.1); our own windows (the companion's panels, theuth.app's Composer — which has its own Ctrl+D; focus in them →
hint "Use Ctrl+D here"); a target whose pid died mid-gesture; Return into secure fields; any delivery while the screen is locked
(`CGSessionCopyCurrentDictionary` `CGSSessionScreenIsLocked` / `WTS_SESSION_LOCK`); text > 20 000 chars in one gesture (the words go to the
card instead). Never: reading the target's full value for anything but verification; sending the target's content anywhere (the rewrite
selection goes only to the chosen model, and the history row stores our text, not the app's).

### 13.9 Windows twin (summary of the per-step twins above)
UIA `GetFocusedElement` + `ControlType`/`ValuePattern`/`TextPattern` for assessment and verification; `SendInput` `KEYEVENTF_UNICODE` for
direct insertion ≤ 200 chars and `Ctrl+V` for the lease; clipboard formats `CF_UNICODETEXT` + the three Windows clipboard-history hints +
a registered session format; `RegisterHotKey` for the key (§15); the failure card and rewrite panel as `ToolWindow`s; `WTS_SESSION_LOCK`
for the lock refusal. The Core project gets the pure parts (`DeliveryAssessment.cs`, `ClipboardLease.cs` state, `PasteVerifier.cs`
decision) with tests mirroring the Swift ones (§16).

## 14. Engine and RPC changes

Everything additive and serde-defaulted: a client that never sends the new params or reads the new fields behaves as today. FROZEN — a
wave that needs a change amends §18 first.

### 14.1 Rust types in `crates/voice` (ts-rs exported where marked; mirrors in §14.4)
```rust
// VOICE/shape/mod.rs — §4.1 verbatim: ShapeConfig, Shaped, EndCommand, Action, Replaced, RuleId, EndPhrases, Shaper.
// VOICE/lib.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)] #[ts]
#[serde(rename_all = "snake_case")]
pub enum Surface { Composer, Tui, Companion, Phone, AnyApp }
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)] #[ts]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Destination { AgentPrompt, Document, MessageReply, AnyApp { app: String } }
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)] #[ts]
#[serde(default)]
pub struct DictationOpts { pub surface: Option<Surface>, pub destination: Option<Destination>,
    /// §13's per-app row for shaping (terminal spacing, commands); the engine never persists it.
    pub app: Option<AppProfile>,
    /// Slash commands the surface accepts (stage 4); empty = none recognised.
    pub commands: Vec<String>,
    /// §8.7: `now − press` on the caller's clock at the moment of the call.
    pub press_ago_ms: Option<u32> }
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)] #[ts]
pub struct AppProfile { pub id: String, pub name: String, pub terminal: bool, pub polish: bool, pub learn: bool,
                        pub polish_prompt: Option<String>, pub rewrite_prompt: Option<String>, pub commands: Vec<String> }
// VoiceEvent::DictationText gains (serde default, additive):
//   shaped: String, command: Option<EndCommand>, actions: Vec<Action>
// New VoiceEvent variants:
//   DictationDevice { id: DictationId, device: Option<String>, state: DeviceState }      // §8.2  DeviceState = Changed | Lost
//   DictionarySuggestion { id: SuggestionId, heard: String, typed: String, occurrences: u32, surface: Surface, has_audio: bool }
// DictationState (VOICE/dictation.rs:103-110) gains: input_device: Option<String>, last: Option<DictationTimings>
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)] #[ts]
pub struct DictationTimings { pub surface: Surface, pub press_to_first_pcm_ms: Option<u32>, pub press_to_first_partial_ms: Option<u32>,
    pub press_to_first_final_ms: Option<u32>, pub stop_to_final_ms: Option<u32>, pub polish_ms: Option<u32>, pub device: Option<String> }
// VOICE/dictionary/mod.rs — §5 verbatim: Dictionary, Entry, Scope, Variant, Negative, Match, matcher constants.
// VOICE/history/mod.rs — §6: HistoryRow, Correction, SuggestionAnswer { Add, NotNow, Never }, learn::{candidates, due}.
// VoiceConfig (VOICE/lib.rs:424-448) gains, all #[serde(default)]:
//   shape: ShapePresets { composer, tui, companion, phone, any_app: Preset }   Preset = Full | Plain | Off
//   end_commands: EndPhrases, input_devices: Vec<String>, standby_secs: u64 (8), duck_media: Duck (Off|Duck|Pause),
//   polish_model: Option<String>, polish_profiles: BTreeMap<String, String>, rewrite_template: Option<String>,
//   history: HistoryConfig { enabled: bool (true), rows: u32 (200), audio_mib: u32 (64), learn: bool (true) }
// Actor messages (VOICE/dictation.rs): StartDictation { source, opts: DictationOpts }; new Correction { id, sent_text },
//   SuggestionAnswer { id, answer }, DictionaryOp (§5.6), Timings; the Task carries ShaperOwned + a history writer handle.
```

### 14.2 RPC methods and events (`api.rs`; positional params in order; every field named)
| Method | Params | Reply | Notes |
|---|---|---|---|
| `dictation_start` (changed) | `sample_rate: Option<u32>`, **`opts: Option<DictationOpts>`** (trailing optional) | `DictationId` | `None` opts = today (`api.rs:1585-1586`, `:6272-6297`) |
| `dictation_correction` (new) | `id: DictationId`, `sent_text: String` | `()` | §6.1; any authenticated peer (text only, the phone included) |
| `dictation_history` (new) | `limit: Option<u32>` | `Vec<HistoryRow>` | local peers only (as the daemon mic, `:6279-6283`) |
| `dictation_history_forget` (new) | `id: Option<DictationId>` | `()` | one row, or everything when `None` |
| `dictation_audio_get` (new) | `id: DictationId`, `utterance: Option<u32>` | `String` (base64 i16 16 kHz) | local peers only; `Err("no audio")` when evicted |
| `dictionary_list` (new) | `scope: Option<Scope>` | `Vec<Entry>` | user + learned; the packaged set is reported as a count + version |
| `dictionary_put` (new) | `entry: Entry` | `Entry` | upsert by canonical + scope |
| `dictionary_delete` (new) | `id: EntryId` | `bool` | |
| `dictionary_suggestion_answer` (new) | `id: SuggestionId`, `answer: SuggestionAnswer` | `()` | §6.3 step 4 |
| `list_input_devices` (new) | — | `Vec<InputDevice { name, default: bool }>` | §8.1 |
| `set_input_devices` (new) | `names: Vec<String>` | `()` | writes `voice.input_devices` |
| `set_shape` (new) | `surface: Surface`, `preset: Preset` | `()` | `write_nested_config_value(.., "voice", "shape", surface, preset)` |
| `set_end_phrases` (new) | `phrases: EndPhrases` | `()` | |
| `polish_model` / `set_polish_model` (new) | — / `model: Option<String>` | `Option<ModelInfo>` | §9.2, mirrors `title_model` (`api.rs:456-459`) |
| `polish` (new) | `text: String`, `destination: Destination`, `session: Option<SessionId>` | `PolishReply { text: Option<String>, model: Option<String>, ms: u32, refused: Option<String> }` | the companion's any-app and rewrite path; `Agents` side call (W-h) |
| `rewrite` (new) | `instruction: String`, `selection: String`, `app: AppProfile`, `thread: Option<Vec<(String, String)>>` | `PolishReply` | §13.6 |
| `dictation_status` (changed reply) | — | `DictationState + input_device + last` | §8.7 |
| `voice_status` (changed reply) | — | `+ input_devices: Vec<String>` | §8.1 |
Events on `subscribe_events`: `DictationText` (+ `shaped`, `command`, `actions`), `DictationDevice`, `DictionarySuggestion`. Agent side:
`Agents` gains `Polish { text, destination, session: Option<SessionId> }` and `ModelRole::Polish`; `CallKind::Polish`
(`crates/runtime/src/side_call.rs:19-40`). Tools: `applescript`, `shortcuts`, `powershell` registered by `crates/computer` (§11); the
`computer` prompt block gains the command paragraph (§10.2).

### 14.3 Settings keys (engine `config.yaml`; written by the RPCs above)
`voice.shape.{enabled,composer,tui,companion,phone,any_app}` (`enabled`: 201's global off switch, `set_shape_enabled`), `voice.stt_engine` (`onnx` | `mlx`, 202 §5, `set_stt_engine`), `voice.end_commands.{send,scratch,literal}`, `voice.input_devices`,
`voice.standby_secs`, `voice.duck_media`, `voice.polish_model`, `voice.polish_profiles.<destination>`, `voice.rewrite_template`,
`voice.history.{enabled,rows,audio_mib,learn}`, `computer.native`. Companion-local (`UserDefaults` / the C# settings store, the
`VoiceKeySettings` pattern, `COMP/Companion/PointAndSay/PointController.swift:529-538`): `dictateAnywhereEnabled`, `commandKeyEnabled`,
`gesture` (holdLock | automatic) per mode, `micOnKeyDown`, `appProfiles: [AppProfile]`, `leaveTextOnClipboard`, `checkPasteLanded`,
`modifierOnlyKeys`. Desktop-local: nothing new beyond the Settings › Voice tab reading the engine.

### 14.4 Companion ↔ engine messages (Swift and C# mirrors, hand-written `Codable`/records pinned by fixtures exported with `just ts-desktop`, 130 D11)
| Shape | Swift (`COMP/TheuthKit/Models.swift`) | C# (`WIN/Theuth.Companion.Rpc`) |
|---|---|---|
| `DictationOpts`, `Surface`, `Destination`, `AppProfile` | `struct DictationOpts: Codable` … | `record DictationOpts(...)` |
| `DictationText` + `shaped/command/actions` | `DictationWire` gains `shaped: String?`, `command: EndCommandWire?`, `actions: [String]?` (`COMP/Companion/AskBar/Store+Ask.swift:148-158`) | `StreamDelta`/the dictation decoder gains the same optionals |
| `DictationDevice`, `DictionarySuggestion` | new `Decodable`s routed in `dictationObserve` (`:161-175`) | new cases in `OnEvent` (`WIN/Theuth.Companion/PointAndSay/PointController.cs:592`) |
| `DictationTimings` | `struct DictationTimings: Codable` | record |
| `PolishReply`, `Entry`, `HistoryRow` | structs | records |
| RPC wrappers | `RpcClient.dictationStart(sampleRate:opts:)`, `.dictationCorrection`, `.polish`, `.rewrite`, `.dictionarySuggestionAnswer`, `.dictationHistory`, `.dictationAudioGet` (`COMP/CompanionCore/AskRpc.swift` pattern) | `RpcClient.149.cs` siblings |
Pure machines (tested on the Mac for both platforms): `PointMachine` + `switchMode` + `Gesture.automatic`; `AskVoiceMachine` + `.openMic`;
`DeliveryAssessment` (roles → verdict); `ClipboardLease` (snapshot/write/paste/settle generations); `PasteVerifier` (three reads → verdict);
`SpokenApproval` (grammar → `Decision?`); `CommandCard` reducer (events → rows); `AnswerCardState` + generation.

### 14.5 TS bindings touched (`just ts-desktop`)
`VoiceEvent.ts` (new fields/variants), `DictationState.ts`, new `DictationOpts.ts`, `Surface.ts`, `Destination.ts`, `AppProfile.ts`,
`DictationTimings.ts`, `EndCommand.ts`, `Action.ts`, `Entry.ts`, `HistoryRow.ts`, `PolishReply.ts`, `CallKind.ts` (+ `polish`);
`UI/src/rpc/api.ts` gains the methods above beside `dictation_*` (`UI/src/rpc/api.ts:358-367`). `UI/lib/dictation.ts`: `Utt` gains
`shaped` and `command` (`:16-17`); `applyEvent` stores both (`:71-77`); `splice` uses `shaped` (`:43-53`); `start()` passes `opts`
(`:272`); a `correction` report at send; the `DictionarySuggestion` listener. `Composer.tsx`: "send it" → `submit()`; the suggestion card.
`components/settings/Voice.tsx` (new tab) and `SettingsDialog.tsx:66` gains the row.

### 14.6 File ownership per crate (one owner per file per wave; §16 has the waves)
| Crate / dir | Files | Wave |
|---|---|---|
| `crates/voice` | `src/shape/*`, `src/dictionary/*`, `src/history/*`, `migrations/*`, `data/dictionary.v1.json.gz` + `dictionary.json` + `data/THIRD_PARTY.md` (§5), `src/dictation.rs` (opts, shaper, timings, re-arm, standby), `src/mic.rs` (`open_on`, device list), `src/lib.rs` (types, config); new deps `sqlx` (workspace), `rphonetic` (Apache-2.0, new), `strsim` (MIT, in the lock already), `fst` (optional) | A |
| `crates/runtime` | `src/side_call.rs` (`CallKind::Polish`) | A |
| `crates/agent` | `src/loop_/side.rs` (`ModelRole::Polish`), `src/supervisor.rs` (`Polish`, `polish_model` slot + setter) | A |
| `crates/rpc` | `src/api.rs` (methods/events above), bindings | A |
| `crates/computer` | `src/native.rs`, `src/lib.rs` (registration + prompt paragraph) | C |
| `crates/desktop/ui` | `lib/dictation.ts`, `components/chat/Composer.tsx`, `components/ui/{dictation,DictionarySuggestion}.tsx`, `components/settings/{Voice,SettingsDialog}.tsx`, `rpc/api.ts` | B |
| `crates/tui` | `src/dictation.rs`, `src/ui.rs` (shaped text, send it, suggestion line, `/voice`) | B |
| `crates/desktop/companion` | `TheuthKit/Models.swift`, `CompanionCore/{AskVoice,PointMachine,KeyBindings,DeliveryAssessment,ClipboardLease,PasteVerifier,SpokenApproval,CommandCard}.swift`, `Companion/{PointAndSay/*,AnyApp/*,AskBar/*,Voice/*,Settings/*}` | B (fields/suggestion), C (card/command/keys), D (any-app) |
| `crates/desktop/companion-windows` | the twins of each Swift file, `Hotkeys.cs`, `AnswerCardWindow.cs`, `AnyApp/*.cs` | B/C/D twins |
| `skills/native-apps` | `SKILL.md` + recipes | C |

## 15. The shared key table

One `KeyTable` on both platforms (`COMP/CompanionCore/KeyBindings.swift:193-245`, `platformDefaults` `:251-257`; `WIN/Theuth.Companion.Core/KeyBindings.cs:335`).
Rows marked ★ are new in 153; 149 §8's rows are repeated so the table is whole (`docs/design/149-companion-native.md:876-913`).

| Scope | Action | macOS default / Windows | Notes |
|---|---|---|---|
| anywhere | Open the panel · hold to talk | ⌃⇧Space / Ctrl+Shift+Space | unchanged |
| anywhere | Open the Ask bar | unbound | unchanged |
| anywhere | Open the agents panel | ⌃⇧A / Ctrl+Shift+A | unchanged |
| anywhere | Voice key — point and say (ask) | ⌃S / Ctrl+Alt+Space | unchanged (`KeyBindings.swift:218`, `:253-255`) |
| anywhere ★ | Dictate anywhere | **⌃⌥D / Ctrl+Alt+D** (W-d) | hold/lock as the voice key; Automatic option per §12.6 |
| anywhere ★ | Command | **⌃⌥C / Ctrl+Alt+C** (W-d) | same gesture; while another mode's gesture is live this key switches it |
| anywhere | Meeting push-to-talk · Meeting eye | ⌥` / Alt+` · ⌥⇧E / Alt+Shift+E | unchanged, only while a meeting is live |
| panel · askBar · agents · island | as 149 §8 | | unchanged |
| pointAndSay | ⎋ cancel | fixed | registered between press and send; the 149g held-⎋ twin (`KeyBindings.swift:270-275`) applies to the two new chords (⌃⌥⎋ for both) |
| pointAndSay ★ | ⏎ expand/collapse the card · O open · J/K thread · ⌘L list | fixed in the card | the card is key (`AnswerCard.swift:84-86`) |
| pointAndSay ★ | ⏎ approve · ⎋ deny (approval box up) | fixed | §10.6; the box's keys take precedence over expand/dismiss while shown |
| settings ★ | Gesture: Hold · double-tap locks / Automatic | per mode | §12.6 |
| settings ★ | Modifier-only chord (incl. fn) | opt-in, needs Input Monitoring / `WH_KEYBOARD_LL` | §12.6 |

**Collisions checked.**
- macOS ⌃⌥D: no entry in `CopySymbolicHotKeys` (⌘⌥D is Dock hide; ⌃D is EOF/forward-delete — 96 §0.1's reason to avoid it,
  `docs/design/96-point-and-say.md:29-45`; ⌃⌥D is neither); Cocoa text views bind no ⌃⌥ letter; terminals with "option as meta" see ⌃⌥D as
  ESC + ⌃D only when the hot key is *not* registered — once registered Carbon swallows it everywhere (the 96 trade-off, accepted). ⌃⌥C: no
  system default; not a Cocoa binding; Xcode/VS Code bind ⌘⌥C/⌃⌥C only in user keymaps (rebindable in Settings › Keys as every row).
- Save/Find/Spotlight: ⌘S/⌘F/⌘Space untouched (no ⌘ in any new chord). Office: Word's ⌃⌥C = © autocorrect symbol on Windows (Ctrl+Alt+C)
  and ⌃⌥D? none; on macOS Office uses ⌘-chords — no clash.
- **Windows AltGr** (W-d): on layouts where right-Alt is AltGr, `Ctrl+Alt+D` / `Ctrl+Alt+C` are AltGr+D / AltGr+C — US-International types
  `ð` / `©`, Icelandic `ð`, Polish AltGr+C = `ć`, Czech/Slovak `AltGr+C` = `&`. At registration the companion probes the active layout with
  `ToUnicodeEx(VK_D, …, keystate Ctrl+Alt)`: a character result → the row is registered as **Ctrl+Shift+Alt+D / +C** instead and the
  Keys tab shows "AltGr on this layout — using Ctrl+Shift+Alt+D"; the probe re-runs on `WM_INPUTLANGCHANGE`. `MOD_NOREPEAT` stays
  (`WIN/Theuth.Companion/Hotkeys.cs:11`, `:199`). Ctrl+Alt+Space (voice key) has no AltGr glyph on any layout checked (space).
- Same-scope uniqueness: the persisted-table test extends to the two rows (149 §8 collision policy); the recorder's `check` refuses the
  voice key's chord for the new rows and vice versa (`KeyBindings.swift:318-335`).
- The Tauri shell's Settings › Shortcuts (`UI/lib/shortcuts.ts:50-54`) gains no new rows: dictate-anywhere and command exist only with a
  companion (D1 lives there); the `voice-key` row's "read-only while a companion is alive" rule (149 §8) is unchanged.

## 16. Waves, ownership, proof list, must-not-regress

Branch `integrate/153` off main once this design lands; each wave a worktree off it; machine rules as 149 §11 (one build at a time under the
shared lock, separate target dirs, no model-loading test while Sky is at the keyboard, `docs/design/149-companion-native.md:1019-1024`).
**PIN rule** per wave: the first commit is *types + a no-op handler* (every new field `serde(default)`, every new RPC answering `Ok(empty)`,
bindings exported, fixtures pinned) and is PINned to the integration branch the same hour so sibling waves compile against it; behaviour
follows in later commits; a wave lands on main only feature-complete (149 D24).

| Wave | Scope | Owner crate set / files (§14.6) | Windows twin |
|---|---|---|---|
| **A engine** | §4 shape, §5 dictionary + packaging, §7 end commands, §8.1–8.4 + 8.7 capture/timings, §6.5 history store + §6.1–6.3 learn (engine side), §9.2–9.3 polish model/chain/RPC, §14 types | `crates/voice`, `crates/runtime` (`CallKind`), `crates/agent` (role + setter), `crates/rpc`, bindings | none (engine is one `t.exe`) |
| **B surfaces** | shaped text + "send it" + suggestion card + history/Voice settings tab in the Composer, TUI, companion fields, phone; polish opt-in per destination; `dictation_correction` at send | `crates/desktop/ui`, `crates/tui`, `COMP/Companion/{AskBar,Voice,Settings}`, `COMP/CompanionCore/AskVoice.swift` | `WIN/…/AskBar`, `Voice/RowTalk.cs`, `AskVoice.cs`, Settings |
| **C companion** | §10 command mode (+ §11 tools + skill), §12 one AnswerCard + state honesty + pointer screen + Automatic + modifier-only keys, §8.5 mic at key-down, §15 keys | `COMP/Companion/PointAndSay/*`, `COMP/CompanionCore/{PointMachine,KeyBindings,SpokenApproval,CommandCard}.swift`, `crates/computer/src/native.rs`, `skills/native-apps` | `PointController.cs`, `PointMachine.cs`, `KeyBindings.cs`, `Hotkeys.cs`, `AnswerCardWindow.cs` |
| **D any-app** | §13 assessment, lease, verification, failure card, rewrite, app profiles, §8.6 duck/pause; §6.6 stays optional after D | `COMP/Companion/AnyApp/*`, `COMP/CompanionCore/{DeliveryAssessment,ClipboardLease,PasteVerifier}.swift` | `WIN/…/AnyApp/*.cs`, Core twins |
A → B and A → C may run in parallel after A's PIN; C waits for `fix/149-island` (§2.3); D after C's card lands (its failure card and
rewrite panel reuse it).

### 16.1 Proof list — P probe · A assertion · T test (each row is a gate for its wave)
| # | Wave | Kind | What proves it |
|---|---|---|---|
| 1 | A | T | `shape::examples_table_rows_1_to_35` green (§4.3) |
| 2 | A | T | `shape::shape_is_total_over_fuzzed_utf8` 10 k cases, no panic |
| 3 | A | T | `dictionary::packaged_loads_and_hashes` — the data file's sha256 equals `dictionary.json`'s record (§5.2) |
| 4 | A | T | `dictionary::match_thresholds` — known 0.70 / new 0.85 / negative veto / self-expansion guard (§5.4) |
| 5 | A | T | `end::*` six tests + `dictation::send_it_settle_then_silence_stops` (§7.5) |
| 6 | A | T | `dictation::device_lost_rearms_within_5s` — synthetic mic feed sends `Err("device lost")`, a second feed opens, `DictationDevice { Changed }` published, no `Finished` |
| 7 | A | T | `dictation::silent_device_rejected_in_480ms` and `readiness_gate_2s` |
| 8 | A | T | `dictation::timings_recorded_with_synthetic_feed` (§8.7) |
| 9 | A | T | `history::budget_evicts_audio_first_then_rows`; `learn::candidates_single_word_non_inflection`; `learn::due_after_2_in_7_days` |
| 10 | A | A | `cargo test -p voice -p agent -p rpc`, `cargo clippy -p voice`, bindings diff shows only the §14.5 files |
| 11 | A | P | `t --headless` + the existing RPC harness: `dictation_start(16000, {surface: "tui"})` with a synthetic tone → events carry `shaped`; `dictation_start(16000, null)` unchanged |
| 12 | B | T | `lib/dictation.test.ts`: `applyEvent` stores `shaped`; `splice` uses it; "send it" → `submit` once (`Composer.test.ts`) |
| 13 | B | T | `crates/tui` splice tests over `shaped`; Swift `AskVoiceMachineTests.sendItSkipsTheHold`; C# twin |
| 14 | B | P | Composer live: say "ship it send it" → message sent with "ship it"; say "foo scratch that bar" → "bar"; Settings › Voice debug row shows four numbers |
| 15 | B | P | correction loop live: dictate "parakeet tee dee tee", edit to "Parakeet TDT", send, twice → the card; Add → next dictation shows "Parakeet TDT" |
| 16 | C | T | `PointMachineTests`: 17 ported + `switchModeKeepsRecording`, `automaticTapTogglesHoldTalks`, `automaticNoDoubleTapLock` (Swift and C#) |
| 17 | C | T | `SpokenApprovalTests`: yes/go/no/near-miss/unknown; `CommandCardTests`: rows from `ToolCallStarted` with and without `purpose`, cap at 12, ✓/✗ parse |
| 18 | C | T | `AnswerCardStateTests`: hide ignored when generation stale; `doneMs` arms only collapsed ∧ not speaking ∧ finished |
| 19 | C | T | `native::*` (§11.5) incl. `exec_without_shell`; `policy_asks_for_mutating_native_tool` |
| 20 | C | P | `--probe-command` (new, `--shareable`, never acting): a recorded desk sequence with `purpose` strings → PNGs of heard/working/approval/done at `/tmp/153-command-*.png`; `--probe-card <collapsed|expanded|failed>` → PNGs |
| 21 | C | P | live: ⌃⌥C "create a reminder to test at six" → steps, approval in the card answered by voice "yes", one ✓ line; ⌃S → the same card anatomy; mode switch ⌃S → ⌃⌥C mid-hold relabels and the desk gets `[command]` |
| 22 | C | A | `KeyTableTests.persistedTableUnique` over the two new rows; Windows `RegisterHotKey` for Ctrl+Alt+D/C logs `registered` on a US layout and `AltGr … Ctrl+Shift+Alt+D` on a US-International layout (the builder's live round) |
| 23 | D | T | `DeliveryAssessmentTests` (every role in §13.1), `ClipboardLeaseTests` (settle only if still ours; stale generation no-op), `PasteVerifierTests` (focus moved → unknown; user acted; all unchanged → notLanded) — Swift and C# |
| 24 | D | P | `--probe-anyapp` (TextEdit scripted open, never a real user app): dictate a synthetic tone → text appears, clipboard restored within 600 ms, verification `landed`; then a `NSButton` focused → "Nothing to type into — button"; a secure field → no Return |
| 25 | D | P | live: Mail compose, Terminal (lease), Chrome address bar, a password field; "send it" in Mail presses Return, in the password field does not; failure card on a target that swallows ⌘V (an app with paste disabled) |
| 26 | D | A | the debug row's delivery counters after 20 gestures: `pasteNotLanded ≤ 1`, `clipboard restored = 20` |
| 27 | all | A | every as-built names the probe PNGs and the live check lines, as 149 §10's gates (`docs/design/149-companion-native.md:1012-1015`) |

### 16.2 Must-not-regress
- The 149 gesture tests (17 `PointMachine` cases, `VoiceKeyGateTests` 5, `AskVoiceMachine`, `DictationLease`) stay green, unmodified.
- Dictation into our own fields is **unchanged by default**: `dictation_start(rate, null)` → `Surface::Composer`, `Full` preset — the shaped
  text equals the raw text for plain prose (table rows 1/3/6/10/20/24 pin it); Ctrl+D / Esc / the undo step (`Composer.tsx:493-505`) unchanged.
- The meeting island: no change to `IslandPanel.swift` placement/visibility; a live meeting still refuses dictation with "Meeting running —
  use the island to ask" (`VOICE/dictation.rs:235-237` "talking"); the island's voice view is still hosted by the island on its own screen.
- Esc semantics: Composer Esc discards (`Composer.tsx:448-457`); companion ⎋ cancels between press and send and undoes the hold
  (`KeyBindings.swift:211`, `:244`); the new "Esc freezes the card" applies only to a card in `thinking`.
- One dictation per daemon (`"already dictating"`), relay-peer mic refusal (`api.rs:6279-6283`), `dictation_stop` idempotent, the 149f
  lifecycle (a start landing after its gesture is stopped at once).
- Spoken replies stay opt-in (149s S1); the card's speaking glyph and Esc-stops rule unchanged.
- No new TCC prompt in any automated run; Accessibility and Input Monitoring prompts appear only from Settings buttons.

## 17. Rejected

| Rejected | Why | What would reopen it |
|---|---|---|
| Project/computer dictionary seeding (D18) | R4: the algorithm's source · rate · trigger · veto are undecided; a bad seed pollutes every surface | a one-page algorithm answering the four questions (§5.7) that Sky approves |
| LLM polish before agent prompts (D18) | the agent reads raw transcripts fine (`VOICE/lib.rs:170-179`); A:L55 shows the 5 s+ cost | never |
| A one-tool command loop (D18) | the desk already has eight `computer_*` tools, shell, browser, MCP and a risk policy (`crates/computer/src/tools.rs:394-407`) | never |
| WebRTC APM on dictation (D18) | FluidVoice itself uses it only for meetings (A:L24); VPIO exists in-crate for talk | a measured echo problem in dictation |
| CTC keyword boosting (D18) | parakeet-rs exposes no vocabulary input (`VOICE/stt.rs:255-259`) | a parakeet-rs API for it — then as an *addition* to the dictionary, not a replacement |
| A closed local LLM at the centre (D18) | theuth has models and the agent | never |
| A new STT model path (R5, D18) | first partials are structurally fast (§2.4); measure first | §8.7's numbers showing press → first partial > 600 ms median on the Composer |
| Shaping only on finals | partials would show "send it" and symbol words that then jump; clients already replace per utterance | — (W-a) |
| Direct CGEvent/AX typing as the primary any-app path | AX writes are ignored by many apps; the lease + verification is what their users' complaints taught (A:L120, A:L128) | — (W-m) |
| A separate command session | the desk's context (projects, memory) is the point; a marker + prompt block suffices (W-l) | the ✓/✗ discipline failing in practice → a per-turn system-prompt profile RPC |
| Keeping dictation audio out of history (54 §12, `docs/design/54-dictation.md:573-575`) | D9 needs it for grounding and playback; budgeted, local, switchable | — (amends 54 §12 and `dictation.rs:7`) |
| Spoken approvals in talk | 145 D6 announces only (`docs/design/145-talk-presence.md:46`); the command card is a visible box with an explicit listen window | Sky asking for it in talk |

## 18. Open questions and amendments

| # | Question for Sky | Recommended default |
|---|---|---|
| Q1 | Modifier-only keys incl. fn need a global key monitor and, most likely, the Input Monitoring grant for the companion [INFERENCE]. Accept the extra permission (opt-in row, disabled until granted)? | yes, opt-in; ship without it if the grant proves not to cover `flagsChanged` |
| Q2 | Chords ⌃⌥D / ⌃⌥C (Ctrl+Alt+D / Ctrl+Alt+C with the AltGr fallback) — fine, or do you want ⌃⇧-family chords? | keep W-d |
| Q3 | D8 lists "voice notes" as polishable, but 45's `[voice note]` is an agent prompt in the tree (`VOICE/lib.rs:157-162`). Read "voice notes" as notes/documents? | yes — agent prompts are never polished |
| Q4 | Spoken "yes"/"no" answers approvals in the command card (D10) while talk only announces them (145 D6). Keep the split? | yes; the card is a visible box with an 8 s listen window |
| Q5 | History keeps audio (54 §12 rejected it): 200 rows / 64 MiB / 120 s grounding, switchable. OK? | yes, default on, "Keep audio" switch in Settings › Voice |
| Q6 | "Automatic" gesture default off (hold / double-tap lock stays)? | yes (W-i) |
| Q7 | The command turn cap 12; the approval listen window 8 s; polish budget 4 s — constants to tune after the first week? | yes; they are `pub const`s with one home each |
| Q8 | Polish and rewrite are charged to theuth's desk session when the destination has none (W-h). OK, or a dedicated "voice" session row? | the desk |
| Q9 | 149 ISLAND FIX (main `46960464`, landed while 153 was written) pins pill + card to the island area for the whole gesture. Keep that (island area of the display under the pointer at press, fixed for the gesture) or let 153's card follow the pointer? | keep the island-area rule; choose the display at press |

### Amendment log
- 2026-10-09 v0 — written. Amends: 54 §0.10/§12 (audio may be kept under D9's budget; `dictation.rs:7`'s header changes with wave A);
  96 §0.6 (modifier-only chords become an opt-in companion option, §12.6); 149 §8 (two new anywhere rows, §15); 149 §3.7/D14 (the
  AnswerCard grows into §12's card); the study's §2.4 latency inference (0.7 s → ≈0.24 s structural floor, both [INFERENCE] until §8.7 measures).
- 2026-10-09 17:35 (orchestrator, before landing) — main moved to `46960464` (149 ISLAND FIX) while this was written: its item P pins the
  point-and-say dictation pill and the answer card to the **island area for the whole gesture** instead of following the pointer, and D1/D2
  make the island open while a meeting is live with no hover model. §12's "pointer-screen presentation" and §2.3's "card and pill already
  follow the pointer" must be read against that landing: wave C presents the card/pill in the island area of the display under the pointer
  **at press** (per-display island memory, 149 D3) and does not move it mid-gesture. Q9 below records the fork.
- 2026-10-09 wave A as built — see `153-as-built-A.md`. Section-level amendments: §5.4 packaged entries match on the normalised form only
  (fuzzy at `ACCEPT_NEW` over 40k names rewrote prose: and → wand, plan → Plane; fuzzy kept for user 0.70 / learned 0.85); §5.5 user layer
  in `<data_dir>/voice/dictionary.json`, not `voice.db`; §6.5 `dictations.utterance_ends` (per-utterance byte offsets in the `.pcm16`
  file); §6.3 `Store::correction` returns `Suggestion`s with ids (`occurrences`, `surface`, `has_audio`), `Store::answer` takes
  `&mut Dictionary` and writes the learned entry / negative itself; §7.3 `SETTLE_MS` / `SILENCE_MS` live in `voice::shape`; §8.2 re-arm
  every 500 ms for 5 s (not 300 ms × 3); §8.3 readiness / silent-device gate is mic-only; §8.4 resampler priming skipped; §8.6 duck/pause
  deferred to the companion; §9 `polish(session: None)` is charged to theuth's desk, polish reply length rule 0.3×–2× + 8 chars; §14.1
  actor message is `AnswerSuggestion`, `VoiceStatus` ships without a `dictionary` field, `Shaper` gained builders
  `with_surface` / `with_commands` / `with_mentions` and `shape_with(&mut EndParser, …)` (the parser is the actor's, one per utterance);
  §4.2 gates read TLD / extension lists from the packaged `Path` pairs on top of the built-in lists (the talon lists lack `dev`, `io`, `rs`).
- 2026-10-09 waves B+C as built — see `153-as-built-BC.md`. Section-level amendments: §7.4 Ask "send it" skips the undo hold (`.sendNow` →
  `commitAsk`; Composer/TUI `submit()` once at `DictationFinished` with a final `command: send`); §8.5 the mic opens at key-down with chunks
  buffered until `started`, a tap (< 250 ms) discards them and never starts a dictation; §10.3 `purpose` is a top-level key of every tool
  call's `args` (explicit on `applescript`/`shortcuts`/`powershell`, `**extra` kwarg elsewhere) with `checking:`/`executing:`/`verifying:`
  prefixes, ≤ 60 chars, fallback `reason` → tool name; §10.4 the 12th `ToolCallStarted` freezes the card and cancels; §10.6 the approval
  listen is an 8 s companion dictation reading `text` only (no per-call end-command switch); §11.2 risk reclassifier and `computer.native`
  policy not built (all three native tools `Risk::Mutating`, default `Ask`); §12.2 voice follow-up = hold the mode key, typed marker
  `[command, 1 s]`; §12.4 `Phase.speaking` dropped — `speaking` is a parallel flag; §12.6 modifier-only chords = settings row + grant
  button only; §4.1 `Shaped.replaced` is not on the wire (fields show `shaped`, fallback `text` only when no end command rode the piece);
  §14.3 no getter for the shape preset / end phrases (surfaces remember the last written value); §14.4 `DictationWire` is the public
  `TheuthKit` mirror; §6.4 auto-dismiss sends no RPC and ▶ plays `dictation_audio_get(id, null)`.
- 2026-10-09 F1 — see `153-as-built-A.md` "F1". §14.3 `voice_settings()` getter added (`VoiceSettings`: shape, end_commands,
  input_devices, standby_secs, duck_media, polish_model, polish_profiles, history), so surfaces read the presets / end phrases back
  instead of remembering the last written value; §14.5 the voice dictionary's `Kind` / `Source` export to TS as `EntryKind` /
  `EntrySource` (the bare names collided with the artifact `Kind` and knowledge `Source` bindings).
- 2026-10-10 F2 — see `153-as-built-A.md` "F2" (Windows live finding: the words before `dictation_stop` were lost without ≈2.5 s of tail).
  §14.1 `StopDictation` feeds the audio already pushed (queued chunks + the sub-chunk remainder, ≤ 5 s) through the model before the close
  and refuses later `PushAudio`; the §7.3 settle drains the same way. A deeper decoder flush in `finish()` (2–6 zero windows) was measured
  and rejected: the pad + one zero window already flushes the tail (one zero window ≈ 57 ms), more perturbed the decoder.
