# 149f as built — the voice key (⌃S point-and-say) regression: reproduced, fixed, proved

Branch `fix/149-voice-key` (worktree `~/Develop/theuth-149f`) off `origin/main` 7197905f (v2.14.0). Files:
`crates/desktop/companion/**` (Swift), `crates/voice/src/dictation.rs` (INFO tracing only), this file. Mechanism C (below) is
not real, so `crates/desktop/src/**` and `crates/desktop/ui/**` are untouched.

## The report (owner, 2026-10-09 09:45)
> "this update completely breaks dictation in the companion app and in the tauri app. If I hold down Ctrl S, it just like flips
> back and forth between dictating and not dictating. It doesn't record anything, doesn't draw anything. And then for a period of
> time after the night turns off it says dictating elsewhere in the tauri app and I can't dictate there either."

Running: `/Applications/theuth.app` (theuth-desktop pid 42083, 09:37:40), the login-item companion (pid 42090, 09:37:42, main
3ae1c54d, 2.14.0), `t --headless` (09:37:43). Never touched by this work.

## Evidence before any change
- Engine log (`~/.theuth/logs/theuth.log.2026-10-09`, UTC): `computer::context: screen_context composed` at 08:41:40Z, 08:42:50Z,
  08:43:24Z, 08:43:27Z — four gestures reached the Stop sequence (`PointController.stopSequence`). Nothing from the dictation
  path: `crates/voice/src/dictation.rs` logged only the model load (`dictation model loaded in 1.14 s` at 08:37:58Z); start,
  refuse, stop and finish were silent at INFO. Fixed below.
- Unified log: AVAUVoiceIO / HAL voice isolation initialised at 09:39:51 — the companion opened the microphone at least once.
- The companion had NO persistent log (`Log.write` → stderr + `/tmp/130-*-probe.log` under probe flags only; `PointLog.write`
  → `/tmp/149-W2-probe.log` under `--probe-point` only). Nothing from the owner's attempts exists on disk. Fixed below.
- `theuth-desktop` logs to stderr only (`crates/desktop/src/lib.rs:32 tracing_subscriber::fmt::init()`); nothing on disk either.

