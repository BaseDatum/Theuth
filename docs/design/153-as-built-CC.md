# 153 as-built — CC (companion contract / PIN)

Worktree `~/Develop/theuth-153-cc`, branch `feat/153-cc`, cut from `integrate/153` at `e464a304`. PIN `5e0b9980`
(`/Users/Sky.Moore/Develop/theuth-153-briefs/PIN-CC.txt`). Design: `153-voice-anywhere.md` §14.1–14.4, §15, §12.4, §10.3/10.6;
the engine truth is `crates/voice/src/{lib,shape/mod,dictionary/mod,history/mod}.rs` and `crates/rpc/src/api.rs:1594-1684` on this tree.

## Files
- `COMP/TheuthKit/Models.swift` — `// MARK: 153 — voice`: `Surface`, `Destination` (tag `kind`), `AppProfile`, `DictationOpts`,
  `DictationTimings`, `EndCommand` (tag `command`), `Action`, `ShapePreset`, `EndPhrases`, `EntryId` (= `String`), `EntryKind`, `EntrySource`,
  `Scope` (tag `scope`, named fields), `Variant` (`metaphone: [String]` = Rust's tuple), `Entry`, `HistoryRow`, `PolishReply`, `SuggestionAnswer`,
  `DeviceState`, `InputDevice`, `ModelInfo`, `ModelState` (tag `state`), `DictationState` (+ `inputDevice`, `last`), `VoiceStatus` (+ `inputDevices`),
  `DictationWire` (every `Dictation*` event; + `shaped`, `command`, `actions`, `device`, `state`), `DictationDevice`, `DictionarySuggestion`.
- `COMP/CompanionCore/AskRpc.swift` — `dictationStart(sampleRate:opts:)` (opts trailing positional, `nil` → `null`), `dictationStatus`,
  `dictationCorrection`, `dictationHistory`, `dictationHistoryForget`, `dictationAudioGet`, `dictionaryList/Put/Delete`, `dictionarySuggestionAnswer`,
  `listInputDevices`, `setInputDevices`, `setShape`, `setEndPhrases`, `polishModel`, `setPolishModel`, `polish`, `rewrite`, `voiceStatus`.
- `COMP/CompanionCore/KeyBindings.swift` — `KeyAction.dictateAnywhere` (⌃⌥D) and `.command` (⌃⌥C), both `anywhere`, intent `nil`;
  `ModeKeySettings` (keys `dictateAnywhereEnabled`, `commandKeyEnabled`, `gesture.<ask|dictate|command>`, `micOnKeyDown`, `modifierOnlyKeys`,
  `leaveTextOnClipboard`, `checkPasteLanded`, `appProfiles` as JSON `[AppProfile]`; `UserDefaults`-parameterised like `SpokenReplies`).
- Stubs (`// 153-CC stub — <worker> fills`): `CompanionCore/AnswerCardState.swift` (struct; `Phase` listening…failed, `generation`,
  `begin`, `hide(generation:)` ignores stale, `armsDone`, `doneMs = 6000`), `CompanionCore/CommandCard.swift` (`Row{kind,text,purpose,ok}`,
  `Event`, `Verdict ok|failed|replied`, `maxSteps = 12`, `observe` / `parseVerdict` / `parsePurpose` no-op), `CompanionCore/SpokenApproval.swift`
  (`allow`/`deny` phrase lists, `listenSecs = 8`, `decide(_:) -> Decision?` returns nil), `Companion/Settings/{DictationSettings,CommandSettings}.swift`
  (`DictationSettingsTab` / `CommandSettingsTab`, empty `Form`s), `SettingsWindow.swift` (two tabs, tags `dictation` / `command`),
  `Store.swift` `showAnswerCard(_ state: CompanionCore.AnswerCardState)` no-op.
- `Tests/TheuthKitTests/Fixtures/153-*.json` (13 files) + 4 `FixtureTests`; `W6Tests.KeyTableTests` + 2.

## Design vs built
| Design | Built |
|---|---|
| §14.4 `DictationWire` gains `shaped`, `command: EndCommandWire?`, `actions: [String]?` | The decoder in `Store+Ask.swift:148` is `private`; a public `TheuthKit.DictationWire` with the same keys + the new optionals (`command: EndCommand?`, `actions: [String]?`, `device`, `state`) is the mirror. B3 switches `dictationObserve` to it (the private one shadows it until deleted). |
| §14.4 `Suggestion` payload | `DictionarySuggestion` struct (the event's fields); `DictationDevice` struct; both also readable through `DictationWire`. |
| §15 "`check` refuses the voice key's chord for the new rows and vice versa" | Already the same-scope rule: `check` answers `.usedBy(.voiceKey)` / `.usedBy(.dictateAnywhere)` / `.usedBy(.command)`; no new `Verdict`. |
| §14.3 `appProfiles: [AppProfile]` | One JSON blob under `appProfiles` in the wire shape (`TheuthJSON`), so a row goes into `DictationOpts.app` as is. |
| Fixtures "exported with `just ts-desktop`" | Written by hand from the serde attributes (field names, enum tags, `(String,String)` → array). The ts-rs files under `crates/desktop/ui/src/rpc/bindings/` are gitignored and absent in this tree (no Rust build in CC's scope); regenerate there to cross-check. |
| `ModelInfo` | Only `id`, `name`, `provider` declared (the picker's fields); unknown keys ignored. |
| `AnswerCardState` "class name stays" | `CompanionCore.AnswerCardState` is a pure struct; the Companion target's `AnswerCardState` view-model (`PointAndSay/AnswerCard.swift`) shadows the name inside that module — `Store.showAnswerCard` qualifies it. C1 merges the two. |

## Tests
`BUILD_LABEL=153-CC build-lock ./test.sh` → `✔ Test run with 211 tests in 33 suites passed` (was 205). New: `FixtureTests.dictationOptsEncodeLikeSerde`,
`.dictationEventsEveryNewShape`, `.dictionaryEntriesEveryScope`, `.historyPolishAndStatus`; `KeyTableTests.rows153ModeKeysUniqueWithTheVoiceKey`,
`.modeKeySettingsDefaultsAndAppProfiles`. Pinned encodings: `DictationOpts()` → `{"commands":[]}`; `[16000, null]` is today's `dictation_start`;
`Destination.anyApp` → `{"app":…,"kind":"any_app"}`; `Scope.surface(.anyApp)` → `{"scope":"surface","surface":"any_app"}`; `SuggestionAnswer.notNow` → `"not_now"`.

## Gates
- `BUILD_LABEL=153-CC build-lock swift build` → `Build complete! (143.16s)` (only the pre-existing `shared` actor-isolation warnings).
- Plain `swift test` fails with `no such module 'Testing'` on this CLT-only Mac (130 W1 note) — `./test.sh` is the gate.

## Hand-offs
- **B3**: decode `Dictation*` events with `TheuthKit.DictationWire` (drop the private copy); `shaped ?? text` is what the field shows;
  `command == .send(…)` acts after `DictationFinished`; `actions` are `Action` raw values. Start with
  `dictationStart(opts: DictationOpts(surface: .companion, destination: .agentPrompt, pressAgoMs: …))`; `opts` is the trailing positional
  (`null` = today). `DictationSettingsTab` is yours; bind `ModeKeySettings.micOnKeyDownKey` etc. on `Settings.defaults`.
- **C1**: rows `KeyAction.dictateAnywhere` / `.command` (ids `dictateAnywhere`, `command`; add them to `Hotkeys.names` to register);
  `ModeKeySettings.gesture(.command, d)`, `.commandKeyEnabled(d)`; stub bodies in `AnswerCardState`, `CommandCard`, `SpokenApproval`;
  `Store.showAnswerCard(_:)`; `CommandSettingsTab`. `AnswerCardState` name collision as above.
- **Windows twin**: the same fixtures (`Fixtures/153-*.json`) are the C# contract test's input; chords ⌃⌥D/⌃⌥C have no `platformDefaults` row (Ctrl+Alt+D/C are the same chord).
