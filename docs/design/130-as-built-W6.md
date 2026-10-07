# 130 — as-built W6: four post-landing fixes to the macOS companion

Status: built 2026-10-07 evening on `feat/130-w6-fixes` (cut from `origin/main` `1354b6d8` = v2.12.0), against the user's
running engine (`~/.theuth/engine.json` → `127.0.0.1:59231`, untouched). The four items are Sky's 18:31/18:36 reports,
recorded verbatim in `docs/design/130-companion.md` Revision 2 (R2-1 … R2-4). Toolchain as W1–W3: Command Line Tools
only, Swift Testing through `test.sh`, KeyboardShortcuts pinned 1.10.0 (still used for the global registration and the
key-down/key-up hotkey events — just not its `Recorder`). Installed app and `/Applications` untouched; the user's
UserDefaults suite (`io.theuth.mac.companion`, shared with the installed app) was written to only by the Keys probe,
which restores what it changed (see §2.4).

## 1. What was wrong, what changed (path:line)

### 1.1 Settings › Keys crashed (R2-1)

**Evidence.** `~/Library/Logs/DiagnosticReports/Companion-2026-10-07-182854.ips` thread 0 (`/tmp/130-crash.py`):
`libswiftCore _assertionFailure` ← `closure #1 in variable initialization expression of static NSBundle.module`
(`resource_bundle_accessor.swift:12`) ← `String.localized.getter` (`Utilities.swift:10`) ←
`KeyboardShortcuts.RecorderCocoa.init(for:onChange:)` (`RecorderCocoa.swift:90`) ← `_Recorder.makeNSView`.

**Root cause.** The SwiftPM-generated accessor, read after one build
(`.build/arm64-apple-macosx/release/KeyboardShortcuts.build/DerivedSources/resource_bundle_accessor.swift`):

```swift
static nonisolated let module: Bundle = {
    let mainPath = Bundle.main.bundleURL.appendingPathComponent("KeyboardShortcuts_KeyboardShortcuts.bundle").path
    let buildPath = "/Users/Sky.Moore/Develop/theuth-130-w6/crates/desktop/companion/.build/arm64-apple-macosx/release/KeyboardShortcuts_KeyboardShortcuts.bundle"
    let preferredBundle = Bundle(path: mainPath)
    guard let bundle = preferredBundle ?? Bundle(path: buildPath) else {
        Swift.fatalError("could not load resource bundle: from \(mainPath) or \(buildPath)")
    }
    return bundle
}()
```

It looks at the `.app` ROOT and at the absolute `.build/…/release/` path baked in at compile time. `bundle-companion.sh`
puts the bundle in `Contents/Resources/` — which neither lookup sees. The machine that built the app has the `.build`
path, so it never crashed there; this machine does not. Every `Bundle.module` use in KeyboardShortcuts 1.10.0 (grep of
`.build/checkouts/KeyboardShortcuts/Sources`) is `String.localized` (`Utilities.swift:10`), called only from
`RecorderCocoa.swift:90,149,174,243,256,258` — the recorder's placeholder strings and its alerts. `Shortcut.description`,
`Shortcut(event:)`, `setShortcut`, `getShortcut`, `onKeyDown/Up` and the Carbon registration never touch it.