### Does Carbon re-deliver `kEventHotKeyPressed` while the key is held (auto-repeat)?
Yes. `KeyboardShortcuts` (`.build/checkouts/KeyboardShortcuts/Sources/KeyboardShortcuts/CarbonKeyboardShortcuts.swift`) registers
with `RegisterEventHotKey(…, options 0, …)` (:109-116, non-exclusive) and its handler forwards EVERY `kEventHotKeyPressed` to
`onKeyDown` with no repeat filter (:229-231); the HIToolbox header documents no suppression (`CarbonEvents.h:4675-4693`) and the
system posts a hot-key-pressed event per keyboard repeat. Design 96 already assumed this ("auto-repeat `Pressed` is ignored",
`96-point-and-say.md:56, :442-443`), and the Windows companion registers `MOD_NOREPEAT` for the same reason
(`149-companion-native.md:177`). The Rust and Swift machines ignore a repeat in `armed`/`holding` — but NOT in `idle`, which is
where a refusal puts the machine while the key is still physically down. (Not verified by registering ⌃S from a second process —
forbidden while the owner's companion holds it — the probe feeds the same stream synthetically.)

Is there a macOS equivalent of Windows' `MOD_NOREPEAT`? No. `RegisterEventHotKey`'s `inOptions` has exactly one bit,
`kEventHotKeyExclusive` (`CarbonEvents.h:15395-15424`: `kEventHotKeyNoOptions = 0`, `kEventHotKeyExclusive = 1 << 0`); nothing
suppresses repeats, and KeyboardShortcuts neither passes an option nor filters (`grep -i repeat` over its sources finds only an
array initialiser). Key-down state tracking in the companion (`VoiceKeyGate`) is the only guard on macOS.

## Reproduction (`--probe-point 44 --shareable`, the in-worktree bundle's executable, against the owner's live engine)
Commit b19b2cf7 added three scenarios to `PointProbe.sequence` (after the existing control/stop/gesture phases), driven through
the hot key's own entry points `PointController.keyDown()/keyUp()` with exactly what Carbon delivers for a held chord — one
key-down, a key-down every 30 ms, the key-up — plus `dictation_status` (`RpcClient.dictationActive()`, `AskRpc.swift`) to read the
engine's one dictation slot. No ⌃S registration (`AskFlags.automated`), no microphone, nothing sent (`dryRun`). Logs:
`/tmp/149f-probe-before.log` (pre-fix, pid 48668) and `/tmp/149f-probe-after.log` (post-fix); PNG `/tmp/149f-hold-probe.png`
(the engine's capture mid-hold: the voice pill "Listening… · ⌃S held · theuth's desk" beside the pointer).

| scenario | pre-fix (b19b2cf7) | post-fix (338a54de) |
|---|---|---|
| HOLD 1.5 s with the tone (47 repeats) | 1 gesture, Stop sequence, "Nothing recognized" (a tone), `active=nil` — PASS | same — PASS |
| REFUSED HOLD: the probe takes the slot first, then the hold | **25 gestures in 1.5 s** (24 repeats): `pressed idle → armed [start]` → `dictation_start failed: rpc -32000: already dictating` → `dictation refused: dictating elsewhere` → `cancel armed → idle [discard]` → next repeat → `pressed idle → armed [start]` … — **FAIL** (the report's "flips back and forth", overlays revealed/concealed within one RPC round trip = "doesn't draw anything") | **1 gesture**, `state=idle hint="dictating elsewhere"`, 46 repeats ignored, the probe's dictation untouched — PASS |
| CANCEL IN FLIGHT: key-down, Esc before `dictation_start` replies, key-up | `dictation_start → 01a11fe6-396b… adopted=false phase=idle`; 1 s later `dictation_status.active=01a11fe6-396b…` — **the dictation is live on the engine with no owner**, FAIL (the probe stopped it: `cleanup: stopped the leaked dictation`) | `dictation_start → … landed after the gesture ended: dictation_stop`; `active=nil` — PASS |
| RELEASE BEFORE START (addendum, the Windows port's race): `dictation_start` held 1.5 s via `probeStartDelay`, a 1 s hold released into the Stop sequence before the id exists | not run pre-fix; from code: `endDictation` set `stopWanted`, `finalText` waited while `.starting`, the landing start adopted and stopped — covered by `stopWanted` already, but a start landing after a NEW gesture began adopted into the wrong gesture | `point released holding → sending [stop(release)]` → 1.5 s later `dictation_start → 01a11ff3-d86e…` → stopped; `state=idle hint="Nothing recognized" active=nil` — PASS |

So A and B are both real and feed each other:

## Root causes (file:line at main 3ae1c54d / 7197905f)
- **B — a dictation left open on the engine.** `PointController.beginDictation` (PointController.swift:317-342) enqueued
  `dictation_start`; any exit before the reply (Esc, cancel, a short release, a mic refusal — all through `teardownGesture`
  :216-225) found `dictation.id == nil` (phase `.starting`), stopped nothing and reset `dictation`. When the reply landed,
  `AskDictation.handle(.started)` (AskVoice.swift:139-142) ignored it (`guard phase == .starting`), so the engine kept the
  dictation until `dictation_max_secs` (300 s, `crates/voice/src/lib.rs:459`): the Tauri app's `elsewhere`
  (`crates/desktop/ui/src/lib/dictation.ts:30-62`, a `DictationStarted` with a foreign id, cleared only by its
  `DictationFinished`) = "for a period of time … dictating elsewhere … can't dictate there either", and every companion
  `dictation_start` was refused "already dictating".
- **A — the refusal loop.** A refusal (`refuse`, PointController.swift:344-352) sends `.cancel` → `(.armed, .cancel)` →
  `.idle` (PointMachine.swift:70-71). The key is still down; the next Carbon auto-repeat `.pressed` in `.idle` → `.start`
  (PointMachine.swift:57-59) → `dictation_start` → refused → `.cancel` → … at the keyboard repeat rate. Overlays and the pill
  are revealed and concealed within one RPC round trip ("doesn't draw anything"); the recorder is started and cancelled each
  cycle ("doesn't record anything"); a cycle whose refusal lands late reaches the Stop sequence (the four `screen_context
  composed` lines) and ends with "Nothing recognized". Any first refusal starts it: B's "already dictating", a microphone
  failure, a live meeting's "talking".
- **C — two owners of ⌃S: not real.** `crates/desktop/src/companion/mod.rs:26 BUNDLE_ID = "io.theuth.mac.companion"` equals the
  running bundle's `CFBundleIdentifier` (read from `/Applications/theuth.app/…/Theuth Companion.app/Contents/Info.plist`; the
  AUVoiceIO client name `io-theuth-mac-companion` is CoreAudio's dashed form of it). `macos.rs:25,141-151` polls
  `runningApplicationsWithBundleIdentifier` every 2 s; `App.tsx:386-400` binds `want = enabled && !companion ? chord : ''` and
  `point/mod.rs:114-147 voice_key_bind("")` unregisters. The desktop started 2 s before the companion, so Tauri held ⌃S for at
  most one poll (~2-4 s after 09:37:42) — not at 09:40. And the probe reproduces the exact symptom with a single owner. Not
  verifiable at runtime without registering ⌃S from a second process (forbidden) or a desktop log (none exists).

## The fixes (commit 338a54de; diagnostics b19b2cf7)
1. **Auto-repeat immunity — `VoiceKeyGate`** (`CompanionCore/PointMachine.swift`): `press(now:)` is `true` for the first
   key-down and `false` while the key is down; `release()` clears it. `PointController.keyDown()/keyUp()` run the hot key (and
   the probe) through it, so one physical press is one gesture and a gesture a refusal ended stays ended until the key is
   released — nothing re-arms from `.idle` on a repeat. A press ≥ `lostKeyUpMs` (2.5 s, past the slowest macOS auto-repeat of
   1.8 s) after the last one while "down" counts as a new press, so a key-up Carbon never delivered cannot leave the key dead.
   Tests `VoiceKeyGateTests` (PointMachineTests.swift, fake ms clock, the gate in front of the machine): `oneHoldIsOneGesture`,
   `refusalEndsTheGestureForTheRestOfTheHold` (the report), `standingRefusalAnswersOncePerPress` (a live meeting),
   `lockedHoldDoesNotSendOnAutoRepeat` (a repeat in `.locked` used to be the send), `lostKeyUpIsNotADeadKey`.
2. **Dictation lifecycle** (`PointController.beginDictation`): the start closure captures the gesture number; when the reply
   lands after the gesture ended (`gesture` moved on, or `dictation.phase != .starting` because `teardownGesture` reset it) the
   id is stopped at once on the same serial queue (`dictation_stop`), logged `landed after the gesture ended`. The release-
   before-reply path keeps `stopWanted` (adopt, then stop). A late failure or mic refusal of an OLD gesture no longer cancels
   the new one (`g == gesture` guards). Every other exit already stopped a known id (`teardownGesture`, `stopSequence →
   endDictation`); `dictation_stop` is idempotent on the engine (`dictation.rs` `StopDictation`).
3. **Microphone** (`Voice/VoiceRecorder.swift`): `start()` tears down a running engine first (never two input engines, never a
   leaked one) and carries a session number across the `requestAccess` suspension — a `cancel()`/`stop()` in between means the
   resumed start installs nothing. A failed `engine.start()` still removes its tap. Stop on every abort was already the case
   (`teardownGesture → recorder.cancel()`, `stopSequence → recorder.stop()`); the Ask bar shares the recorder class.
4. **Permanent diagnostics** (`Palette.swift Log`): every `Log/PointLog/AskLog` line now also appends to
   `~/.theuth/logs/companion.log` (`yyyy-MM-dd HH:mm:ss.SSS`, pid; rotated to `companion.log.1` at 5 MB), stderr and the probe
   files unchanged. New lines: every machine step with an effect or a transition (`point <event> <from> → <to> [effects]`),
   `hotkey voiceKey registered ⌃S` / `unregistered`, `dictation_start → id` / `failed: …` / `landed after the gesture ended`,
   `dictation refused: …`, `hotkey key-up without a key-down (ignored)`. Engine side (`crates/voice/src/dictation.rs`, INFO):
   `voice: dictation started id=… source=…`, `dictation refused source=… reason=… [active=…]`, `dictation stop id=… known=…`,
   `dictation finished id=… reason=Done|Stopped|Error{…}` (`Stopped` = the `dictation_max_secs` timeout). The peer/client id
   lives in `crates/rpc` (not owned here); the source (`Remote { sample_rate: 16000 }` = a companion, `Mic` = the engine's own)
   is logged instead.

## Gates
- `env BUILD_LABEL=149f build-lock bash crates/desktop/companion/test.sh` ✔ **183 tests in 29 suites passed** (178 + the 5
  gate tests; `/tmp/149f-test.log`).
- `env BUILD_LABEL=149f build-lock swift build -c release --package-path crates/desktop/companion` ✔ (inside
  `bundle-companion.sh`; pre-existing deprecation/Sendable warnings only).
- `env BUILD_LABEL=149f build-lock bash scripts/bundle-companion.sh` ✔ `valid on disk · satisfies its Designated Requirement`
  (`/tmp/149f-bundle.log`; `codesign --verify --deep --strict` is the script's last step).
- `--probe-point 44 --shareable` ✔ control 120/120 → stop 0/120 PASS; gesture phase PASS; HOLD PASS; REFUSED HOLD PASS;
  CANCEL IN FLIGHT PASS; RELEASE BEFORE START PASS (`/tmp/149f-probe-after.log`, `/tmp/149f-hold-probe.png`). Pre-fix run for
  the record: `/tmp/149f-probe-before.log` (REFUSED HOLD FAIL 25 gestures; CANCEL IN FLIGHT FAIL, leak stopped by the probe's
  cleanup).
- Rust (voice tracing), each through `env BUILD_LABEL=149f build-lock`: `mbx test -p voice --lib` ✔ 27 passed
  (`/tmp/149f-voice-test.log`); `mbx test -p desktop --no-run` ✔ (`/tmp/149f-desktop-build.log`); `mbx clippy -p voice -p
  desktop --all-targets -- -D warnings` ✔ (`/tmp/149f-clippy.log`; the pre-existing `block v0.1.6` future-incompat note only);
  `cargo fmt -p voice -p desktop -- --check` ✔.
- UI untouched: no UI gates.

## Live check (the owner; 3 steps)
1. Rebuild and install the companion from this branch (`bash scripts/bundle-companion.sh`, then your usual nesting into
   `/Applications/theuth.app/Contents/Library/LoginItems/`), quit the old companion, start the new one. Watch
   `tail -f ~/.theuth/logs/companion.log`: expect `launch bundle=io.theuth.mac.companion probe=no`, `connection: open`,
   `hotkey voiceKey registered ⌃S`.
2. Hold ⌃S for ~2 s while moving the pointer and saying a sentence; release. Expect ONE `point pressed idle → armed [start]`,
   `dictation_start → <id>`, `tick … → holding`, the trail drawn, the pill's live text, `point released holding → sending
   [stop(release)]`, `point sent to desk …`; no further `point pressed` lines while the key was held. In the engine log (if
   rebuilt): `voice: dictation started …` / `dictation stop … known=true` / `dictation finished … Done`.
3. Tap ⌃S and immediately press ⎋ (or tap ⌃S twice quickly then ⎋), then hold ⌃S again at once. Expect the second hold to
   dictate normally (no "dictating elsewhere"), and the log to show either a normal `dictation_stop` or
   `dictation_start → <id> landed after the gesture ended: dictation_stop`; the Tauri app's composer never shows "dictating
   elsewhere" more than a second after the gesture ended.

## Deviations
- The engine-side trace logs the audio source, not the RPC client id (the peer is `crates/rpc`'s, outside this brief's
  ownership).
- Mechanism C is argued from code and the single-owner reproduction; not observed at runtime (no desktop log, no second ⌃S
  registration allowed).
- The Ask bar's own `beginDictation` (`AskBar/AskBarPanel.swift:248-269`) has the same cancel-while-start-in-flight shape (its
  `voiceEvent(.failed)` path stops only a known id). Not changed here — the brief is the voice key; worth the same guard.
- The first refusal on the owner's machine at 09:40 is not known (no log existed); every candidate (a leaked dictation from an
  earlier tap/Esc/short release, a microphone failure, a meeting lease) now ends the hold once with its hint.
