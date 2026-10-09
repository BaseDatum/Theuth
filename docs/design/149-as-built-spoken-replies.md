# 149s as-built — spoken replies as two opt-in settings; "Ask theuth" replies spoken; the panel key opens the panel

Branch `fix/149-spoken-replies` off `origin/main` ece44049, worktree `theuth-149s`. Swift only; companion package.

## The owner's words (2026-10-09 15:58)
"when I do ask theuth and I dictate, the response doesn't get spoken aloud, but honestly we need a setting for the voice key
response spoken aloud and the ask theuth spoken aloud, those need to be options the user can use if desired, not always on."

16:02 (Addendum K): "Open the panel and open the ask bar I have set to two different keyboard shortcuts, and they open the same thing…?"

## Rulings
- **S1** Two independent opt-in settings, both OFF by default: `voiceKeySpeak` (existing key, default flipped true → false) and
  `askSpeak` (new). Persisted in `Settings.defaults`.
- **S2** One place: a "Spoken replies" section in the Rail tab (the Ask bar has no settings elsewhere), two plain system Toggles
  with one sentence each. The Voice key tab's existing "Speak the answer" row stays and binds the same key — one source of truth.
- **S3** The Ask reply is spoken through the voice key's path (`client.speak` → `SpeechAudio` → `SpeechPlayer`), typed and dictated
  alike, once the reply is final; partials, empty and failed turns say nothing. Esc on the Ask bar, a new ask, or a new voice-key
  gesture stops the current speech. One shared `Speaker` holds the one "currently speaking" id for both surfaces.
- **S4** While speaking, the surface that asked shows `speaker.wave.2.fill` in the accent colour; clicking it stops.
- **K1** The panel key always toggles the Attention panel on a tap (empty state included) and talks on a hold, never the Ask bar;
  the Ask bar key toggles the Ask bar only.

