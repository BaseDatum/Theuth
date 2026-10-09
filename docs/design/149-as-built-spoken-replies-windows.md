# 149 as-built — spoken replies + key routing, Windows (fix/149w-spoken)

Branch `fix/149w-spoken` off `origin/main ece44049`; worker `w149-fix`, 2026-10-09 15:56–16:15. The Windows twin of the macOS
session's `docs/design/149-as-built-spoken-replies.md` (its §K = the keys). Sky's words (15:58, 16:02): spoken replies must be
options "not always on", and the panel key and the Ask-bar key "open the same thing…?". Only `crates/desktop/companion-windows/` changed;
`Island/*` untouched; the shared files (`PolicySettings.cs`, `SettingsWindow*.cs`) got additive edits only.

## 1. Spoken replies — two independent opt-in switches, both OFF by default

**Settings.**
- `PointSettings.Speak` (Voice key › "Speak the answer aloud") now defaults **off**. A stored `voiceKeySpeak` is honoured only when the
  user set the switch themselves: a new marker `voiceKeySpeakSet` is written when the Settings toggle is flipped; `Load` reads
  `Speak = SpeakSet && voiceKeySpeak`. **Existing users:** every earlier Voice key edit wrote back all six keys with the old default
  (`voiceKeySpeak: true`), so that value cannot mean "set explicitly" — those users get off too; only a switch flipped from now on
  sticks. Proven on build-B: the installed companion's log said `speak=on` (stored true) and the fixed build on the same settings.json
  says `point-and-say: installed (… speak=off)`.
- `CompanionSettings.AskSpeak` (new, key `askSpeak`, default off) and a new **Settings › Ask bar** page (nav key `ask`, between Voice key
  and Keys) with "Speak the reply aloud" (`settings-ask-speak`) and a caption naming the stop rules.
- Core tests `SpokenRepliesTests`: both defaults off; stored-true-without-marker → off, marker + true/false round-trips; `askSpeak`
  round-trip; the switches are independent and gate final, non-empty, non-replay text only.

**One shared speaker.** Core `SpeakerModel` (`Speaking`, `Begin` displaces the current utterance, `Named` adopts the engine's id from
`SpeechStarted`, `Stop(why)` for Esc · NewAsk · NewGesture · Glyph, `Finished(id)` only for the current one; `ShouldSpeak(source, …)`).
App `Voice/Speaker` (new) owns the one `SpeechPlayer` (follows `Speaking.Session`), `Speak(session, text, source)` →
`speak` RPC, `Stop(why)` → player cut + `stop_speaking`, `Changed` for the glyphs; created once per Store (`Store.Spoken.cs`,
`store.Speaker`). `PointController` takes it instead of its own player: a new gesture → `Stop(NewGesture)`, Esc/discard →
`Stop(Esc)`, `TurnFinished` → `Speak(desk, answer, VoiceKey)` when `ShouldSpeak`; `--speech-probe` goes through the shared speaker.
**Ask replies:** Core `StoreModel.AskReplyFinished(session, text)` is raised once the desk's turn is final and successful (never a
partial, never a failed turn); `Store.InstallSpokenReplies` speaks it when `AskSpeak` is on (log: `ask: reply final on … — speaking |
not spoken (Ask bar › Speak the reply aloud is off)`); a new ask (the hold starts) → `Stop(NewAsk)`; Esc in the Ask bar → `Stop(Esc)`.
Tests: one speaker across sources (a second Begin displaces the first), the four stop triggers, Finished only for the current id.

**Speaker glyph** (Segoe Fluent Volume `E767`, accent): on the rail's actions pill it replaces the mic while speaking
(`act-speaking`, "Speaking — click to stop"), and in the Ask bar's receipt row (`ask-speaking`); clicking either → `Stop(Glyph)`.
The island's answer card is `Island/*` (the sibling's files) and was not touched.

## 2. Keys — the panel key always opens the panel

Core `KeyRouting.Route(action, needsYou, askBarAvailable)`: `OpenPanel → Panel` always, `OpenAskBar → AskBar` (None when no bar),
`OpenAgents → Agents`. `Surfaces.OnHotkey` switches on it; the 130-W3 fallback ("the panel on the oldest needs-you item; the Ask bar
when nothing needs you") is gone. Log: `hotkey openPanel: Attention panel (empty state | N need you)`.
**"The hold still talks":** `AskBarInstall` no longer rides on the panel key's press; a panel-key press still held after
`HoldToTalkMs = 250` becomes the Ask bar's voice press (the bar opens over the panel and dictates to theuth; release sends — the bar's
own 0.25 s press rule starts from there, so ~0.5 s of hold in total), a tap is the panel. The Ask-bar key opens the bar and, held,
dictates into it as before. Test `ThePanelKeyAlwaysOpensThePanelAndTheAskKeyOnlyTheAskBar`.

