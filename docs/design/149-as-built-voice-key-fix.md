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

## Follow-ups F1/F2 (149g)
Branch `fix/149-voice-key-followups` (worktree `~/Develop/theuth-149g`) off `origin/main` d7d6549e. Swift only:
`crates/desktop/companion/**` and this section. Commits 7cc4cb14, f2f61757 (+ this file).

### F1 — ⎋ while the voice key is held
While ⌃S is physically down, ⎋ reaches Carbon as ⌃⎋; the plain-⎋ `pointEscape` registration never matched, so ⎋ cancelled
only after the release (`.locked`/`.sending`/`.replying`). Now a gesture registers a second ⎋ carrying the chord's modifiers:
- **Derivation** `KeyTable.escapeDuringHold(of:reserved:)` (`CompanionCore/KeyBindings.swift:270`): ⎋ (key code 53) with
  exactly the voice key's modifiers — a rebound chord works; ⌘/⌥ only when the chord has them. `nil` (nothing extra) for a
  bare chord (plain ⎋ already matches), for the system's `KeyTable.systemEscapes` ⌘⌥⎋ (Force Quit) and ⌘⌥⇧⎋ (held: force-quit
  the front app — neither is in `CopySymbolicHotKeys`), and for any enabled symbolic hot key passed in `reserved`
  (`Hotkeys.reserved` at runtime). Default ⌃S → ⌃⎋.
- **Registration** `PointController` (`PointAndSay/PointController.swift`): `KeyboardShortcuts.Name.pointEscapeHeld` (:14);
  `applyHotkey` (:117) sets it from the table whenever the voice key changes (`setShortcut` registers at once, so it is
  disabled again unless a gesture is live) and logs `hotkey pointEscapeHeld set ⌃⎋` / `cleared — <chord>: …`;
  `escapeHotkeys(on:)` (:136) enables/disables `pointEscape` + `pointEscapeHeld` together — on at `.start` (:202), off on
  the transition into `.idle`/`.replying` (:234) — logging `hotkey pointEscape registered ⎋ ⌃⎋` / `unregistered ⎋ ⌃⎋` to
  `~/.theuth/logs/companion.log`. Both names call `send(.esc)`: the same `PointMachine` path (`(.armed|.holding|.locked,
  .esc)` → `.idle` + `discard("esc", hint)`), and `VoiceKeyGate` keeps the rest of the hold's auto-repeats and the key-up out.
  Probes (`AskFlags.automated`) register neither ⎋ (`escapeHotkeys` returns; before 149g the probe did register plain ⎋
  for the length of each synthetic gesture).
- **Tests** (`test.sh` 183 → 190 in 30 suites): `KeyTableTests.escapeDuringHoldCarriesTheVoiceKeysModifiers` (W6Tests.swift:134:
  ⌃S → ⌃⎋, ⌃⌥Space → ⌃⌥⎋, ⌘⇧S → ⌘⇧⎋, bare F1 → nil, ⌘⌥ / ⌘⌥⇧ → nil, a reserved ⌃⌘⎋ → nil, `reserved` matches the whole
  chord); `VoiceKeyGateTests.escDuringTheHoldCancelsAndTheRestOfTheHoldIsIgnored` and `escCancelsAnArmedOrLockedHoldToo`
  (PointMachineTests.swift:252, :266: ⎋ at 500 ms into a 1.5 s hold from `.holding`, from `.armed` after one repeat and from
  `.locked` with the second tap still held → `.idle` with the hint, one `.discard("esc")`, the following repeats and the
  key-up no-ops, the next physical press a new gesture).
- **Probe** `--probe-point 52 --shareable` (`PointProbe.swift:251` ESC DURING HOLD; `:271` `syntheticHeldEscape`), log
  `/tmp/149g-probe-point.log` (pid 13706), PNG `/tmp/149g-hold-probe.png`:
  `ESC DURING HOLD: Esc at repeat 15 → state=idle overlays=false pill=true hint="Hold to talk · double-tap to lock"` →
  `23 repeats → gestures=1 state=idle … dictation.phase=idle dictation_status.active=nil` → **PASS**. Then the Carbon side:
  ⌃⎋ (derived from the default voice key — never ⌃S) registered under a probe-only name `probeHeldEscape` for 1 s,
  `AXIsProcessTrusted=true` for this process, a synthetic ⌃⎋ key-down/key-up (`CGEvent`, key code 53, `.maskControl`) posted at
  `.cghidEventTap` → `received=1 within 1 s → PASS; ⌃⎋ unregistered`. The modifier-carrying registration is **proven**, not
  argued. The 149f scenarios still pass (HOLD, REFUSED HOLD, CANCEL IN FLIGHT, RELEASE BEFORE START; control/stop PASS).

### F2 — the Ask bar's dictation gets the same in-flight guard
`AskBarPanel.swift beginDictation` had the voice key's old shape: an async `dictation_start` whose reply could land after
the hold ended (⎋/close → `.failed("cancelled")`, release-to-send, a refusal), and `.fail` stopped only a known id while
leaving the reducer in `.starting` — so the late reply was adopted by nobody and the engine held the slot for 300 s.
- **One rule, three callers** `AskDictation.awaitsStart(from:current:)` (`CompanionCore/AskVoice.swift:132`): a start's
  reply or refusal belongs to the controller only while its gesture is still the current one AND the session still waits
  (`.starting`). `PointController.beginDictation` (:377 success, :389 failure) now uses it too.