## What changed
| File | Change |
|---|---|
| `Sources/CompanionCore/SpokenReplies.swift` (new) | `SpokenReplies` — keys `voiceKeySpeak` / `askSpeak`, `voiceKey(_:)` / `ask(_:)` read `UserDefaults.bool` (missing = false), `shouldSpeak(enabled:reply:)` = enabled ∧ finished ∧ no failure ∧ non-empty (S3). `SpeechClient` protocol (`speak`, `stopSpeaking`; `extension RpcClient: SpeechClient {}`), `SpeechSink` protocol (`expect`, `stop`). `Speaker` (@MainActor, ObservableObject): `speak(_:on:from:)` stops whatever is playing first, bumps a generation, `sink.expect`, asks the engine; an id landing after a `stop` is `stop_speaking`'d at once; `stop()` silences the sink and sends `stop_speaking(id)`; `ended(id)` from the sink clears `surface`. `@Published surface: Surface?` (`.voiceKey` / `.askBar`) is what the glyphs bind to. |
| `Sources/CompanionCore/AskVoice.swift:77-92` | `SurfaceKey` (K1): `.panel.action(held:)` → `.togglePanel` / `.talk`; `.askBar` → `.toggleAskBar`. |
| `Sources/Companion/PointAndSay/SpeechPlayer.swift` | Conforms to `SpeechSink`; `observe(_ data:)` decodes `SpeechStarted` / `SpeechAudio` / `SpeechFinished` itself (`:33-47`); `volume` (probe mute) applied to the player node (`:63`); `decoded` packet count (`:26`, `:69`). |
| `Sources/Companion/Store.swift:49-57, 91, 118-119` | `let speech = SpeechPlayer()`, `lazy var speaker = Speaker(sink: speech)` with `speech.onEnded → speaker.ended`; `speaker.client = client` in `start`; `handle` feeds `speech.observe(data)` before `pointObserve`. |
| `Sources/Companion/AskBar/Store+Ask.swift:51-60, 64-65, 140-143` | `submitAsk` → `speaker.stop()` first (a new ask silences); `askSpeakEnabled` = `--probe-speak` override ∨ `SpokenReplies.ask(Settings.defaults)`; `commitAsk` refuses to send under any probe (`AskFlags.automated`); `askObserve`: on `reply.finished`, `shouldSpeak` → `speaker.speak(reply.text, on: reply.session, from: .askBar)` — the same `AskReply` the receipt row shows. |
| `Sources/Companion/AskBar/AskBarPanel.swift:34, 114, 143-163, 186-191` | `AskBarView` gets `store.speaker`; `.escape` → `store.speaker.stop()` before undo/close; K1: `hotkeyDown`/`hotkeyUp` feed `AskVoiceMachine` only; `.tap` → `perform(.togglePanel)` = `attention.toggle(anchor: .mouse)`, `.startRecording` → `perform(.talk)` opens the Ask bar if it is not up, then dictates; `openedByPress` removed. |
| `Sources/Companion/AskBar/AskBarView.swift:30-31, 73-74` | `@ObservedObject speaker`; `SpeakingGlyph { speaker.stop() }` in the hints row while `speaker.surface == .askBar` (S4). |
| `Sources/Companion/PointAndSay/AnswerCard.swift:16-17, 32, 70-82, 83-86` | `SpeakingGlyph` (shared: SF `speaker.wave.2.fill`, `.accentColor`, `.plain` button, "Click to stop"); the card's glyph is that button → `onStop`. |
| `Sources/Companion/PointAndSay/PointController.swift:59-60, 71, 104-107, 197, 479, 503, 506-516, 529-538` | Own `SpeechPlayer` / `speakingId` removed; `speaker` = `store.speaker`; `card.state.speaking` mirrors `speaker.$surface == .voiceKey` and `armDone` when ours ends; `.start` → `speaker.stop()` (any surface); `.conceal`/⎋ → stop only if ours; reply → `SpokenReplies.shouldSpeak(enabled: VoiceKeySettings.speak, …)`; `VoiceKeySettings.speak` = `SpokenReplies.voiceKey(Settings.defaults)` (default false); `speakKey` dropped. |
| `Sources/Companion/Settings/SettingsWindow.swift:55-57, 93-98` | Rail tab: `Section("Spoken replies")` — "Voice key — speak the answer aloud" / "Read the desk's answer aloud after a voice-key gesture." and "Ask theuth — speak the reply aloud" / "Read the desk's reply aloud after an Ask."; `@AppStorage` on `SpokenReplies.voiceKeyKey` / `askKey`, default false. |
| `Sources/Companion/PointAndSay/VoiceKeyTab.swift:13, 31-32` | "Speak the answer" binds `SpokenReplies.voiceKeyKey`, default false; caption names the Rail section. |
| `Sources/Companion/AppDelegate.swift:30-36` | Comment states K1; key-up still routes to `askBar.hotkeyUp()` (now the hold machine's key-up). |
| `Sources/Companion/AskBar/AskBarInstall.swift:44, 148-201` | `--probe-speak` (with `--ask-probe N`): `AskProbe.speakScenario` (below). |
| `Store+Ask.swift:225-226` | `AskFlags.probeSpeak`. |
| `Tests/CompanionCoreTests/SpokenRepliesTests.swift` (new) | 11 tests (below). |
| `docs/design/130-as-built-W3.md:34` | The old hotkey rule marked superseded by K1. |

Legend text and the key table's label ("Open the panel · hold to talk", "Hold to talk · release to send in 3 s") already describe K1; unchanged.

## Defaults and migration (S1)
`UserDefaults.object(forKey:) == nil` does tell "never set" from "set": `@AppStorage` only writes on a user change, and the old
`VoiceKeySettings.flag` read the same way. So a user who never touched "Speak the answer" now gets `false`; a user who explicitly
toggled it (even back to on) keeps the stored value. `SpokenReplies.voiceKey/ask` read `bool(forKey:)`, which is `false` for a
missing key — the same outcome in one line. No migration code; nothing is written at launch.

## Tests (`SpokenRepliesTests.swift`)
- `SpokenRepliesSettingsTests`: both false on an empty isolated suite; independent; an explicit value is kept.
  `askSpeakDecision`: on/off × final / partial / empty / failure.
- `SpeakerTests` (fake `SpeechClient` recording `speak`/`stopSpeaking`, gated reply; fake sink): speak → `speak(session,text)`
  once, id adopted, surface set; `stop()` → `stop_speaking(id)` exactly once, a second `stop` sends nothing; a new speak from the
  other surface stops the first id before starting; an id landing after `stop` is not adopted and is stopped; playback ending clears
  the surface without a `stop_speaking`; a failed `speak` leaves the sink not expecting.
- `SurfaceKeyTests` (K1): a 0.1 s press through `AskVoiceMachine` is `.tap` → `.togglePanel` (never the Ask bar, whatever the
  inbox holds); a press reaching `holdThreshold` is `.startRecording` → `.talk`; the Ask bar key → `.toggleAskBar` tap or hold.

## Probe evidence (in-worktree bundle executable, `--shareable`; the owner's companion pid 60555, desktop app, engine and
`Settings.defaults` untouched — `defaults read io.theuth.mac.companion askSpeak` → "does not exist" before and after)
`Theuth Companion.app/Contents/MacOS/Companion --ask-probe 12 --probe-speak --shareable` → exit 0, `/tmp/149s-speak-probe.log`:
```
SPEAK: askSpeak=true (override, defaults untouched: askSpeak key=unset) desk=01a10df6… muted=true
partials fed → speaks=0 surface=nil
speaker: speak(askBar) 52 chars on 01a10df6…
speaker: speak → 01a12144-c0f7-7d12-9198-c78c2039e358
final fed → speaks=["speak(askBar) 52 chars on 01a10df6…"] id=01a12144-… surface=Optional(…askBar) playing=true decoded=11 receipt=Optional(…reply(who: "theuth", text: "Probe reply: the spoken-reply path works end to end.", …))
ASSERT speak once with the final text, none for partials: PASS
ASSERT SpeechAudio received and decoded: PASS (11 packets)
speaker: stop 01a12144-c0f7-7d12-9198-c78c2039e358
second ask → surface=nil playing=false stops=["stop 01a12144-…"] receipt=Optional(Companion.Store.Receipt.undone)
ASSERT a second ask stops it (stop_speaking for its id): PASS
SPEAK done — nothing sent (receipts=0)
```
The fixture reply (`TextDelta` ×3, `TurnFinished end_turn`) goes through `askObserve`, the path a real reply takes; `send_input` is
never called (and `commitAsk` refuses under a probe). `speak` goes to the live engine; its Opus packets decode here into a player
node at **volume 0** (muted, not played — the choice: a probe must never sound on the owner's desk). The second ask is
`submitAsk` + `.escape` (the hold is undone; receipt `.undone`).

Settings: `--probe-settings rail --shareable` → exit 0, window 11716, `/tmp/149s-settings-spoken.png`: Rail tab with "Spoken replies",
both toggles off. (The "Rail presence" label's left clipping at 500 pt is pre-existing, not this change's.)

## Gates (all `env BUILD_LABEL=149s build-lock …`)
- `bash crates/desktop/companion/test.sh` — **200 tests in 33 suites passed** (189 + 11); the Speaker suite stress-looped 15× green.
- `swift build -c release` (through `scripts/bundle-companion.sh`) — Build complete.
- `bash scripts/bundle-companion.sh` + `codesign --verify --deep --strict "target/release/companion/Theuth Companion.app"` — valid on disk · satisfies its Designated Requirement.
- hex grep (`0x[0-9a-fA-F]{6}|#[0-9a-fA-F]{6}` over Sources, excluding `OverlayModel.hex`'s own formatter) — clean; emoji grep over
  Sources/Tests — only the pre-existing ✕ / ★ / ‹› glyphs in comments and strings, none from this change.

## Deviations
- The brief's `--probe-ask` is `--ask-probe N` in the code; the speak scenario is `--probe-speak` alongside it.
- The probe mutes the player (volume 0) rather than playing ≤ 1 s at low volume.
- `commitAsk` now refuses to send under any probe (`AskFlags.automated`) — a safety net the second-ask probe step relies on in
  addition to the undo; no user-facing change.
- The K1 unit test is of the pure routing (`AskVoiceMachine` + `SurfaceKey`); `AskBarController`/`PanelController` are AppKit
  and not reachable from `swift test`.
- Voice-key ⎋/conceal stops only the voice key's own utterance; a new voice-key gesture stops any (S3). The Ask bar's Esc stops any.

## Live check (3 steps)
1. Settings › Rail › Spoken replies: turn on "Ask theuth — speak the reply aloud". Open the Ask bar, hold the panel key and ask
   something by voice (or type it) → when the desk's reply lands in the receipt row it is spoken; the speaker glyph shows in the
   Ask bar's hints row.
2. Esc on the Ask bar (or click the glyph, or send another ask) → the speech stops at once.
3. Voice key (⌃S) → ask something → the answer card shows the text but stays silent until "Voice key — speak the answer aloud"
   (same toggle as Voice key › "Speak the answer") is on.

## §K live check
Bind "Open the panel · hold to talk" and "Open the Ask bar" to two chords (Settings › Keys). With an empty inbox: tap the panel
key → the Attention panel ("Nothing needs you…") toggles; tap the Ask bar key → the Ask bar toggles; hold the panel key → the Ask bar
opens listening and the release sends.
