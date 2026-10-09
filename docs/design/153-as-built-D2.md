# 153-D2 as-built — any-app dictation controller (macOS companion)

Worktree `~/Develop/theuth-153-d2`, branch `feat/153-d2`, cut from D1's DONE `b44a9615`. Design `153-voice-anywhere.md` §3.2, §8.6,
§9.1, §13, §14.3, §15, §16 row 24, §18 Q9 (island area at press). `COMP/` = `crates/desktop/companion/Sources/`.

## Files
- `COMP/CompanionCore/AnyAppSession.swift` (new, pure) — one gesture's memory over D1's machines: `init(snapshot:profile:selection:trusted:
  ownWindow:screenLocked:)` → `verdict` (`assess(s, terminal: profile.terminal)`), `entry = .refused(hint) | .dictate(label) | .rewrite(chars)`,
  `label` ("Dictating into Mail" · "Terminal · unknown field — paste" · "Accessibility off — pasting blind"), `method = .direct | .lease`
  (`editable` ∧ no failed read-back), `directFailedOnce()`, `payload(text)` (a joining space after a previous delivery), `exceedsCap`
  (20 000 chars), `record/isDelivered`, `returnAllowed` (`assess(payload: .returnKey)` ≠ secure, never on certainly-not), `scratch(i)` →
  `.selectBack(chars) | .backspace(chars) | .ignore(hint)` (last delivery only; a paste only when verified), `refined(instruction:result:)`
  → `thread: [[String]]`, `doneCopy` ("✓ 3 lines into Mail"). Constants: `refusalPrefix`, `ownWindowHint "Use Ctrl+D here"`, `lockedHint`,
  `blindHint`, `scratchHint`, `maxCharsPerGesture`.
- `COMP/Companion/AnyApp/AXTarget.swift` (new) — `AXTarget.capture()`: `AXUIElementCreateSystemWide` → `kAXFocusedUIElement` → pid,
  `NSRunningApplication`, role/subrole, `AXUIElementIsAttributeSettable(kAXValue)` → `isEditable`, value / selected-range presence,
  secure by role/subrole, window `kAXTitle`, `kAXSelectedText` (not read on secure fields); no element → `AXSnapshot(appBundleId:
  frontmost)`. `read() -> FieldRead?` (nil when the focus left the element or the app quit; value ≤ 60 000), `insert` (`kAXSelectedText`),
  `selectBack(n)`, `focus()` (activate + `kAXFocused`). `SystemState.screenLocked` (`CGSSessionScreenIsLocked`), `accessibilityTrusted`.
- `COMP/Companion/AnyApp/SystemPasteboard.swift` (new) — `SystemPasteboard: CompanionCore.Pasteboard` over `NSPasteboard.general`
  (snapshot = items × types → data, promised types without bytes skipped; write = `clearContents` + one `NSPasteboardItem` with every
  type, answers `changeCount`; restore = clear + items; `copyPlain`). `Keystrokes.paste/returnKey/backspace(n)` = `CGEvent` down+up with
  explicit flags `postToPid(pid)`; `mediaPlayPause()` = `NX_KEYTYPE_PLAY` (16) system-defined event down+up on the HID tap.
- `COMP/Companion/AnyApp/AnyAppController.swift` (new) — `press()` (capture → profile `AppProfiles.resolve(bundleId:name:in:
  ModeKeySettings.appProfiles)` → `AnyAppSession`; own windows = our pid or `io.theuth.mac`; media toggle when `pauseMediaWhileDictating`),
  `opts(pressAgoMs:)` = `DictationOpts(surface: .anyApp, destination: .anyApp(app: profile.id), app: profile, commands: [])`,
  `utterance(i, text:, final:)` → one serial chain: polish (`polish(text, .anyApp(app), nil)`, 4 s race, refused/timeout → shaped; pill
  label "polishing…") → `deliver`: direct `insert` + 150 ms `PasteVerifier.verify(reads: 1)`; not landed → the lease for this and every
  later utterance; lease = `begin → write(text, session: gestureId) → ⌘V to pid → pasteIssued → settle at 500 ms (detached, generation-
  stamped) + PasteVerifier.run(150/350/1000)` awaited inline (gated on `checkPasteLanded`; off → 120 ms gap, unverified); `.notLanded` /
  lease `Failure` → `failed[]` + note. `finish(text:sendIt:mouse:)`: awaits the chain, media back, Return via `Keystrokes.returnKey` iff
  `returnAllowed`, else "not sent — secure field"; failures → the card; else `doneCopy`. Rewrite: `rewrite(instruction, selection, app,
  thread)` → panel; Replace = `focus()` + 200 ms + the same `deliver`; a hold while the panel is up = refine (same target, thread). Card
  actions: Copy (`copyPlain`), Try again (`focus()` + lease once more), Open in theuth (`resolveDesk` + `send_input` with
  `PointController.noteLine`). `hotkeyEnabled` = key unset → `AXIsProcessTrusted()`, else the switch. `dryRun` for probes.
