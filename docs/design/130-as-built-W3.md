# 130 — as-built W3: the Ask bar (typed and spoken requests via theuth's desk), receipts, R on cards

Status: built 2026-10-07 on `feat/130-w3-askbar` (cut from `integrate/130` at `54962df8`), against TODAY's engine
(2.10.0, no W0 fields, pre-129). Design: `docs/design/130-companion.md` §0 D6, D7 (Sky's R4: always theuth's desk —
no agent picker, no ⌘1–9, no "New agent"), §4.3, §5.4; Revision 1 R1-5 (Decision is Allow|Deny: R on an approval =
Deny + a note); mock round 2 frame 4. Toolchain as W1: Command Line Tools only, Swift Testing through `test.sh`,
KeyboardShortcuts pinned 1.10.0 (it has `onKeyDown` + `onKeyUp`, so no `NSEvent` monitor was needed for the hold).

## 1. File map (path:line)

New files (owned by W3):

| Path | What |
|---|---|
| `Sources/CompanionCore/AskRouter.swift:5-33` | `MeshStatus` subset of `mesh_status` (`crates/mesh/src/types.rs:94-202`): `isHub` = `lease.holder == self_id`, `hubName` |
| `AskRouter.swift:35-89` | `AskRouter`: desk state (`unknown` · `ready(session, hub)` · `unavailable(reason)`), `plan(mesh:agents:)` → `openHere` (hub, or no mesh) / `use(session)` (off-hub, theuth's desk replicated in `agents_list`) / `refuse("theuth's desk lives on <hub> — open theuth there")`; `sent(on:)` + `observe(ReplyEvent)` collect the desk's `TextDelta`s until a `TurnFinished` with text (an empty-text `TurnFinished` is the turn we steered into, not the reply) |
| `AskRouter.swift:97-112` | `AskReply.firstTwoLines`: first two non-empty lines, bullets/heading marks stripped, joined " — " |
| `AskRouter.swift:115-123` | `AskModel` — the one `@Published` value the store carries: router, receipts, `replyFor`, `current` |
| `AskRouter.swift:125-152` | `Card.replyAction(text)` (brief item 5): ask → `answer_ask(id, [{kind:text,text}])` (one question only), decision → `task_decide(decision, text)`, approval → `.denyWithNote` (`resolve_approval(Deny)` + `steer(session, note)`), review → `task_move(id, "todo", note)`, finished → `send_input(session, text)`; `replyLabel` |
| `Sources/CompanionCore/ReceiptModel.swift:4-28` | `ActivityEntry` (`tasks/src/activity.rs:94-111`), `ActivityFilter` (`:117-125`, encodes `{since_ms, limit}`) |
| `ReceiptModel.swift:30-56` | `Receipt` (`ask:<uuid>` local · `activity:<id>` engine); `init?(activity:)` keeps only `actor == user@companion` (W0 E4) |
| `ReceiptModel.swift:58-97` | `ReceiptModel`: newest first, dedupe by id, 24 h window, `clear` persists across merges, `update` for the streaming text |
| `Sources/CompanionCore/AskVoice.swift:6-76` | `AskVoiceMachine`: idle → pressed → (≥ 250 ms) recording → (key-up or 60 s) transcribing → `setText` / `fail`; a press under 250 ms is `.tap` (the hotkey's toggle) |
| `AskVoice.swift:78-92` | `WAV.encode(pcm16:)`: 16 kHz mono 16-bit RIFF (`put_attachment` ext `wav`) |
| `Sources/CompanionCore/AskRpc.swift` | `Transcript` (`voice/src/lib.rs:103-108`) + the W3 calls on `RpcClient`: `steer(session, text, null, [], null)`, `put_attachment(b64, ext)`, `transcribe(path)`, `mesh_status`, `desk_open(name)`, `activity_list({…})`, `set_event_focus([…])` |
| `Sources/Companion/AskBar/Store+Ask.swift:14-40` | `performAsk`: R (only when the kind takes words), ⌘L → `onAskBar`, ⎋ closes the R field, E / ⌘O on a focused Done-zone row |
| `Store+Ask.swift:42-58` | `submitReply` / `submitAsk`: both go through W1's `Store.hold` (same `UndoQueue`, 3 s, ⎋ undo, same receipt row); an ask is `Held(itemId: "", who: "theuth", label: text, action: .askTheuth)` |
| `Store+Ask.swift:60-99` | `commitAsk` (fires from `Store.commit` after the hold) → `resolveDesk` (cached per session: `mesh_status` → `desk_open("theuth")` on the hub) → `send_input(desk, text, [], null, null)` → `router.sent(on:)`, a local receipt, the row says "theuth · sent — waiting for the desk" |
| `Store+Ask.swift:104-134` | `focusDeltas` (`set_event_focus([desk])`, 129 §3.2; re-applied on every `PendingPrompts`; pre-129 engines answer -32601, ignored) and `askObserve(data)`: raw events → `TextDelta`/`TurnFinished` on the desk → receipt row `.reply(theuth, two lines, session)` + the Done-zone row; expires 12 s after the turn finishes |
| `Store+Ask.swift:136-155` | `askRefetchReceipts`: `activity_list({since_ms: now−24 h, limit: 200})` after every refetch → `ReceiptModel.merge` → `panel.receiptCount` |
| `Store+Ask.swift:157-183` | `AskLog` (stderr + `/tmp/130-w3-probe.log` under `--ask-probe`), `AskFlags` (`--ask`, `--ask-probe N`, `automated`) |
| `Sources/Companion/AskBar/AskBarPanel.swift:11-100` | `AskBarController`: a second `KeyPanel` (480 pt, `GlassPanel.make` = the Attention panel's glass/level/collection behaviour/sharing, `:250-292`), `open()` (remembers the frontmost app, key, local key monitor), `showWithoutKey(text:)` (probe: not key, mouse-transparent), `close(returnKey:)` re-activates the previous app; ⌘L closes the Attention panel first |
| `AskBarPanel.swift:105-133` | Keys: only ⎋ (undo during the hold — the text stays in the field — else close) and ⌘O (open the reply's session, else the desk) are intercepted; ⏎ is the field's `onSubmit`; typing during a dictated hold cancels it (`typedDuringVoiceHold`) |
| `AskBarPanel.swift:136-157` | The hotkey: `hotkeyDown` opens the Attention panel when needs-you > 0, else the Ask bar (and starts the press); `hotkeyUp` is W1's toggle unless a voice press is live; a press that opened a surface never closes it on release |
| `AskBarPanel.swift:159-219` | Voice effects: `startRecording` → `VoiceRecorder.start()`; `stopAndTranscribe` → WAV → `put_attachment(…, "wav")` → `transcribe(path)` → `setText` → the field + `submitAsk` (the same 3 s hold); failures land in the hints row |
| `AskBarPanel.swift:221-228` | Menu "Ask theuth…" inserted after "Open theuth" |
| `Sources/Companion/AskBar/AskBarView.swift` | `AskBarState` (text, recording, status, `voiceText`), `AskBarView`: caret glyph · "Ask theuth…" · mic (carnelian while recording); hints "⏎ send to theuth · it routes" · "hold ⌃⇧Space to talk" · "⎋ cancel/undo"; W1's `ReceiptView` beneath (480 pt countdown bar) |
| `Sources/Companion/AskBar/AskPanelViews.swift` | `AskItemLines` (the items as lines while the focus is in the Done zone), `DoneZone` ("Done · last 24 h", ≤ 4 rows windowed around the focus, malachite focus bar, E / ⌘O hints), `ReplyField` (R: one line, ⏎ sends after the hold, ⎋ cancels, empty refused) |
| `Sources/Companion/AskBar/AskBarInstall.swift` | `AppDelegate.installAskBar(attention:)`: the controller, `KeyboardShortcuts.onKeyDown`, `--ask` / `--ask-probe`; `AskProbe.run` |
| `Sources/Companion/Voice/VoiceRecorder.swift` | `AVAudioEngine` input tap → `AVAudioConverter` → 16 kHz mono Int16; `AVCaptureDevice.requestAccess(.audio)` only inside `start()` (reached only from a user's hold — `AskFlags.automated` blocks the hotkey); refuses to run without `NSMicrophoneUsageDescription` in the bundle; < 0.2 s of audio = "Nothing recorded" |
| `Tests/CompanionCoreTests/AskTests.swift` | 10 tests: hub rule, `plan` (hub · no mesh · off-hub replicated desk · refusal names the hub), reply collection (other sessions ignored, early `TurnFinished` ignored, two-line extraction, finished stays put), `replyAction` per kind, grammar R/⌘L, Done-zone focus walk, voice tap/hold/empty/error/cap, WAV header |
| `Tests/CompanionCoreTests/ReceiptTests.swift` | 6 tests: ordering, actor filter, dedupe + streaming update, 24 h window (on arrival and over time), cleared stays cleared, wire decode of `ActivityEntry` + `ActivityFilter` encoding |
| `docs/design/130-as-built-W3.md` | this file |

Shared files (smallest additive edits, every one tagged `// W3`):

| Path:line | Edit |
|---|---|
| `Sources/CompanionCore/KeyGrammar.swift:29-30, 61-62` | `Intent.reply` (R), `Intent.askBar` (⌘L) |
| `Sources/CompanionCore/CardModel.swift:16-19` | `CardAction.denyWithNote`, `CardAction.askTheuth` |
| `Sources/CompanionCore/PanelModel.swift:9-10, 24-28, 33-37, 48, 63` | `receiptCount`; `move` ranges over items + receipts; `receiptFocus`; `focused` is nil in the Done zone; `position` stays "n of items" |
| `Sources/Companion/Store.swift:23-24` | `Receipt.reply(who, text, session)` |
| `Store.swift:39-40, 57-58` | `@Published var ask = AskModel()`, hook `onAskBar` |
| `Store.swift:43, 204, 272` | `client` readable (`private(set)`), `hold` and `expireReceipt` internal — the extension file reuses them, nothing is duplicated |
| `Store.swift:92, 131, 157, 184` | `askObserve(data)` at the end of `handle`; `askRefetchReceipts()` at the end of `apply`; `if performAsk(intent) { return }` first in `perform`; the two new intents in the switch |
| `Store.swift:233, 256-259` | `commit`: an `.askTheuth` hold goes to `commitAsk` (no item, so no E5 re-read); `.denyWithNote` = `resolve_approval(Deny)` then `steer(session, note)` |
| `Store.swift:281-283` | `setReceipt`, `setReceiptCount` |
| `Sources/Companion/PanelView.swift:28-30, 35` | the items-as-lines branch while a receipt is focused; `DoneZone` after the items, before the footer |
| `PanelView.swift:128, 135` | `ReplyField` under the options when `ask.replyFor == card.item.id`; `Hint("R", "reply")` when the kind takes words |
| `PanelView.swift:225, 244-247, 254` | `ReceiptView.width` (default unchanged), the `.reply` row with "Open ⌘O" |
| `Sources/Companion/AttentionPanel.swift:134, 148` | close clears `replyFor`; while the R field is up only ⎋ is the grammar's |
| `Sources/Companion/AppDelegate.swift:18, 31, 46, 117` | `askBar`; W1's key-up handler now calls `askBar.hotkeyUp()`; `installAskBar(attention: panel)` at the end of launch; the menu item |
| `scripts/bundle-companion.sh:61` | `NSMicrophoneUsageDescription` appended to the plist |
| `Tests/CompanionCoreTests/CompanionCoreTests.swift:172` | W1's table row `r → .none` ("not in the grammar — W3") became `r → .reply`, plus `⌘L → .askBar` |

## 2. What was verified and how

`BUILD_LABEL=130-w3 build-lock crates/desktop/companion/test.sh`: **41 tests in 9 suites passed** (W1's 24 + 1 updated row,
10 `AskRouterTests`/`AskVoiceTests`, 6 `ReceiptModelTests`). `scripts/bundle-companion.sh` (its own build-lock):
release build 47 s cold on the loaded machine (load ~30), bundle `plutil -lint` OK, ad-hoc `codesign --verify --deep
--strict`: "valid on disk · satisfies its Designated Requirement"; `plutil -p` shows `NSMicrophoneUsageDescription`.

Live probe against the user's running engine (`~/.theuth/engine.json` → `127.0.0.1:49431`, 2.10.0, untouched),
`open -g 'target/release/companion/Theuth Companion.app' --args --ask-probe 15 --shareable`, `/tmp/130-w3-probe.log`
(13:19 run; three earlier runs 13:15–13:18 read the same):

```
13:19:02  ask-probe start seconds=15.0 shareable=true bundle=io.theuth.mac.companion mic-usage-key=true
13:19:02  desk_open(theuth) = 01a10df6-1812-76a3-9423-da868a83b7e4 hub=true self=NAMGQ1Y4930L0
          holder=c2d88a8b202f25841300acc4d0fb7c39589703b1ccc70a5ed902839aefce9272
13:19:02  ask-probe desk=01a10df6-1812-76a3-9423-da868a83b7e4 this-engine-is-hub=true (nothing sent)
13:19:03  activity_list(24 h) = 89 entries, 0 by user@companion → receipts=0
13:19:04  ask-probe panel frame=(1040.0, 1056.0, 480.0, 72.0) visible=true key=false ignoresMouse=true active=false
13:19:18  ask-probe windows (CGWindowList): ["x=1040 y=312 w=480 h=72", "x=2534 y=690 w=26 h=341"]
13:19:18  ask-probe done after 15.0 s — quitting without sending (receipts=0 frontmost=theuth active=false
          key=none askbar key=false)
```

- **Hub + desk:** `mesh_status.lease.holder == self_id` on this engine, so `desk_open("theuth")` was answered here
  (the same session id on every run — the desk was already open; `roster::desk_or_open` returns it without
  opening another). Nothing was sent: `--ask-probe` never calls `send_input`/`steer`/`put_attachment`/
  `transcribe` (`AskBarController.submit` is also gated by `AskFlags.automated`).
- **`activity_list` works** against 2.10.0 (89 entries in 24 h); none carry `user@companion` yet because nobody has
  answered from the companion on this engine — so the Done zone was empty in the probe and is verified by the
  model tests only. `set_event_focus` → `-32601 Method not found` on 2.10.0 (pre-129), swallowed as designed
  (stderr line seen on a bare-executable run); on a 129+ engine it narrows deltas to the desk.
- **Ask bar on screen** (`/tmp/130-w3-shot.png`, crop `/tmp/130-w3-crop-askbar.png`, second 8): glass panel
  480 × 72 pt top-centre of the main display (x 1040–1520, y 312–384 top-down — the same 20 %-down rule as the
  Attention panel), caret glyph, "probe — do not act" in the field, the mic glyph at the right, hints
  "⏎ send to theuth · it routes · hold ⌃⇧Space to talk · ⎋ cancel"; W1's rail at the right edge beside it. CGWindowList
  bounds match the AppKit frame.
- **Focus:** `osascript` read the frontmost process as `Self Service` just before `open -g` and again at second 8
  (`theuth` at quit — the user's switch; the companion's own log reads `active=false key=none askbar key=false`).
  Probe panel is `ignoresMouseEvents = true`, never key. No dialog (the microphone path is unreachable in automated
  runs; no `requestAccess` was called). The app quit itself after 15 s; `pgrep -x Companion` empty afterwards.

Not exercised live (by the rules — a key Ask bar steals key, a hold opens the microphone prompt, a send is real):
⏎ → hold → `send_input`, the streamed reply, ⌘L, the R field, dictation. Those paths are the tested pure models
(`AskRouter`, `AskVoiceMachine`, `ReceiptModel`, `Card.replyAction`, `PanelModel` focus) plus thin glue; to try
them by hand: `open -g "<bundle>" --args --ask` opens the Ask bar key at launch.

## 3. Deviations from the design and the brief

- **Off the hub there is no second connection.** `mesh_status` names peers by iroh endpoint id and iroh
  addresses only; the companion's discovery (`<data_dir>/engine.json` + `token`) describes the local engine and
  nothing resolves a peer's WebSocket URL or token. The Tauri app's own voice-key path has the same limit
  (`UI/lib/voice-key.ts:349-354`: "theuth's desk lives on another engine"). So: hub → `desk_open` here (verified
  live); off-hub → theuth's desk session from `agents_list` if the replica carries it, and `send_input` to it
  (routed by the engine to the owner — [INFERENCE], not observed: this machine is the hub) — else the receipt row
  says "theuth's desk lives on <hub name> — open theuth there". Both off-hub branches are unit-tested, neither ran live.
- **The desk is resolved at the first send, not at launch**, so a probe or an idle companion never opens the desk;
  the id is cached for the process and forgotten after a failed send.
- **Reply collection is "from the send until the desk's next `TurnFinished` with text"**, keyed by session, not by
  turn id: `send_input` with the default `steer` delivery lands inside a running turn, so the reply may be the
  tail of that turn. A `TurnFinished` that arrives with nothing streamed is ignored (the turn we steered into).
- **Voice is the Ask bar's only.** "Hold the hotkey to dictate into it" for the R field (D6) is not built: the R
  field is typed; its hint says so. Point-and-say stays design 96's `Ctrl+S` in the Tauri process (untouched);
  the Ask bar's hints name only `⌃⇧Space`.
- **A hold from cold opens the Ask bar and records in one press** (needs-you = 0); when something needs you the
  same press opens the Attention panel and the release does not close it (W1's toggle still closes on the next tap).
- **R on a multi-question ask** is inert (like W1's card: "answer them in theuth"); R on `show`/unknown kinds is inert.
  R on an approval always denies (R1-5) — the field's placeholder says "Deny with a note".
- **Receipts history ↑ in the Ask bar (§5.4) is not built**; the Done zone in the Attention panel is the history.
- **The Done zone shows ≤ 4 rows**, windowed around the focused row, so the panel stays ≤ 560 pt; J/K walk all of them.
- **One publish per `TextDelta`** (`// ponytail:` in `askObserve`): no coalescing timer until the stream is seen to stutter.
- **`GlassPanel.make` duplicates W1's panel construction** (≈ 35 lines) rather than refactoring `PanelController`
  (a shared file W2 may touch in flight); fold `PanelController.init` onto it at merge.

## 4. Open items

- Merge: the orchestrator resolves adjacent-line conflicts with W2 in `AppDelegate.swift` (menu, launch tail),
  `Store.swift` (property/hook/func tails) and `PanelView.swift` if W2 touched them.
- First real answer from the companion will prove the E4 round-trip (`user@companion` rows → Done zone); the probe
  engine has none yet.
- A 129+ engine: confirm `set_event_focus([desk])` returns `null` and the panel sees only the desk's deltas.
- Off-hub machines: observe `agents_list[theuth].desk` on a spoke and whether `send_input` to a hub-owned session is
  routed; if not, the refusal text is what the user sees.
- Dictation end to end (permission prompt, `transcribe` with the voice plugin on) needs a hand run from the bundle.
- `Flags.probe` (W1) and `AskFlags.probe` (W3) are two switches; W4 can fold them.