## Files
- new: `src/Theuth.Companion.Core/SpeakerModel.cs` (SpeakerModel, SpeechSource, SpeechStop, KeySurface, KeyRouting), `src/Theuth.Companion/Voice/Speaker.cs`,
  `src/Theuth.Companion/Store.Spoken.cs`, `tests/Theuth.Companion.Core.Tests/SpokenRepliesTests.cs` (8)
- changed: Core `PointSettings.cs`, `PolicySettings.cs` (AskSpeak, additive), `StoreModel.cs` (AskReplyFinished); app `PointAndSay/PointController.cs`,
  `WW3Install.cs`, `Windows/Surfaces.cs`, `AskBar/AskBarInstall.cs`, `AskBar/AskBarWindow.cs`, `Windows/RailPillsWindow.cs`,
  `Settings/SettingsWindow.cs` (+ page, additive), `Settings/SettingsWindow.WW3.cs` (Voice key toggle, Ask bar page); test `PointSettingsTests.cs` (default)

## Gates
- Mac: Core **301/301** (293 + 8), Rpc **68/68**; 0 warnings.
- build-B Debug `Theuth.Companion.slnx` x64 @ fb96f262, slot `spoken`: **0 errors / 0 warnings** (33 s, job 20261009T150515-415).

## Live on build-B (Debug fb96f262 `--background --shareable`; shell up; desk owner `spoken`, 16:05–16:10)
Evidence `~/.theuth/briefs/2026-10-09/149-windows/evidence/spoken/`.
- **Defaults / migration:** `16:04:00 pid 15444 (installed, stored voiceKeySpeak:true) point-and-say: installed (… speak=on)` →
  `16:06:36 pid 3152 (fix) spoken replies: installed (Ask bar speak=off; one shared speaker)` · `point-and-say: installed (… speak=off)`.
  Settings › Ask bar and › Voice key both show the switch off: `SPOKEN-settings-ask-bar-speak-off.jpg`, `SPOKEN-settings-voice-key-speak-off.jpg`.
- **Panel key with nothing pending** → the Attention panel's empty state, not the Ask bar:
  `16:06:55.827 WM_HOTKEY Ctrl+Shift+Space (openPanel)` → `16:06:55.873 panel open (Mouse) … active=true` → `16:06:55.874 hotkey openPanel: Attention panel (empty state)`
  (the injected key-ups re-activate Notepad 30 ms later, which closes the panel by focus loss — a windows-mcp artefact, so the empty-state
  screenshot `SPOKEN-K-attention-panel-empty-state.jpg` was taken with the panel opened from the inbox pill, the same `panel.Open`).
  A press held longer: `16:07:54.566 ask bar: the panel key is still held after 250 ms — talking` → the bar opens and dictates
  (`SPOKEN-K-panel-key-held-past-250ms-ask-bar-talks.jpg`, "Nothing recognized" with no speech).
- **Speech through the shared speaker** (`--speech-probe`, 70 chars, no model needed):
  `16:09:24.156 speaker: speak(01a11896-…, 70 chars) for VoiceKey` → `16:09:26.099 speech: 01a12136-… started (engine rate 24000 Hz, decoding at 48000 Hz) → device opened "Speaker/HP (Realtek High Definition Audio)" state=Playing`
  → `16:09:27.320 speech: 01a12136-… finished — 65 packets decoded, mean rms 0.079 (not silent), gaps=0`.
  The rail's actions pill shows the accent speaker glyph in place of the mic while it speaks: `SPOKEN-rail-speaker-glyph-while-speaking.jpg`
  (vs `SPOKEN-rail-before-speech.jpg`).
- Not producible on build-B (no model credential): a real Ask reply being spoken, and a live Esc/new-ask/glyph stop of a long utterance —
  the gate (`ShouldSpeak`), the one-speaker and stop rules are Core-tested; the glyph's click and Esc call the same `Speaker.Stop`.
Round closed: kill-mine → start-installed (pid 2312) → `Run\TheuthCompanion = "…\companion\Theuth.Companion.exe" --background` → desk released.

## Deviations / notes
- "Hold still talks" on the panel key = the Ask bar's dictation to theuth (the pre-ruling hold semantics), not a reply to the focused card;
  the Mac may choose otherwise — easy to re-point (`AskBarInstall`'s hold timer).
- The speaker glyph is on the rail pill and in the Ask bar; the island answer card (sibling's `Island/*`) does not show one yet.
- Migration rule for `voiceKeySpeak`: honoured only with the new `voiceKeySpeakSet` marker (see §1); a user who had turned it on before today
  must flip it on once more.