- `COMP/Companion/AnyApp/AnyAppCard.swift` (new) — `AnyAppCardModel` (`kind failure|rewrite`, title, text, instruction, excerpt, note,
  busy, `onAction copy|tryAgain|openInTheuth|replace`), `AnyAppCardView` (title in the failed colour for failures; buttons; footnote),
  `AnyAppCard` panel (`GlassPanel.make`, 440 pt, `IslandArea.frame` at the press point, 180 ms resize, keys ⎋ · ⏎ primary · C · O).
- `COMP/Companion/AnyApp/AnyAppProbe.swift` (new) — `--probe-anyapp <refused|landed|notlanded|rewrite>` (`--shareable`, `dryRun`, no AX
  read of a real app, no keystroke, no pasteboard write, no RPC) → `/tmp/153-d2/153-anyapp-<kind>.png` + `probe.log`.
- `COMP/Companion/Settings/AppProfilesSettings.swift` (new) — `AppProfilesSection`: "Dictate anywhere" switch (writes
  `dictateAnywhereEnabled`, `PointController.hotkeyChanged()`), `KeyRow(.dictateAnywhere)`, `GesturePicker(gesture.dictate)`,
  Accessibility status + "Open System Settings" (`x-apple.systempreferences:…Privacy_Accessibility`), per-app rows (Add app… from
  running regular apps; terminal · polish · learn; remove) + "Other apps" (`any_app`) row, `leaveTextOnClipboard`, `checkPasteLanded`,
  "Pause media while dictating (may un-pause)" (`pauseMediaWhileDictating`, off).
- `Tests/CompanionCoreTests/AnyAppSessionTests.swift` (new) — 6 tests.
- Shared-file hunks: `PointController.swift` (+114/−19: `Name.dictateKey/.dictateEscapeHeld`, `applyHotkey` row + probe guard + one
  held-⎋ per chord, `keyDown/keyUp(.dictate)`, `chordSymbol/pillLabel(.dictate)`, `.start`/`.relabel`/`.discard` → `anyApp`, no trail
  in dictate mode, `beginDictation(opts:)`, `Wire.shaped/command`, `DictationText` final → `anyApp.utterance`, `DictationFinished` with
  `send` → `dictateStop`, `stopSequence` → `dictateStop`, `Flags.probeAnyApp`, `installPointAndSay` probe line);
  `DictationSettings.swift:20` placeholder → `AppProfilesSection()`; `SettingsStore.swift:66` `anyProbe` + `probeAnyApp`.

