# 153 as-built — B3 (companion fields, macOS)

Worktree `~/Develop/theuth-153-b3`, branch `feat/153-b3`, cut from the CC PIN `5e0b9980` (+ `557eb083` as-built). Design
`153-voice-anywhere.md` §3.1, §3.3, §4.4, §6.1, §6.4, §7.4, §8.5, §8.7, §14.3–14.4, §16 row 13; engine truth `153-as-built-{A1,A3,A4,CC}.md`.
`COMP/` = `crates/desktop/companion/Sources/`.

## Files
- `COMP/CompanionCore/AskVoice.swift` — `AskVoiceMachine`: `micOnKeyDown` (set per press by the controller), `pressedAt`, `isPressed`;
  effects `.openMic` (key-down when the setting is on), `.discardMic` (a tap after `.openMic`), `.sendNow(text)` ("send it");
  `.dictated(text, send:)` is accepted in `.recording` too (the engine ends a dictation itself after "send it" settles, §7.3).
  `.startRecording` keeps its pre-153 meaning (the mic opens there unless `.openMic` already did) — the 149 tests stay unmodified.
  `AskDictation`: `text` = `shaped ?? text`; `.text(…, command:)` arms `send` (sticky to the finish); `.device` → `deviceHint`;
  `correctionId` = the finished id until `.settled`; `static event(_ DictationWire) -> Event?` is the one wire → reducer mapping.
- `COMP/CompanionCore/AskRouter.swift` — `AskModel.suggestion: DictionarySuggestion?` (one line; the file has no owner in the table).
- `COMP/Companion/AskBar/Store+Ask.swift` — private `DictationWire` deleted; `dictationObserve` = `TheuthKit.DictationWire` → `AskDictation.event`;
  `DictionarySuggestion` routed to `ask.suggestion`; `answerSuggestion(_:)` (`nil` = auto-dismiss, no RPC); `sendAskNow` (no hold);
  `commitAsk` sends `dictation_correction(correctionId, text)` when the ask was dictated and drives `CompanionCore.AnswerCardState`
  through `store.showAnswerCard` (`begin` → thinking → answer/failed per delta; C1 renders).
- `COMP/Companion/AskBar/AskBarPanel.swift` — `hotkeyDown` reads `ModeKeySettings.micOnKeyDown`; `startMic()` once per gesture (`micOpen`);
  `feed` buffers chunks while pressed-with-mic or `.starting`; `beginDictation` sends
  `dictation_start(16000, {surface: companion, destination: {kind: agent_prompt}, press_ago_ms: now − pressedAt})`; `.sendNow` →
  `store.sendAskNow`; status slot shows `deviceHint` / "sending…"; `perform` is inert under any probe; `anchorFrame` + `static frame(for:)`;
  probe knobs `probeMicOnKeyDown/probeMicOpen/probePendingChunks`.
- `COMP/Companion/AskBar/AskBarInstall.swift` — creates `DictionarySuggestionCard`; probe scenarios 4 (MIC AT KEY-DOWN · TAP / HOLD) and 5 (SUGGESTION CARD).
- `COMP/Companion/Voice/HoldToTalk.swift` — the same mic-at-key-down / opts / `.sendNow` (→ `onSend ?? onText`) / device-hint changes.
- `COMP/Companion/Voice/DictionarySuggestionView.swift` (new) — the §6.4 card (frame D copy), `DictionarySuggestionCard` (a non-key
  `GlassPanel` 8 pt under `askBar.anchorFrame`, `becomesKeyOnlyIfNeeded` so a click never closes the Ask bar, 20 s auto-dismiss),
  `PCMPlayer` (16 kHz i16 → `AVAudioEngine`/`AVAudioPlayerNode`, the `SpeechPlayer` shape) for ▶ `dictation_audio_get(id, nil)`.
- `COMP/Companion/Settings/DictationSettings.swift` — "Open the mic on key-down" (`ModeKeySettings.micOnKeyDownKey`, default on),
  "Learn from my corrections" (read-only pointer), "Apps" placeholder (wave D), "Last dictation" (§8.7 line from `dictation_status.last`).
- `Tests/CompanionCoreTests/Voice153Tests.swift` (new); `AskTests.swift:188` — the 149g driver's `switch` lists the three new effects (compile only).

