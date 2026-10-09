# 153 — as built: W153C, the Windows twin of wave C (command mode, one AnswerCard, keys)

Branch `feat/153w-c` (worktree `~/Develop/theuth-153w-c`), cut from `integrate/153w` = main `6aadda9a`, PIN-B `4fcbcf1e` merged.
PIN-C `1e9f40b9` (sent 20:02). Final: see the report (`~/.theuth/briefs/2026-10-09/153-windows/reports/W153C.md`).
Design: `docs/design/153-voice-anywhere.md` §10 (10.7), §12 (12.7), §15, §16.1 rows 16–18, 20–22. Aligned with the Mac twin's
hand-offs H3-CC (557eb083), H3-C1 (76fbe448) and H3-C2 (1fa78c3b).

## What was built

### Pure machines — `Theuth.Companion.Core` (tested on the Mac)
| File | What |
|---|---|
| `PointMachine.cs` | `PointMode` (Ask · Dictate · Command), `Gesture` (HoldLock · Automatic), `PointEvent.SwitchMode` / `PointEffect.SwitchMode` (state unchanged, recording continues — §10.1), `PointState.Toggled` (§12.6 `recordingToggled`), Automatic transitions (tap < 400 ms toggles, hold ≥ 400 ms talks, no Tap1 → no double-tap lock, `AutomaticHint`), `followUp` press during Sending (§10.5/§12.2), `HoldThreshold`, `PointModeText`, and the PIN-C record `ModeGesture` (Started · Locked · Stopped · Discarded · Switched · Refused). `Step`'s new parameters are optional: the 17 ported tests are unchanged. |
| `KeyBindings.cs` | `KeyAction.DictateAnywhere` (Ctrl+Alt+D) and `KeyAction.Command` (Ctrl+Alt+C) as anywhere rows (persisted ids `dictateAnywhere` / `command`, H3-CC); the gesture family (`ModeKeys`, `ModeOf`, `ActionOf`) and `VerdictKind.ModeKeyClash` (no family row takes another's chord, no "Use here instead"); modifier-only chords (`Chord.IsModifierOnly`, L/R key names, `VerdictKind.NeedsModifierOnly`); `AltGrRule` (probe answer → chord to register + "AltGr on this layout — using Ctrl+Shift+Alt+D"); `ModeKeySettings` (`gesture.<mode>`, `dictateAnywhereEnabled`, `commandKeyEnabled`, `modifierOnlyKeys`, `commandIncludeScreen`). |
| `KeyGrammar.cs` | `CardKeys` / `CardKeyIntent` / `CardKeyContext`: ⏎ expand/collapse · O open · J/K (←/→) thread · Ctrl+L list · ⎋, and with the approval box up ⏎ approve · ⎋ deny first; ⎋ order deny → stop speaking → stop (thinking) → collapse → dismiss. |
| `SpokenApproval.cs` (new) | §10.6 grammar → `Decision?` (`Decide`, H3-CC name), near-miss `max(1, len/5)` for phrases ≥ 5 chars, one-polarity safety rule; `ApprovalListen` (8 s window, "didn't catch that — Enter or Esc"). |
| `CommandCard.cs` (new) | The reducer `Observe(state, input) → (state, effects)`; `ParsePurpose`, `ParseVerdict`, `Summary`, `StatusCopy`, `Header`, `FailedLine`, `ContinueText`, `CommandText` / `ComposeBody` (`[command, N s] …`), `FromEvent` (the engine's `ToolCallStarted` args.purpose → args.reason → tool name, `ToolCallFinished`, `ApprovalRequested/Resolved`, `TextDelta`, `TurnFinished`, `Error`). Cap: the 12th start → `CancelTurn("cap")` + "stopped after 12 steps — hold … to continue" (H3-C1). |
| `AnswerCardState.cs` (new) | The one card: phases listening · finishing · thinking · answer · failed, `Speaking` as a parallel flag (H3-C1), `Generation` + generation-checked `Hide`, `DoneArmed` = visible ∧ answer ∧ finished ∧ ¬speaking ∧ ¬expanded ∧ ¬approval open, thread (J/K), `Stop` (Esc freezes with "stopped"), §12.4 copy, `CollapsedText` = `AskReply.FirstTwoLines`; `AnswerMarkdown` (headings, paragraphs, bullets, numbered, fenced code, inline code/bold/italic/links). |

### The app — `Theuth.Companion` (built on build-B)
| File | What |
|---|---|
| `Hotkeys.cs` | The two rows register with `MOD_NOREPEAT`; every exact Ctrl+Alt+typing-key anywhere chord is probed with `ToUnicodeEx(vk, keystate Ctrl+Alt, flags 0x4)` on the foreground thread's layout → the Ctrl+Shift+Alt fallback + notice + `AltGr …` log line; re-probe on `WM_INPUTLANGCHANGE` (host subclass) and a 2 s look at the foreground layout (re-registers only when an answer changes, never while a key is held); per-key release watches (either key-up ends a switched hold); `WH_KEYBOARD_LL` for modifier-only rows with chord protection (`Chorded`); `ModeGate`; PIN-C events `DictateAnywhere` / `CommandKey`, `TakeDictateWords`, `HasDictateAnywhereSubscriber`, `EffectiveChord`, `AltGrNotices`. |
| `PointAndSay/PointController.cs` | ONE machine for three mode keys: the press picks the mode, another mode's key while live is `SwitchMode` (ask ↔ command keep the recording; into/out of dictation the words are handed over), each key's gesture from Settings, follow-up holds (steer); dictate gestures go to `Hotkeys.DictateAnywhere`; command sends `[command, N s] …` (+ `[screen …]` only with Command › Include the screen) and drives `CommandCard` (rows, cap → `cancel(desk, root)`, approvals → `resolve_approval` by click / Esc / Enter ≥ 500 ms / voice over a short `dictation_start` with Surface Companion); every answer is `AnswerCardState` — collapsed in the island's voice view, detached into `AnswerCardWindow` on click/scroll/approval; Esc while thinking freezes; the 6 s timer only when `DoneArmed`; speaking from the shared Speaker. |
| `PointAndSay/PointProbe.cs` | `--probe-command`, `--probe-states`, `--probe-card <collapsed\|expanded\|failed>`, `--probe-switch` (never acting; PNGs of both surfaces to `%TEMP%\153-*.png`). |
| `Island/IslandController.cs` | `ModeKeys` / `SaveModeKeys` / `ModeKeysChanged`. |
| `Island/IslandWindow.cs` | `VoiceView` gains title, step rows, approval box (Yes/No buttons → `VoiceAction`), failed tone, footer; tap/scroll → `VoiceTapped`; `CapturePng`. |
| `Windows/AnswerCardWindow.cs` (new) | `ToolWindow` with both anatomies (440 × ≤ 70 % of the display, scrolls inside), island-area placement on the display under the pointer at `Begin` (§18 Q9), generation-checked `Hide`, card keys, follow-up field, chips, steps, approval box, actions; `XamlPng`. |
| `Settings/ShortcutRecorder.cs` | `ModeKeyClash` / `NeedsModifierOnly` verdicts, lone L/R modifier recording; under each mode row: the AltGr line, the row's switch, the gesture picker, Command's "Include the screen", the voice key's "Modifier-only keys". **No line in `SettingsWindow.cs`.** |
| `w153c-gate.ps1` (new) | PNG list/base64, `langs intl\|restore` (proof 22), `layout-probe`, SendInput key tests. |

Tests: `PointMachineTests` (+3: `SwitchModeKeepsRecording`, `AutomaticTapTogglesHoldTalks`, `AutomaticNoDoubleTapLock`), `KeyTable153Tests.cs` (`KeyTableTests153`: 7 incl. `PersistedTableUnique`, `RecorderRefusesTheVoiceKeysChordForTheNewRowsAndViceVersa`, `AltGrDecisionFromTheLayoutProbe`), `SpokenApprovalTests.cs` (Yes/Go/No/NearMiss/Unknown theories + listen window), `CommandCardTests.cs` (8 incl. `RowsFromToolCallStartedWithAndWithoutPurpose`, `CapAt12`, `VerdictParse`, `CommandMarkerLine`), `AnswerCardStateTests.cs` (5 incl. `HideIgnoredWhenGenerationStale`, `DoneMsArmsOnlyCollapsedNotSpeakingFinished`, `StateHonestyTableEveryRow`).

## How it was verified (numbers)
- Mac, `dotnet test tests/Theuth.Companion.Core.Tests`: **367 / 367** (baseline 310 + 57 new; the 17 ported PointMachine tests unchanged and green). `tests/Theuth.Companion.Rpc.Tests`: see the report (W153B's PIN-B tests on this branch).
- build-B `dotnet build Theuth.Companion.slnx -c Debug -p:Platform=x64`: **0 errors, 0 warnings** (29 s) at `882e9467`; the final sha's build is in the report.
- Live round on build-B (desk lock 20:03–20:12, one 1536×864 virtual display, installed engine without a model credential):
  - US layout (`0x04090409`, Ctrl+Alt+D/C type nothing): `hotkey dictateAnywhere = Ctrl+Alt+D registered (RegisterHotKey, MOD_NOREPEAT)`, `hotkey command = Ctrl+Alt+C registered`, each after `AltGr probe … → no glyph`; the voice key still recovers from the shell's hold by the 149 retry.
  - **Proof 22**: language list switched to en-US / US-International only (`0409:00020409`, layout `0xF0010409`; Ctrl+Alt+D types ð, +C types ©) → `hotkey dictateAnywhere: AltGr on this layout — using Ctrl+Shift+Alt+D (Ctrl+Alt+D types 'ð' …)`, `hotkey command: AltGr on this layout — using Ctrl+Shift+Alt+C`, `… = Ctrl+Alt+Shift+D registered`; Settings › Keys (UIA): `settings-keys-altgr-dictateAnywhere` = "AltGr on this layout — using Ctrl+Shift+Alt+D", `settings-keys-altgr-command` = "… +C", top notice "Dictate anywhere: AltGr on this layout — using Ctrl+Shift+Alt+D · Command: …"; list restored to en-US `0409:00000409`, no override.
  - **SwitchMode while holding**, real keys (windows-mcp key_down/key_up): `WM_HOTKEY Ctrl+Alt+Space (voiceKey)` → holding → `WM_HOTKEY Ctrl+Alt+C (command)` → `--SwitchMode(command)--> {"state":"holding"}` "the recording continues; held=voiceKey+command" → C up → `--Released--> sending fx=Stop mode=command` → nothing recognized → idle, `dictation_status … active=none`. The same with `--probe-switch` (simulated keys).
  - **§12.4 every state** (`--probe-states`, fixtures through the real `AnswerCardState`, island view and window): listening, finishing, thinking (clock 0 s → 5 s), thinking expanded, answer (island), answer collapsed in the window, answer expanded, speaking (island + expanded), failed (island + expanded), stopped.
  - **Proof 20** (`--probe-command`, recorded desk events through `Store.HandleEvent` → the real reducer; never acting): listening ("Ctrl+Alt+Shift+C held" — the effective AltGr chord), heard ("heard → working"), working (rows checked/executed/checking/executing with purposes), approval in the card ("Run `rmdir /s /q …`? destructive · policy: ask · Yes Enter · No Esc · or say “yes” / “no”"), done ("✓ Reminder … removed", "O open session · Esc dismiss · hold … follow up"), done expanded.
  - Restored: installed companion running, HKCU Run → `%LOCALAPPDATA%\theuth\companion\Theuth.Companion.exe --background`.
- Evidence: `~/.theuth/briefs/2026-10-09/153-windows/evidence/W153C/` — `live-*.log` (log extracts), `live-keys-tab-uia.txt`, `png/153-*.png` (XAML renders of both surfaces per state; `153-state-*`, `153-command-*`; the island renders are opaque, the card-window renders of round 1 carry the window's transparent tree — the Mica backdrop is not part of the XAML — so `png/on-dark/*-card.png` are the same renders composited over the dark theme's solid base #202020, which is what the opaque capture in `adfa20b8` paints), `png/w153c-keys-altgr.png` (GDI desktop shot with Settings › Keys open on US-International).

## Deviations (with reasons)
1. **Markdown**: `MarkdownTextBlock` (Community Toolkit) is not referenced by the 134 build (adding it pulls more of the Windows App SDK than the trimmed set) → §12.2's fallback: a pure parser (`AnswerMarkdown`, tested) rendered into TextBlocks (selectable, monospace code blocks that scroll sideways).
2. **Dictation's capture is W153D's** (COMMON.md seam): the dictate key's gesture goes out on `Hotkeys.DictateAnywhere`; a switch into dictation hands the words heard so far (`Switched.Words`) and frees the mic and the engine's one dictation; out of dictation the AnyApp controller returns its words (`TakeDictateWords`) and this controller records the rest. Ask ↔ command keep one recording; a switch across the dictation boundary restarts the engine dictation once (the capture changes owner).
3. **⏎ approves only when the card window is key, and only ≥ 500 ms after the box appeared**; ⎋ denies through the global Esc registration; clicks and voice always work. Windows has no non-activating key panel and a global Enter would steal Return from every app. When an approval arrives the card detaches into the window and asks for the foreground (Windows may refuse).
4. **Expanding the collapsed card** is a click or a scroll on the island's voice view (the island is non-activating, ⏎ cannot reach it); ⏎ toggles once the card window is key.
5. **Ctrl+L (⌘L list)** opens the desk session in theuth; the 24 h history list stays the Ask bar's receipts (W153B's files). Open item.
6. **H3-C1 alignments**: the cap fires on the 12th start (design: "at 12"); purpose fallback `reason`, then the tool name; speaking is a flag, not a `CardPhase`; doneMs also waits for no open approval; Command's screen is its own setting (default off) — a checkbox under the Command row in Settings › Keys (there is no Settings › Command page on Windows); the pointer trail shows only when the screen goes with the request.
7. **Family refusal** covers all three family rows mutually (the design names voice key ↔ new rows; Dictate ↔ Command sharing a chord would break the switch too).
8. **AltGr** applies to every exact Ctrl+Alt+typing-key anywhere row (the voice key's Ctrl+Alt+Space is never affected: AltGr+Space types a space); the notice uses the design's words "Ctrl+Shift+Alt+D" while the registration line prints Windows' order "Ctrl+Alt+Shift+D". The hidden host window rarely gets `WM_INPUTLANGCHANGE`, so a 2 s look at the foreground thread's layout backs it.
9. **Gesture dictation sends the Mac's opts** `[16000, {surface: companion, destination: agent_prompt, commands: [], press_ago_ms}]` (BC hand-off) and falls back to the one-argument call on `-32602/-32601` (build-B's installed engine predated 153 until H-ENGINE); the approval listen sends `{surface: companion, destination: agent_prompt}` with the same fallback.
10. **Spoken approval safety** (writer's call): decided only when every word belongs to one polarity (politeness fillers aside); "don't do it", "no, go ahead" → null.
11. **Typed follow-ups** use the card's marker family with a 1 s floor: `[voice note, 1 s] …` for a voice-key card, `[command, 1 s] …` for command (H3-C1).
12. **Modifier-only chords are bound on Windows** (recorder + hook + chord protection); the Mac deferred binding (H3-C1). Parity difference, opt-in, default off.
13. **A failed send keeps the failed card until ⎋** (D13) instead of 149's 2 s hint.
14. **Evidence form**: build-B's desktop captures came back black (the RDP session was not being composed), so states are proven by in-app XAML renders (RenderTargetBitmap of the island and the card window) plus UIA reads and one GDI desktop shot.
15. **`Store.showAnswerCard`** twin is `AnswerCardWindow.Shared().Begin/Present` (no Store partial — ownership).

## 153-as-built-BC.md deviations table — rows followed (H-BC 253f1ada)
| Row | Windows twin |
|---|---|
| §4.4 `shaped ?? text` in companion fields | followed for the gesture's live words (`DictationEvent.DisplayText`) |
| §10.3 top-level `purpose`, prefixes, ≤ 60 chars, fallback `reason` → tool name | followed (`CommandCard.FromEvent` / `ParsePurpose`) |
| §10.4 the 12th `ToolCallStarted` freezes and cancels | followed |
| §10.6 8 s listen, `surface: companion, destination: agentPrompt`, reads `text`, ignores `command` | followed |
| §12.2 follow-up = hold the mode key; typed `[command, 1 s]` | followed (the voice-key card's typed follow-up uses `[voice note, 1 s]`) |
| §12.4 no speaking phase, a parallel flag | followed |
| §12.6 modifier-only: settings row only on the Mac | row + opt-in built; Windows also binds (no Input Monitoring analogue; `WH_KEYBOARD_LL`, default off) — parity difference |
| §15 `dictateAnywhere` registers nothing yet (Mac) | Windows registers Ctrl+Alt+D (COMMON.md seam, proof 22) and reports the gesture on `Hotkeys.DictateAnywhere`; without W153D's controller the press says "Dictate anywhere isn't available in this build" |
| Hand-off constants `tapToggleMs 400`, `maxSteps 12`, `textCap 60`, `listenSecs 8`, `doneMs 6000` | followed (`AutomaticTapMs`, `MaxSteps`, `PurposeMax`, `ListenSeconds`, `DoneMs`) |
| Hand-off `dictation_start` params | followed (deviation 9) |
Rows §4.1, §7.4, §3.1, §8.5, §6.1, §6.4, §8.1, §8.7, §14.3, §14.4, §9.2 belong to W153B / W153D; §11.2 is engine-side.

## Open items
- A live command turn with a model (build-B has no credential): `send_input` with `[command, N s]`, a real `ApprovalRequested` in the card, a spoken "yes" in the 8 s listen, the cap's `cancel(desk, root)` — verified from recorded sequences only.
- Multi-display: the card sits on the display under the pointer at `Begin`; verified on one display.
- W153D: subscribe `Hotkeys.DictateAnywhere`, answer `TakeDictateWords`, reuse `AnswerCardWindow` for the failure card and the rewrite panel.
- After H-A / H-ENGINE: optionally pass `DictationOpts { surface: companion }` on the gesture dictation; re-check the approval listen against the 153 engine.
- The Ctrl+L history list view (deviation 5).
- Integrator: `SettingsWindow.cs`, `WW3Install.cs` and `App.xaml.cs` are untouched by this wave.