- **Ask bar** (`AskBar/AskBarPanel.swift`): `hold` (:237) is bumped by every `beginDictation`; the reply landing outside its
  hold is stopped at once (`dictation_start → <id> landed after the hold ended: dictation_stop`), a stale refusal is ignored
  (:283); `.fail` (every exit: ⎋, close, a refusal, an Error finish) stops the known id, **resets** `store.ask.dictation`,
  `pendingChunks`, `stopWanted` (:221); the mic-refusal task is guarded by the hold (`g == hold`) and never runs under a
  probe; `.setText` never sends under a probe (:213). Release-to-send before the reply keeps `stopWanted` (adopt, then stop).
  `VoiceRecorder` was already idempotent (149f). User-visible behaviour unchanged.
- **The same shape in `Voice/HoldToTalk.swift`** (the panel's V / the agents rows' mic — shares `store.ask.dictation`) got the
  identical guard (:35 `hold`, :109 reset, :138/:149 `awaitsStart`).
- **Foreign `DictationStarted` no longer adopted** (`PointController.observe` :462, `Store+Ask.swift dictationObserve` :160).
  The engine broadcasts `DictationStarted` for every dictation (the other surface's, Tauri's, a probe's); both reducers adopted
  it whenever they were `.starting`. The stricter guard exposed it: the first 149g probe run's REFUSED HOLD adopted the probe's
  own id, ignored its refusal and stopped the probe's dictation (FAIL, pid 12086). A surface's id is its `dictation_start`
  reply, nothing else.
- **Tests** `AskDictationHoldTests` (AskTests.swift:161; a pure driver over `AskVoiceMachine` + `AskDictation` + the hold
  counter with a fake clock and a fake RPC call list): `escBeforeTheStartRepliesStopsTheLateIdAtOnce`,
  `releaseBeforeTheStartRepliesAdoptsThenStops`, `aLateStartNeverLandsInTheNextHold`, `aStaleRefusalDoesNotEndTheNextHold`.
- **Probe** `--ask-probe 25 --synthetic-dictation --shareable` (`AskBarInstall.swift:93 holdScenarios`, mirroring
  `PointProbe`'s `probeStartDelay` knob as `AskBarController.probeStartDelay`), logs `/tmp/149g-probe-ask.log` (pid 14023) and
  `/tmp/149g-probe-ask-asklog.log`, against the live engine, driving `voiceEvent` (what the hotkey calls), no mic, no send:
  CANCEL IN FLIGHT (start held 1.5 s, `close` — ⎋'s path — at 400 ms) → `dictation_start → … landed after the hold ended:
  dictation_stop`, `voice=idle phase=idle status="cancelled" dictation_status.active=nil` **PASS**; RELEASE BEFORE START
  (key-up into the send path at 400 ms) → `dictation_stop(…) ok`, `voice: Nothing recognized`, `active=nil` **PASS**;
  REFUSED (the probe holds the slot) → `voice=idle status="dictating elsewhere" active=<the probe's id>` **PASS**.

### Gates (all under `env BUILD_LABEL=149g build-lock`)
- `bash crates/desktop/companion/test.sh` ✔ **190 tests in 30 suites passed** (`/tmp/149g-test.log`).
- `swift build -c release --package-path crates/desktop/companion` ✔ (inside `bundle-companion.sh`; pre-existing warnings only).
- `bash scripts/bundle-companion.sh` + `codesign --verify --deep --strict` ✔ `valid on disk · satisfies its Designated
  Requirement` (`/tmp/149g-bundle.log`).
- Probes: `/tmp/149g-probe-point.log` 6/6 PASS + synthetic ⌃⎋ PASS; `/tmp/149g-probe-ask.log` 3/3 PASS; `/tmp/149g-hold-probe.png`.
- Leftover greps: hex/palette grep → the same two history comments (`Palette.swift:4`, `PointProbe.swift:16`); emoji-glyph
  `Text("…")` grep → empty.

### Live check — one extra step (the owner)
4. Hold ⌃S, and while still holding press ⎋. Expect the pill to show "Hold to talk · double-tap to lock", the trail gone at
   once, nothing sent, and in `companion.log`: `hotkey pointEscape registered ⎋ ⌃⎋` at the press, `point esc holding(…) → idle
   [discard(esc, …)]`, `hotkey pointEscape unregistered ⎋ ⌃⎋`, and either `dictation_stop` or `dictation_start → <id> landed
   after the gesture ended: dictation_stop`; no further `point pressed` while the key stays down; release, hold again at once →
   dictates normally, no "dictating elsewhere" anywhere. At launch the log shows `hotkey pointEscapeHeld set ⌃⎋` (first run)
   and `hotkey pointEscape unregistered ⎋ ⌃⎋`.

### Deviations
- `HoldToTalk` (panel V / agents mic) was fixed alongside the Ask bar: identical code shape, same leaked-slot symptom, inside
  `crates/desktop/companion/**`.
- Foreign-`DictationStarted` adoption removed in both reducers (found by the probe, not in the brief).
- Probes no longer register plain ⎋ during their synthetic gestures (they did before 149g).
- No Rust, no TypeScript, nothing pushed or merged.