## Design vs built
| Design | Built |
|---|---|
| §8.5 `.openMic` on key-down, `.startRecording` at 250 ms | Same; with the setting OFF `.startRecording` alone opens the mic (controller flag), so the 149 `AskVoiceMachine` tests run unmodified. Audio between key-down and 250 ms is kept for a hold (`pendingChunks` flushes after `started`), dropped for a tap. |
| §8.5 "800 ms min-note" | `PointMachine.minNoteMs` — the voice key's, untouched (not this machine's). |
| §7.4 `setText` then `commitAsk` with no hold | `.sendNow` → `Store.sendAskNow` → `commitAsk` directly; `submitAsk`'s `hold` is bypassed. `near_miss` is treated as send. |
| §3.1 settling copy "sending in 1.2 s · ⎋ keep editing" | Status slot shows "sending…" while `send` is armed; no countdown (the actor owns the settle). |
| §6.1 correction "at send" on every surface | Ask bar only (`commitAsk`). `HoldToTalk`'s text goes to callers in `AgentsPanel`/`ReplyField` (not B3's files) — `correctionId` is set for them too; hand-off below. |
| §6.4 auto-dismiss 20 s "counts as Not now without the cooldown" | No RPC on dismiss: A3 counts an unanswered card itself (`CARD_TIMEOUT_MS`). |
| §6.4 ▶ `dictation_audio_get(id, utterance)` | `(id, nil)` per A3:22; ▶ hidden when `has_audio == false` (A4 publishes `false` today). |
| §3.3 the Ask reply ends in the AnswerCard | `commitAsk`/`askObserve` fill `CompanionCore.AnswerCardState` and call `store.showAnswerCard` (CC's no-op until C1). The receipt row stays. |
| Settings "Learn from my corrections" shows `voice.history.learn` | No RPC exposes it (`api.rs` has no voice settings getter); the row is a read-only pointer to theuth › Settings › Voice. |
| `DictionarySuggestion` state on `Store` | `AskModel.suggestion` (AskRouter.swift) so the card observes `$ask` like `HoldToTalk` does. |

## Tests
`BUILD_LABEL=153-B3 build-lock ./test.sh` → `✔ Test run with 217 tests in 35 suites passed` (was 211). New: `AskVoiceMachineTests.sendItSkipsTheHold`,
`.openMicDiscardsOnTap`, `.correctionSentOnCommit`, `.sendArmsAndDeviceHints`; `DictationWireTests.shapedWinsOverRawAndCommandRides`,
`.scratchEmptiesThePreviousUtterance`. The 149 `AskVoiceTests` / `AskDictationHoldTests` / `AskDictationTests` bodies are unchanged.

## Gates
- `BUILD_LABEL=153-B3 build-lock swift build` → `Build complete!` (pre-existing warnings only).
- Live probe (debug binary hand-bundled to `/tmp/153-b3/Theuth Companion.app`, engine 2.14.0 on 127.0.0.1:56525, no mic, nothing sent):
  `Companion --ask-probe 40 --shareable --synthetic-dictation` → `/tmp/130-w6-dictation.log`: the three 149g asserts PASS, then
  `ASSERT TAP opened the mic, buffered, then discarded everything and never started a dictation: PASS` (`pressed mic=true buffered=2 phase=idle → voice=idle mic=false buffered=0 dictation_status.active=nil`),
  `ASSERT HOLD opened the mic once at key-down, started our dictation, ended idle with the slot free: PASS` (`dictation_start → 01a12201-49e9…`),
  `ASSERT SUGGESTION CARD shown under the Ask bar's anchor: PASS` → `/tmp/153-b3-suggestion-card.png` (420 × 125, frame D copy).
- `Companion --probe-settings dictation --shareable` → `/tmp/153-b3-settings-dictation.png`.
- The engine on this Mac is pre-153 (no `shaped`, no `dictation_status.last`): the probe exercised the `shaped ?? text` fallback and
  "Last dictation: none yet". `shaped`/`command` decoding is proven by the wire tests, not live. `PCMPlayer` was not sounded (no audio
  on this engine; a probe would play on the owner's speakers) — `[INFERENCE]` it plays, same node graph as `SpeechPlayer`.

## Hand-offs
- **C1**: `store.showAnswerCard(_:)` is now called with a live `AnswerCardState` (`begin(question:)`, `.thinking` → `.answer` with `text`/`turnFinished`,
  `.failed` with `status`) from `Store+Ask.swift`; render it. `HoldToTalk.onSend` exists for "send it" on the row mic / card V
  (falls back to `onText`); wire it where the caller commits without the hold, and send `dictation_correction(store.ask.dictation.correctionId, text)` there.
- **Windows twin**: `holdThreshold` 250 ms, `maxDuration` 60 s; mic opens at key-down when `micOnKeyDown` (default on); a release < 250 ms
  cancels the recorder and drops the buffer — no `dictation_start`; a hold flushes the buffered chunks after `started`. `dictation_start`
  params: `[16000, {"surface":"companion","destination":{"kind":"agent_prompt"},"commands":[],"press_ago_ms":<now − key-down ms>}]`.
  Field shows `shaped ?? text`; `DictationText.command == {"command":"send"}` (any piece, sticky) + `DictationFinished` → commit with no hold;
  `DictationFinished` while still recording lands like a release. `DictionarySuggestion` card: Add / Not now / Never for this →
  `dictionary_suggestion_answer(id, "add"|"not_now"|"never")`; 20 s auto-dismiss sends nothing; ▶ only when `has_audio`, plays
  `dictation_audio_get(id, null)` (base64 LE i16 16 kHz). `dictation_correction(id, sent_text)` at commit when the field's text came from
  dictation `id`; cleared when the field is emptied or a new dictation starts.