**Reproduced** (`2ec5a2bb`): new launch flag `--probe-settings <tab>` (`Palette.swift:80-85`, runner
`NotificationsInstaller.swift:113-123`: Settings shown on that tab without activating at +1 s, window number logged for the
shell's `screencapture -l`, quit 0 at +4 s). Bundle built, `mv .build .build.away`, then
`"Theuth Companion.app/Contents/MacOS/Companion" --probe-settings keys --shareable` → **exit 133 (SIGTRAP)**, stderr
`KeyboardShortcuts/resource_bundle_accessor.swift:12: Fatal error: could not load resource bundle: from …/Theuth
Companion.app/KeyboardShortcuts_KeyboardShortcuts.bundle or …/.build/arm64-apple-macosx/release/…`, and a new
`Companion-2026-10-07-184718.ips` with the identical frames (`RecorderCocoa.init(for:onChange:)` ← `String.localized`
← `NSBundle.module`). `.build` restored.

**Fix at the root.** `KeyboardShortcuts.Recorder` is gone from the app (`SettingsWindow.swift` no longer imports
KeyboardShortcuts). Our own recorder:

| Path | What |
|---|---|
| `Sources/Companion/Settings/ShortcutRecorder.swift:17-127` | `ShortcutRecorderView` (AppKit `NSView`): click → first responder with "Type a shortcut…" (`acceptsFirstResponder` is `clicked || recording`, `:41` — the window must not hand it first responder on its own; the first probe run showed exactly that); `keyDown` (`:68`) + `performKeyEquivalent` for ⌘-chords → `Chord(chars: charactersIgnoringModifiers, keyCode, modifiers)` → `record` (`:85`) → `KeyTable.check` → `model.keys.set`; ⌫/⌦ clears, ⎋ cancels, ⇥ moves on, clicking away resigns. Draws the chord's `symbol`, "unassigned" (warning colour only when the row lost its chord to a move), or the prompt |
| `ShortcutRecorder.swift:130-136` | `ShortcutRecorder: NSViewRepresentable` |
| `ShortcutRecorder.swift:139-172` | `KeyRow`: label · recorder (or the fixed chord for ⎋) · reset-to-default; the verdict line under it ("Already used by …" + **Use here instead**, "… is reserved by macOS", "… is held by another app — pick another") |
| `Sources/CompanionCore/KeyBindings.swift:6-70` | `Chord` (AppKit-free): `chars` + `keyCode` + `Modifiers`; printable keys compare by character (layout-independent, ⇧/case-insensitive after `normalizedLocal`), everything else by key code; `symbol` renders "⌃⇧Space" · "⌘O" · "⏎" · "fnF1" from `KeyNames` (`:72-87`, special keys + F1–F20) — no `Shortcut.description` needed |

Rendering is ours, so nothing in the app can reach `Bundle.module` any more.

**Belt and braces that was NOT done** (brief 1c): a copy of `KeyboardShortcuts_KeyboardShortcuts.bundle` at the `.app`
root makes `codesign --verify --deep --strict` fail with `unsealed contents present in the bundle root` (tested, exit 1);
a symlink at the root fails the same way (tested, exit 1). So the script keeps only the `Contents/Resources` copy and now
says why (`scripts/bundle-companion.sh:26-31`).

### 1.2 Ask bar: live dictation into the field (R2-2)

Record-then-transcribe (`put_attachment` + `transcribe`) is gone; the Ask bar streams to the engine's dictation
(design 54 §8; `crates/rpc/src/api.rs:1293-1325`, `crates/voice/src/lib.rs:364-395`).

| Path | What |
|---|---|
| `Sources/CompanionCore/AskVoice.swift:7-77` | `AskVoiceMachine` (the press): idle → pressed → (≥ 250 ms) recording → (key-up or 60 s) **stopping** → `dictated(text)` → `setText` / `fail`; effects renamed `stopDictation`, event `dictated` (W3's `.transcribing`/`.transcript` gone; the W3 tests adapted in name only) |
| `AskVoice.swift:78-168` | `AskDictation` (pure reducer): phase idle → starting → live(id) → stopping(id) → idle; `utterances: [(text, final)]` indexed by the event's `utterance` (append-only; `final` once; an empty closed utterance is dropped); `finalText` · `partialText` · `liveText`; `levels` from `DictationLevel`; `error` ("already dictating" → "dictating elsewhere"; `DictationFinished` with `Error` → its message); `.typed` makes later partials/finish return `false` so typed edits win. `handle` returns "the field should take `liveText`" |
| `Sources/CompanionCore/AskRpc.swift:28-49` | `dictationPrewarm()`, `dictationStart(sampleRate: 16000)` → id (never `None`: that would open the ENGINE's mic), `dictationAudio(id, pcm16:)` (little-endian i16 → base64), `dictationStop(id)`. `Transcript`, `putAttachment`, `transcribe` and `WAV` deleted (no caller; the WAV header test went with it) |
| `Sources/Companion/Voice/VoiceRecorder.swift` | AVAudioEngine tap → `AVAudioConverter` → 16 kHz mono Int16, now cut into 1600-sample (100 ms) chunks handed to `onChunk` on the main thread; `stop()` flushes the tail. Permission logic unchanged (prompt only inside `start()`, reached only from a user's hold) |
| `Sources/Companion/AskBar/AskBarPanel.swift:165-305` | Effects: `startRecording` → `beginDictation()` (`:248`: reducer `.start`, `dictation_start` queued, chunks that arrive before the id are held and flushed in order) + `recorder.start()`; `feed` (`:274`) → `dictation_audio`; `stopDictation` → `recorder.stop()` + `endDictation()` (`:283`: `dictation_stop` after every queued chunk; 5 s cap on "finishing…" if `DictationFinished` never comes). `enqueue` (`:229`) chains the calls so audio and stop stay ordered. `dictationChanged` (`:295`, from `Store.onDictation`): live text into the field (finals in `text`, the open partial in `partial`), `level` for the mic pulse, and on finish → `.dictated(liveText)` / `.failed(error)` → `setText` → the same 3 s hold (`submitAsk`); ⎋ keeps editing, typing cancels the hold and tells the reducer `.typed`. `prewarm()` (`:238`) runs `dictation_prewarm` once when the Ask bar first opens (not at launch) |
| `AskBarPanel.swift:307-330` | `probeDictation(seconds:)` — the `--synthetic-dictation` source: a 440 Hz, 0.3-amplitude sine in 100 ms chunks through `beginDictation` / `feed` / `endDictation`; the recorder (mic) is never started |
| `Sources/Companion/AskBar/Store+Ask.swift:118-165` | `askObserve` routes `Dictation*` to `dictationObserve`: decodes `{type, id, utterance, text, final, levels, reason{reason,message}}` → `ask.dictation.handle` → `onDictation(dictation, takeText)`; `onDictationWire` gives the probe every raw line |
| `Sources/Companion/AskBar/AskBarView.swift` | While `state.live`: a `Text` in the field's place — finals in ink, the open partial muted (`.sand`), "Listening…" when empty; otherwise the `TextField` (its `onSubmit` is gone — ⏎ is the table's `askSend`, so a rebound ⏎ really is rebound). Mic glyph scales with `level`. Hint row: "hold ⌃⇧Space · to talk · release to send in 3 s" rendered from the table's `openPanel` chord |
| `Sources/Companion/AskBar/AskBarInstall.swift:43,61-84` | `--ask-probe N --shareable --synthetic-dictation`: after the desk resolves, `probeDictation(2 s)`; every event to `/tmp/130-w6-dictation.log`; nothing sent |

### 1.3 Rail: "Always a sliver" (R2-3)

| Path | What |
|---|---|
| `Sources/CompanionCore/RailModel.swift:49-84` | `RailPresence` (`expanded` · `sliverWhenQuiet` = W1 · `alwaysSliver`); `RailCollapse.collapseDelay(presence, quiet, hovering, afterHover)` → `nil` stay / `8` s (quiet) / `0` now / `1` s after the pointer leaves an always-sliver rail; `expandsOnActivity` is true only for `sliverWhenQuiet`; widths 26 / 4 |
| `Sources/CompanionCore/PolicySettings.swift:10,22,31-32,40,47,55` | `CompanionSettings.railPresence` (key `railPresence`); W2's `railCollapse` toggle migrates (false → `expanded`, true/absent → `sliverWhenQuiet`); `collapseWhenQuiet` removed |
| `Sources/Companion/RailPanel.swift:42-106` | `RailController`: `railChanged` expands a collapsed rail on activity only when the presence says so; `scheduleCollapse` runs the pure rule (a running quiet timer is left alone, as W1 — not restarted by every refetch); hover cancels the timer and expands; the pointer leaving schedules with `afterHover`; `presenceChanged` (Settings live, via `SettingsModel.$value`) applies at once; at init `scheduleCollapse` makes `alwaysSliver` a sliver from the first frame. Side (left/right) untouched — the user's suite has `railSide = left` already |
| `Sources/Companion/Settings/SettingsWindow.swift:78-85` | Settings › Rail: "Rail presence" picker (Expanded · Sliver when quiet · Always a sliver) + caption; the W2 toggle is gone |

### 1.4 Keyboard shortcuts: one table, a legend, every key rebindable, collisions (R2-4)

| Path | What |
|---|---|
| `Sources/CompanionCore/KeyBindings.swift:89-152` | `KeyScope` (anywhere · panel · askBar), `KeyAction` (22 ids; `intent` maps the local ones to W1's `Intent`), `KeyBinding` (action, scope, label, `defaultChord`, effective `chord`, `fixed`) |
| `KeyBindings.swift:154-215` | `KeyTable`: `defaults` (`:167`) — anywhere: open panel ⌃⇧Space, open Ask bar (none); panel: 1 2 3 ⏎ J K S E D O ⌘O R ⌘L ⌘/ ⌘, ⎋; askBar: ⏎ send, ⌘O, ⌘/, ⎋. `aliases` (`:193`, fixed, shown in the legend): ↓/↑, keypad ⏎, `?` → legend. `overrides: [KeyAction: Chord?]` (null = unassigned); `bindings(in:)`, `action(for:in:)` (local chords normalized), `isUniquePerScope` (`:218`) |
| `KeyBindings.swift:225-262` | The collision policy: `check(chord, for:, reserved:)` → `.unchanged` / `.reservedByMacOS` (global scope: a `CopySymbolicHotKeys` chord, or no modifier but ⇧ and not an F-key) / `.usedBy(action)` (same scope) / `.ok`; `move` = "Use here instead" (the other row goes unassigned); `set` / `reset` / `resetAll` |
| `KeyBindings.swift:264-275` | Persistence: `json` / `init(json:)` = `{"<actionId>": chord | null}` under `CompanionSettings.keys` → UserDefaults key `keyOverrides` (`PolicySettings.swift:17,40,55`) |
| `KeyBindings.swift:277-294` | `resolveGlobal(saved:default:canRegister:)`: the saved chord if it registers, else the default with the notice "⌃⌥K is held by another app — using ⌃⇧Space", else none with "… no hotkey until you pick another" |
| `Sources/CompanionCore/KeyGrammar.swift:5-51` | `KeyPress` gains `modifiers` (the `command:` initializer kept for W1's tests); `Intent` gains `legend`, `settings`, `send`; `KeyGrammar.intent(for:scope:table:textFieldFocused:)` is a table lookup — defaults reproduce W1/W3 exactly (`CompanionCoreTests.swift:161-176` and `AskTests.swift:66-72` pass unchanged); `textFieldFocused` lets only ⎋ through (the send-back note and the R field type, they do not act) |
| `Sources/Companion/Settings/Hotkeys.swift` | The `anywhere` scope: `KeyboardShortcuts.Name.openPanel` (default ⌃⇧Space) + `.openAskBar` (none); `start` (`:22`, at launch before any handler registers): `resolveGlobal` per global row with `canRegister`, persists the fallback, publishes `SettingsModel.hotkeyNotice`; `apply` (`:35`, on every table change from `SettingsModel.didSet`): `KeyboardShortcuts.setShortcut` from the table — the table is the source of truth, `KeyboardShortcuts_openPanel` is rewritten from it; `reserved` (`:67`, `CopySymbolicHotKeys`, enabled entries — 169 on this machine); `canRegister` (`:79`, an exclusive `RegisterEventHotKey` probe released at once; see §3) |
| `Sources/Companion/Settings/Legend.swift:9-116` | `LegendController`: a third `GlassPanel` (360 pt, sized to its rows, ≤ 760), non-activating, under the status item (or top-centre of the mouse's screen), closes on ⎋ · ⌘/ · resign-key, returns key to the previous app; `runProbe` for `--probe-legend` |
| `Legend.swift:119-173` | `LegendView`: the three scopes FROM the table (label · chord · aliases, "unassigned" muted; warning colour when lost to a move), the Ask bar's "Hold to talk · release to send in 3 s — hold ⌃⇧Space" from the `openPanel` chord, the hotkey notice, the footer "Panel and Ask bar keys fire only while that surface is key, so they never collide with other apps — which is why the global set stays tiny" and **Change in Settings… ⌘,** |
| `Sources/Companion/AppDelegate.swift:24-39,144-151` | `Hotkeys.start` before the handlers; `onKeyUp(.openAskBar)` toggles the Ask bar; `Store.onLegend` / `onSettings`; menu item **Keyboard shortcuts… ⌘/** after "Open theuth"; `showLegend` closes the panel/Ask bar and toggles the legend under the status item; `--probe-legend` |
| `Sources/Companion/AttentionPanel.swift:141-147` · `AskBar/AskBarPanel.swift:107-126` | Both key monitors look the chord up in the table for their scope (`.send` → submit, `.legend`, `.settings`); the text-field rule moved into `KeyGrammar` |
| `Sources/Companion/Store.swift:57-64,190-192` | `onLegend` / `onSettings` hooks; `perform(.legend / .settings)` |
| `Sources/Companion/Settings/SettingsWindow.swift:91-137` | Settings › Keys: hotkey notice (orange, one line), one `KeyRow` per table row grouped by scope, the collision explanation, **Reset all to defaults**, then W2's double-tap ⌥ and undo-delay controls |
| `Sources/Companion/Settings/SettingsStore.swift` | `SettingsModel.keys` (get/set through `value`), `hotkeyNotice`, `rowNotice: [KeyAction: RowNotice]`; `didSet` pushes table changes to `Hotkeys.apply` |
| `Sources/Companion/Notifications/NotificationsInstaller.swift:108-149` | `--probe-settings <tab>`; on `keys` it also drives the recorder without key events (`recordProbe`, §2.4) and restores the user's binding |

Tests (`Tests/CompanionCoreTests/W6Tests.swift`, 14 new; `PolicyTests.swift:234-265` extended): table round-trip and
uniqueness (`:8`), lookup by chars/codes/aliases and ⌥J ≠ J (`:19`), override wins → reset → unassigned persists as null
→ local chords stored lower-cased without ⇧ (`:32`), the collision policy incl. a stubbed reserved list, bare keys, F5,
"Use here instead" (`:52`), registration-failure fallback with a stubbed registrar (`:72`), chord symbols (`:84`), text
fields type instead of acting + ⏎/⌘//⌘, intents (`:94`); rail widths, the three presences, no auto-expand on needs-you in
`alwaysSliver` (`:104-129`); `AskDictation` start/live/stop/idle with partial/final/empty utterances, foreign ids, levels
(`:132`), typed edits win over late partials (`:156`), errors in the hint row (`:169`); `CompanionSettings` round-trip with
`railPresence` + `keys` and the `railCollapse` migration.

## 2. Verification

`BUILD_LABEL=130-w6 build-lock ./test.sh`: **81 tests in 18 suites passed** (W1–W3's 67 + 14). `scripts/bundle-companion.sh`:
`Theuth Companion.app` 2.12.0, `codesign --verify --deep --strict` "valid on disk · satisfies its Designated Requirement".
All probes ran from the in-bundle executable (`"…/Theuth Companion.app/Contents/MacOS/Companion" … --shareable`) so the
exit code is observable; `/tmp/130-w6-probe.log` is the W6 probe log, screenshots from the shell with `screencapture -x -o -l
<window number from the log>` (the app has no Screen Recording grant, as W2 found). Mouse-transparent, never key.

### 2.1 Keys tab with `.build` moved away (item 1d) — exit 0, recorder on screen

`mv .build .build.away` → `--probe-settings keys --shareable` (19:08:49) → `exit=0`, window `number=3181 active=false
key=none`, `probe-settings done … active=false frontmost=theuth — quitting 0`; `.build` restored. Screenshot
`/tmp/130-w6-settings-keys.png`: Rail · **Keys** · Notifications · Engine tabs; ANYWHERE "Open the panel · hold to talk
⌃⌥K" (the probe's recording, below), "Open the Ask bar unassigned" with the orange line "⌘Space is reserved by macOS";
ATTENTION PANEL rows 1 2 3 ⏎ J K S … each with its reset arrow. The first run of this build, before `acceptsFirstResponder`
was gated on a click, showed the first recorder already in "Type a shortcut…" (the window had made it the initial first
responder) — fixed in `74ff11c1`, the two re-runs show the chord.

### 2.2 The recorder records, persists, refuses (items 1d, 4e) — `recordProbe`, same run

```
probe-settings recorded ⌃⌥K for openPanel → table=⌃⌥K notice=none | defaults keyOverrides={"openPanel":{"modifiers":3,"keyCode":40,"chars":"k"}} KeyboardShortcuts_openPanel={"carbonKeyCode":40,"carbonModifiers":6144}
probe-settings recorded ⌃⌥K for openAskBar → table=nil notice=Already used by Open the panel · hold to talk moveOffered=true
probe-settings recorded ⌘Space for openAskBar → table=nil notice=⌘Space is reserved by macOS reserved-list=169 chords
probe-settings table unique-per-scope=true overrides={"openPanel":{"modifiers":3,"keyCode":40,"chars":"k"}}
probe-settings restored openPanel → ⌃⇧Space KeyboardShortcuts_openPanel={"carbonKeyCode":49,"carbonModifiers":4608} keyOverrides={}
```

Read back from the shell afterwards: `defaults read io.theuth.mac.companion KeyboardShortcuts_openPanel` =
`{"carbonKeyCode":49,"carbonModifiers":4608}` (⌃⇧Space, as before the probe), `keyOverrides` = `{}`. The chord went to
both stores (the table under `keyOverrides`, and — through `Hotkeys.apply` — KeyboardShortcuts' own key, carbon modifiers
6144 = control 4096 + option 2048, so the Carbon registration followed it); the same-scope collision was refused with the
owner's label and "Use here instead" offered; ⌘Space was refused as reserved (Spotlight is in the 169 enabled symbolic hot
keys). The hotkey itself was not pressed: the user's installed companion is running (pid 93375) and holds ⌃⇧Space too, so a
synthesized key event would have gone to one of two apps — the registration is proven by the stored `KeyboardShortcuts_openPanel`
value and `Hotkeys.apply`'s `setShortcut` path, which is the same code W1 registered through.

### 2.3 Legend (item 4e) — `--probe-legend --shareable`

19:09:55: `rows=22 unique=true`, `shown frame=(1100.0, 549.0, 360.0, 579.0) … key=false active=false`, done `active=false`,
exit 0. `/tmp/130-w6-legend.png`: "Keyboard shortcuts · ⎋ close"; ANYWHERE (⌃⇧Space · unassigned), ATTENTION PANEL (… "Answer
with the recommended option ⏎ · ⌅", "Next item J · ↓", "Keyboard shortcuts ⌘/ · ?", "Settings ⌘,", "Undo the hold, else close
⎋"), ASK BAR (… "Hold to talk · release to send in 3 s — hold ⌃⇧Space"), the footer line and "Change in Settings… ⌘,". The
first run was clipped at a 560 pt cap (the header and footer cut); the cap is now 760 and the panel sizes to its rows.

### 2.4 Rail presence (item 3) — W1's `--probe 4 --shareable` with the suite key set and unset

With `defaults write io.theuth.mac.companion railPresence alwaysSliver` (19:10): `rail: needsYou=24 working=6` and
`rail frame=(0.0, 660.0, 4.0, 120.0)` — **4 pt at the left edge with 24 items needing you and 6 agents working**. Key
deleted again (`defaults read … railPresence` → does not exist), re-run: `rail frame=(0.0, 409.0, 26.0, 341.0)` — 26 pt,
the migrated W2 default (`railCollapse = 1` → `sliverWhenQuiet`). Side stayed `left` (the user's setting). Hover
expansion and the 1 s linger are the pure rule's (`RailPresenceTests`); not driven live (no synthetic mouse).

### 2.5 Live dictation against the engine (item 2d) — `--ask-probe 9 --shareable --synthetic-dictation`

`/tmp/130-w6-dictation.log` (18:10:43 UTC = 19:10 local), 2 s of a 440 Hz / 0.3-amplitude tone, 20 × 1600 samples:

```
dictation_prewarm ok · dictation_start → 01a1178f-822d-7c91-86a5-593a8a6c111b
event {"type":"DictationStarted","id":"01a1178f-…"}
event {"type":"DictationLevel","id":"01a1178f-…","levels":[0.2124,0.2108,0.2139,0.2105,0.2130,0.2124,0.2108,0.2139]}   × 13 (≈ 6/s)
event {"final":true,"id":"01a1178f-…","utterance":0,"type":"DictationText","text":""}
stop sent; waiting for DictationFinished · dictation_stop(01a1178f-…) ok
event {"type":"DictationFinished","id":"01a1178f-…","reason":{"reason":"done"}}
after stop: phase=idle utterances=[Utterance(text: "", final: true)] liveText="" error=nil
```

Read: the whole path is live — prewarm, start with our sample rate, 100 ms base64 PCM chunks in order, stop, finish.
`DictationLevel` reads RMS ≈ 0.212 = 0.3/√2: the engine received the sine intact. `DictationText` is one closed, empty
utterance — a pure tone is not speech, and the reducer dropped it (`liveText=""`), which in the real flow is the
"Nothing recognized" hint. The microphone was never opened (the recorder is not started on this path), nothing was sent
(`receipts=0`), the app never became active. No speech fixture exists under `crates/` (no `.wav`/`.flac`/`.ogg`), so the
word-level rendering (finals ink, partial muted) is covered by `AskDictationTests` and the view code, not by a live run;
the engine neither answered `voice is disabled` nor a missing model.

## 3. Deviations and findings

- **Carbon does not report plain registrations.** The brief assumed `RegisterEventHotKey` answers `eventHotKeyExistsErr
  (-9878)` when another app holds the chord. Measured on this machine (macOS 26.6.2, `/tmp/hk2.swift`, two processes): a
  holder that registered WITHOUT `kEventHotKeyExclusive` is invisible — an exclusive probe returns 0; only when the holder
  itself registered exclusively does the probe get -9878. KeyboardShortcuts (ours and most apps') registers plain, so with the
  installed companion holding ⌃⇧Space the launch probe still said "registrable" and no fallback fired. `Hotkeys.canRegister`
  keeps the exclusive probe (it catches exclusive holders) and the fallback/notice path is tested with a stub; the as-designed
  "held by another app" line therefore appears only for exclusive holders. There is no API for the rest — which the legend
  footer already says, and why the global set is two rows.
- **1c not done** (copy/symlink at the `.app` root fails strict codesign — measured, §1.1); the fix is in the app instead.
- **⎋ is `fixed`** in both local scopes: the recorder uses it to cancel and every surface closes on it; shown in Settings as a
  non-editable chord. Every other row — including ⏎, ⌘,, ⌘/ and the global two — is rebindable.
- **Local chords ignore ⇧ and case** (W1's `K` = `previous`, kept), so ⇧-chords cannot be local bindings; global chords keep ⇧.
- **The default-unassigned Ask-bar hotkey** renders "unassigned" muted; only a row that LOST its chord to "Use here instead"
  renders it in the warning colour (brief 4d).
- **`dictation_prewarm`** runs when the Ask bar first opens (brief 2c), not at launch, so a companion that is never asked
  never loads the model.
- **The W3 WAV helper, `put_attachment` and `transcribe`** were deleted with their only test (a header check of now-dead code).
- **The shared suite.** The ad-hoc build and the installed app share `io.theuth.mac.companion`. The Keys probe writes
  `keyOverrides`/`KeyboardShortcuts_openPanel` and restores them; the rail check wrote `railPresence` from the shell and
  deleted it. The installed (pre-W6) app ignores both keys. Nothing else was written (`writeLoginItem` only when the file is
  missing; `Hotkeys.apply` only on a difference).
- **One activation seen**: on the very first keys-probe build, `screencapture -l` of the Keys window made the app active
  (`became active … frontmost=Theuth Companion`) once; that build also had the recorder as the window's initial first
  responder. After gating `acceptsFirstResponder` on a click, four probe runs (keys ×3, legend) stayed `active=false`
  throughout, as did every run without `screencapture`.