## Design vs built
| Design / brief | Built |
|---|---|
| §13.5 failure card = C1's `AnswerCard` with a new `Kind` | Own `AnyAppCard` panel (same glass, width, island placement, ⎋ rule): `AnswerCardState.Kind` is a closed enum in C1's file and the card's anatomy (desk thread, J/K, ⌘L, follow-up field) does not fit Copy · Try again · Open. No hunk in `AnswerCard*.swift`. |
| §3.2 "focus leaves the target app → paused" | Not built: a delivery whose read finds another element is `unknown(.focusMoved)` (no card, words kept in the pill); the gesture goes on. |
| §13.3 step 4 "the next utterance may queue" | One chain; a lease delivery awaits its verification inline (landed usually at the 150 ms read), so a second ⌘V never races the first's pasteboard write. `pasteGapMs = 120` only when the check is off. |
| §13.4 verify in parallel with settle | Settle is detached (500 ms, `settle(generation:)`), verification inline — see above. |
| §13.5 kinds | `failureNote` strings: "checked at 150 · 350 · 1000 ms — field unchanged · clipboard restored/kept", "clipboard snapshot/write/restore failed", "paste command failed", "the app is gone", the 20 000-char cap. No counters in a debug row (§16 row 26 is a live gate). |
| §13.5 card stays until acted on | Also retired by the next ⌃⌥D press (the words are in the engine's history). |
| Brief 1 "default ON after Accessibility is granted; OFF until then" | `hotkeyEnabled`: key never written → `AXIsProcessTrusted()`; the switch wins once set (§13.7 blind pasting stays reachable). Re-applied when the Dictation tab appears / the switch flips; a grant made while the app runs takes effect at the next Settings visit or launch. |
| Brief 3 "polishing… ≤ 4 s" | `withTaskGroup` race against `Task.sleep(4 s)`; the losing RPC is cancelled (whether `RpcClient.call` honours cancellation is not verified — the reply is dropped either way). |
| §8.6 pause | Play/Pause at a non-refused dictate press and again at finish/abandon; one `mediaToggled` flag per gesture (a refused press never toggles). |
| §13.2 "no CGEvent typing" | Honoured; direct = `kAXSelectedText`, else ⌘V. "scratch that" on a verified paste types ⌫ × n (the brief's rule). |
| `Destination.anyApp(profile)` (D1 hand-off) | The wire case is `.anyApp(app: String)` → `profile.id`. |
| Pill copy "Dictating into ‹App› · ⌃⌥D held" | `VoiceModeView`'s trailer order: "· ⌃⌥D held · Dictating into Mail" (no edit to the island view). Hints "⎋ cancel · double-tap lock · say 'send it'" not added to the pill (one-line capsule). |
| Rewrite "Refine (hold again with the thread)" | Built; the press while the panel is up never re-reads AX (the focus is on our panel). |

## Tests / gates
- `BUILD_LABEL=153-D2 build-lock swift build` → `Build complete! (10.81s)` (incremental; pre-existing `shared` / `NSEvent Sendable` warnings only).
- `BUILD_LABEL=153-D2 build-lock ./test.sh` → `✔ Test run with 256 tests in 43 suites passed` (250 + 6: `refusePath`, `directVersusLease`,
  `returnRule`, `scratchThatRule`, `payloadSpacingAndCap`, `rewriteThreadBuilding`).
- Probes (`.build/debug/Companion --probe-anyapp <kind> --shareable`, nothing sent, no hot key, no TCC prompt), `/tmp/153-d2/`:
  `153-anyapp-refused.png` (pill hint "Nothing to type into — button", 236×36), `153-anyapp-landed.png` (pill, finals solid + partial
  muted, "· ⌃⌥D held · Dictating into Mail", 560×36), `153-anyapp-notlanded.png` (card 440×113), `153-anyapp-rewrite.png` (panel 440×193);
  logs `probe-<kind>.log` (`landed: method=direct returnAllowed=true`, `notlanded: verdict=unknown method=lease outcome=notLanded`).
  `--probe-settings dictation --shareable` → `153-settings-dictation.png` (window 14014; `settings-probe.log` has 0 `hotkey` lines).
- Not run (needs Sky at the keyboard): §16 rows 24 live half (TextEdit insert, clipboard restored within 600 ms), 25, 26; the AX fill
  against a real focused element and `postToPid` ⌘V are `[INFERENCE]` from the APIs' documented behaviour.

## Hand-offs
- **Windows twin**: `AnyAppSession` 1:1 (entry/label/method/payload/returnAllowed/scratch/thread, constants above); delivery sequence
  per utterance = polish? → direct (`SendInput` unicode ≤ 200 chars, 150 ms read-back, one failure → lease for the gesture) → lease
  (snapshot → write + session format → Ctrl+V to the foreground thread → settle 500 ms → reads 150/350/1000 awaited inline); Return only
  when `returnAllowed`; refusal copy `"Nothing to type into — ‹role›"`, `"Use Ctrl+D here"`, `"Screen locked"`, `"Accessibility off —
  pasting blind"`; done copy `"✓ N line(s) into ‹App›"`; card kinds `failure` (Copy · Try again · Open in theuth + the checked-at note)
  and `rewrite` (instruction · excerpt · proposal · Replace ⏎ · Refine = hold · ⎋); settings keys `pauseMediaWhileDictating` + CC's.
- **Integrator**: hunks in `PointController.swift`, `DictationSettings.swift:20`, `SettingsStore.swift:66` as listed; `AnswerCard*.swift`
  untouched. A later wave may fold `AnyAppCard` into `AnswerCard` by adding `Kind.anyAppFailure/.rewrite` + an `actions` field.
- **Engine**: `rewrite`'s `thread` is sent as `[[instruction, result]]` pairs per `AskRpc`; `polish` is called with `session: nil`
  (Q8: the desk).
