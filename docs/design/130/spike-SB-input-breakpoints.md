# Spike SB — S3 double-tap ⌥ detection + permission class · S4 permission-free idle time and app-switch breakpoints

Status: COMPLETE 2026-10-07 12:05 (skeleton 11:48, re-saved after every section; ~18 min wall-clock work, 40-min box). Design 130 (companion).
Scratch: /tmp/130-spikes/SB/{Tap,Idle}. Machine: macOS 26.6.2, Swift 6.3.2, CLT only (no xcodebuild).

## 1 Detector + synthetic checks

**Package:** `/tmp/130-spikes/SB/Tap` (SwiftPM, tools 5.9, macOS 13+; `swift build -c release` 57 s cold, 2 s warm; no Xcode needed).
Files: `Sources/Tap/DoubleTapDetector.swift` (62 lines), `Sources/Tap/main.swift` (checks + monitor run).

**`DoubleTapDetector`** — a pure `struct` with `mutating func feed(flags: NSEvent.ModifierFlags, at: TimeInterval) -> Bool`
and `mutating func cancel()` (call on any keyDown/mouseDown), four-phase state machine `idle → firstDown → firstUp → secondDown`:
- masks flags to `[.shift, .control, .option, .command, .function]` (Caps Lock is a lock state, ignored);
- an identical consecutive flag state is a non-transition (auto-repeat / duplicate → ignored, no state change);
- a tap must START from nothing held (`previous.isEmpty`), so ⌥⌘ then releasing ⌘ is not "⌥ pressed alone";
- `maxHold` 300 ms per press (longer = hold, not tap) and `window` 300 ms from first release to second press;
- returns `true` exactly once, on the release that completes the second tap; a second tap outside the window becomes a new first tap (double-click semantics).

**Synthetic checks** (`./.build/release/Tap --checks-only`, run 2026-10-07 11:52, exit 0) — 10/10 PASS, output verbatim in `/tmp/130-spikes/SB/tap-run.log`:

| case | timestamps (s) | expected | got |
|---|---|---|---|
| double tap | ⌥↓0 ↑0.08 ↓0.18 ↑0.26 | …,true | PASS |
| tap then hold (2nd press released at 1.0) | ⌥↓0 ↑0.08 ↓0.18 ↑1.00 | all false | PASS |
| single long hold then a tap | ⌥↓0 ↑0.60 ↓0.70 ↑0.75 | all false | PASS |
| ⌥⌘ chord, release ⌘, release ⌥, then a tap | ⌥↓0 +⌘0.05 −⌘0.10 ↑0.15 ↓0.20 ↑0.25 | all false | PASS |
| two taps 400 ms apart (gap 350 ms) | ⌥↓0 ↑0.05 ↓0.40 ↑0.45 | all false | PASS |
| …plus a third tap 100 ms later | + ↓0.55 ↑0.60 | true on last | PASS |
| auto-repeat duplicate flags while held | ⌥↓0 ⌥↓0.03(dup) ↑0.08 ↓0.18 ↑0.26 | true on last | PASS |
| key pressed between taps (`cancel()`) | ⌥↓0 ↑0.05 [key] ↓0.15 ↑0.20 | all false | PASS |
| ⇧ tapped between the ⌥ taps | ⌥↓0 ↑0.05 ⇧↓0.10 ⇧↑0.15 ⌥↓0.20 ↑0.25 | all false | PASS |
| Caps Lock on throughout | same as double tap with `.capsLock` set | …,true | PASS |

Not covered (design decision for the Swift wave): left vs right ⌥ (device-dependent bits 0x20/0x40 are masked away — both count); whether a mouse click between taps should cancel (the detector offers `cancel()`, the caller decides).

## 2 Monitor run (what fired, trust/preflight values)

Program: `Tap` (no `--checks-only`) creates `NSApplication.shared`, `setActivationPolicy(.prohibited)` (no Dock icon, no menu bar, never activates), prints the two read-only permission queries, installs `NSEvent.addGlobalMonitorForEvents(matching: .flagsChanged)` and `addLocalMonitorForEvents(matching: .flagsChanged)`, runs `NSApp.run()` for N s, prints counts, exits. No CGEventTap, no `requestAuthorization`, no `AXIsProcessTrustedWithOptions`, no `CGRequestListenEventAccess`. No dialog appeared (nothing in the user's attention; both runs exited 0 by themselves).

Run A — 8 s, launched from theuth's `shell()` (2026-10-07 11:52, `/tmp/130-spikes/SB/tap-run.log`):
```
AXIsProcessTrusted() = true
CGPreflightListenEventAccess() = true
monitors installed: global=true local=true; listening for 8 s
window closed +8.399s: global flagsChanged events=0 local=0 double-taps detected=0
```
Run B — 90 s, same context, background (`tap-bg90.log`, 11:57:23 → 11:58:58): `AXIsProcessTrusted() = true`, `CGPreflightListenEventAccess() = true`, `global flagsChanged events=0 local=0`.

**Both runs are INCONCLUSIVE for the permission question, for two independent reasons:**
1. *Inherited trust.* The process tree is `launchd(1) → /Applications/theuth.app/Contents/MacOS/theuth-desktop(3595) → …/MacOS/t(3605) → bash → Tap` (`ps -o pid,ppid,comm` walk, 11:53). TCC attributes a helper to its **responsible process**, here theuth.app, which already holds Accessibility; so an unsigned binary in /tmp reports *trusted* and *preflight true*. Proof that it is the parent, not the binary: the same binary launched by launchd (`launchctl submit -l io.theuth.spike130.tap-trust … -- …/Tap --trust-only`, 11:57, ppid=1) printed `AXIsProcessTrusted() = false` and `CGPreflightListenEventAccess() = false` (`/tmp/130-spikes/SB/tap-launchd.out`). The launchd job was removed afterwards (`launchctl remove`, exit 0).
2. *No modifier was pressed.* The user's keyboard was idle for the whole window — `Idle` measured `secondsSinceLastEventType(.keyDown)` at 305 s and climbing during the same minutes (§4) — so "0 global flagsChanged events" says nothing about delivery. The local monitor could never fire: a `.prohibited` app with no window receives no key events of its own.

The un-trusted monitor run (same binary via `launchctl submit` **without** `--trust-only`) was deliberately **not run**: if a `flagsChanged` global monitor in an untrusted process does trigger the Input Monitoring / Accessibility prompt, that would violate the no-dialog rule. It is the 30-second manual step in §7.

## 3 Documentation and community evidence (cited)

**Apple documentation (read 2026-10-07, rendered pages):**
- `NSEvent.addGlobalMonitorForEvents(matching:handler:)` — macOS 10.6+. Discussion, verbatim: *"Key-related events may only be monitored if accessibility is enabled or if your application is trusted for accessibility access (see AXIsProcessTrusted())."* and *"your handler will not be called for events that are sent to your own application."* `NSFlagsChanged` is in the list of monitorable types. It does not say whether `flagsChanged` counts as "key-related". https://developer.apple.com/documentation/appkit/nsevent/addglobalmonitorforevents(matching:handler:)
- `CGPreflightListenEventAccess()` — Mac Catalyst 13.1+, macOS 10.15+; the page carries the signature only, no discussion. https://developer.apple.com/documentation/coregraphics/cgpreflightlisteneventaccess() — SDK 26.5 header `CoreGraphics.framework/Headers/CGEvent.h:398-402`: *"Checks whether the current process already has event listening access"* (`CGPreflightListenEventAccess`, 10.15) next to `CGRequestListenEventAccess` (10.15). It does not prompt (our runs printed it in both a trusted and an untrusted process, §2).
- `CGEventSource.secondsSinceLastEventType(_:eventType:)` — Mac Catalyst 13.1+, macOS 10.4+; *"To get the elapsed time since the previous input event—keyboard, mouse, or tablet—specify kCGAnyInputEventType."* No permission mentioned. https://developer.apple.com/documentation/coregraphics/cgeventsource/secondssincelasteventtype(_:eventtype:) ; header `CGEventSource.h:136-143` `API_AVAILABLE(macos(10.4))`; `CGEventTypes.h:491` `#define kCGAnyInputEventType ((CGEventType)(~0))`.
- `NSWorkspace.didActivateApplicationNotification` — macOS 10.6+; userInfo carries `applicationUserInfoKey` → `NSRunningApplication`; *"To receive this notification, use notificationCenter to register for it"* (the workspace's own center). No permission mentioned. https://developer.apple.com/documentation/appkit/nsworkspace/didactivateapplicationnotification
- Apple Support, "Control access to input monitoring on Mac": *"Some apps can monitor your keyboard, mouse, or trackpad even when you're using other apps"* — the user-facing definition of the Input Monitoring class. https://support.apple.com/guide/mac-help/control-access-to-input-monitoring-on-mac-mchl4cedafb6/mac

**Apple DTS (Quinn), quoted in a GitHub issue I could read (the forum itself is behind a bot check for this harness):**
- Forum thread 707680 (quoted in https://github.com/nikitabobko/AeroSpace/issues/1012 comments, read via the GitHub API): *"Rather than using an NSEvent global event monitor, use a CGEventTap. For weird historical reasons, the former requires the Accessibility privilege whereas the latter requires the Input Monitoring privilege. The Input Monitoring privilege is easily available to sandboxed apps … It even has APIs to check for (CGPreflightListenEventAccess) and explicitly request (CGRequestListenEventAccess)."*
- Forum thread 735223 (quoted in the issue body): *"To listen for keyboard events you'll need the Input Monitoring privilege. … CGEventTap is compatible with the App Sandbox, starting with macOS 10.15."* Also recommends the nested `LSUIElement` helper + `SMAppService.loginItem(identifier:)` shape the companion already plans.

**Community:**
- Stack Overflow 41927843 "Global modifier key press detection in Swift" (accepted answer, score 18, 2017; read via api.stackexchange.com): the recipe is exactly `addGlobalMonitorForEvents(matching: .flagsChanged)` + `.deviceIndependentFlagsMask`; it quotes the accessibility sentence from the docs and makes **no** claim that it works untrusted.
- Stack Overflow 49135281 (accepted, score 10): under Xcode the debugger host (Xcode) had to be trusted too — the same responsible-process effect we measured in §2.
- Stack Overflow 58670785 (accepted): `IOHIDRequestAccess(kIOHIDRequestTypeListenEvent)` requests Input Monitoring, `kIOHIDRequestTypePostEvent` requests Accessibility (10.15+) — the two classes behind the Privacy panes.
- nissaba/KeyCatch README (CGEvent tap sample that reports modifier-only `flagsChanged` presses): *"Accessibility permission is required to observe global input."* https://github.com/nissaba/KeyCatch
- derodero24/sleight README (tap/hold modifier remapper on `CGEventTap`): *"an app that asks for Accessibility"*; modifiers *"arrive as flagsChanged rather than keyDown/keyUp, and left and right share the public modifier masks, so Sleight reads the device-dependent bits"*. https://github.com/derodero24/sleight
- Itsuki, "SwiftUI/MacOS: Detect/Listen To Global Key Events Two Ways" (global monitor for `[.flagsChanged, .keyUp, .keyDown]`): requirements listed as code signing, no App Sandbox, Accessibility enabled. https://levelup.gitconnected.com/swiftui-macos-detect-listen-to-global-key-events-two-ways-df19e565793d
- Apple Developer Forums threads 100877 ("NSEvent.addGlobalMonitorForEvents (…") and 811443 ("Why does NSEvent.addGlobalMonitorF…") surfaced in search but every fetch (plain and rendered) returned the Cloudflare "Security verification" page — **not read, unverified**.
- The brief's premise "many report that flagsChanged/modifier-only monitoring works without trust while keyDown needs it": **I found no readable source stating it** in this time box. Every readable source above (Apple docs, DTS, two CGEventTap samples, one tutorial, One's support page) puts global modifier observation under Accessibility; none distinguishes `flagsChanged` from `keyDown`. Treat the exemption as **unverified** until the §7 manual test — the one piece of evidence that would settle it on *this* macOS (26.6.2).
- Prior theuth finding (knowledge `facts/macos-key-chords-for-a-global-hotkey-ctrl-d-vs-ctrl-s-2026-10-04`): "modifier-only chords need an event tap" — Carbon `RegisterEventHotKey` cannot express a modifier-only chord.

**sindresorhus/KeyboardShortcuts** (README v3.1.0, macOS 10.15+, https://github.com/sindresorhus/KeyboardShortcuts): built on Carbon `RegisterEventHotKey` (FAQ *"Why is this package importing Carbon?"*); FAQ *"Does this package cause any permission dialogs? — No."*; FAQ *"Can I listen to the Caps Lock key? — No, Caps Lock is a modifier key and cannot be directly listened to using this package's standard event methods. If you need to detect Caps Lock events, you'll need to use lower-level APIs like CGEvent.tapCreate."* A `Shortcut` is a key plus modifiers; modifier-only shortcuts are not a supported shape. So a double-tap ⌥ cannot ride on the permission-free hot-key path; it needs a monitor or a tap, i.e. TCC.

**One (getone.one/support, read 2026-10-07):** permissions table row *"Accessibility — Makes the Option key double-tap work from any app and lets One type into terminals. [Without it:] The gesture only works while One is in front, and typing into agents does nothing."* Troubleshooting *"The Option double-tap does nothing — … Check Accessibility: System Settings → Privacy & Security → Accessibility → One must be listed and switched on. … Make sure another utility is not already capturing the Option key (some window managers and text expanders do)."* A shipping product with exactly this gesture needs Accessibility for the global case and falls back to in-front-only without it.

## 4 Idle-time measurements

**Package:** `/tmp/130-spikes/SB/Idle` (SwiftPM, macOS 13+, `swift build -c release` 70 s cold). Every second it prints
`CGEventSource.secondsSinceLastEventType(.combinedSessionState, eventType:)` for `.keyDown`, `.mouseMoved`, `.leftMouseDown`,
`kCGAnyInputEventType` (`CGEventType(rawValue: ~0)`, from `CGEventTypes.h:491`), and IOKit `HIDIdleTime` from the `IOHIDSystem`
registry entry (`IORegistryEntryCreateCFProperties`, ns → s). Activation policy `.prohibited`; nothing is activated.

Run A — 12 s (11:54, `/tmp/130-spikes/SB/idle-run.log`, exit 0):
```
t      keyDown   mouseMoved  leftMouseDown  anyInput   HIDIdleTime(IOKit)
+01.01s    305.55        5.16          24.85      5.16   9.31
+06.01s    310.55       10.16          29.86     10.16   14.31
+12.01s    316.55       16.16          35.85     16.16   20.31
```
Run B — 90 s (11:57:23, `idle-bg90.log`, 90 ticks at exactly 1.00 s spacing): keyDown 496.11 → 584.11, mouseMoved 71.77 → 159.77, leftMouseDown 215.42 → 303.42, anyInput = mouseMoved throughout, HIDIdleTime 72.69 → 160.70.

Run C — 12 s **untrusted** (same binary via `launchctl submit -l io.theuth.spike130.idle … -- …/Idle --seconds 12`, 12:02, ppid=1, `/tmp/130-spikes/SB/idle-launchd.out`, job removed afterwards): identical behaviour, no dialog — keyDown 14.02 → 25.02, mouseMoved 7.87 → 18.87, leftMouseDown 12.07 → 23.07, anyInput = mouseMoved, HIDIdleTime 8.14 → 19.14 (IOKit 0.27 s above CG here). The user had typed ~14 s and moved the mouse ~8 s before this run, so the absolute values are plausible too. This is the run that makes S4 a tested YES rather than an inherited-trust artefact.

Sanity: every column increases by exactly 1.00 s per tick in both runs (user idle; keyboard untouched for ≥ 10 min, last mouse move ≈ 5 s before run A). **"Drops on input" is UNVERIFIED** — the user gave no input during either window (a 20-line manual re-run is in §7). All five sources agree on the ordering (mouse more recent than click more recent than key). The IOKit value is *larger* than `anyInput` by 4.15 s in run A and 0.92 s in run B: `HIDIdleTime` counts hardware HID reports only, while the CG session state also counts events posted by software (e.g. theuth's own `computer_*` synthetic mouse moves) — so for a "user is idle" breakpoint the two can disagree by whatever synthetic input is going on; prefer `HIDIdleTime` (or require both) when theuth itself drives the mouse.

**Dialogs / TCC:** none appeared; the program needs no entitlement. The requested log check could not run from this process: `log show --last 12m --predicate 'subsystem == "com.apple.TCC"'` → `log: Could not open local log store: Operation not permitted`, and `sqlite3 -readonly ~/Library/Application\ Support/com.apple.TCC/TCC.db` → `authorization denied` (theuth's engine has no Full Disk Access). Manual command in §7. Nothing in the API surface used here is TCC-gated (see §3).

Timing note for the design: the GCD `asyncAfter(90 s)` that closed the Tap window fired at +94.5 s — `DispatchQueue.asyncAfter` carries a default leeway of 10 % of the interval — while the `Timer` in `.common` modes ticked at exactly 1.00 s. Use `Timer`/`DispatchSourceTimer` with explicit leeway for breakpoint timing in the companion.

## 5 App-switch observations

Same `Idle` program: `NSWorkspace.shared.notificationCenter.addObserver(forName: NSWorkspace.didActivateApplicationNotification …)` and
`didDeactivateApplicationNotification`, printing `userInfo[NSWorkspace.applicationUserInfoKey]` → `NSRunningApplication.bundleIdentifier`.
Registration succeeded with no dialog; `NSWorkspace.shared.frontmostApplication` read `io.theuth.mac` at start (no permission needed for that either).

Observed: run A (12 s) and run B (90 s): `activations=[] deactivations=[]` — the user did not switch apps during 102 s of observation and the
rule forbids activating anything from the test. **Firing is therefore UNVERIFIED by observation**; the API has no documented permission
requirement (§3) and the observer installed cleanly. 30-second manual confirmation in §7.

## 6 Verdicts

**S3 — double-tap ⌥ detection: detector YES, permission class UNVERIFIED (documented answer: Accessibility).**
- The pure detector is done and passes 10/10 synthetic sequences (§1); it is ~60 lines with no system dependencies beyond `NSEvent.ModifierFlags`.
- Which permission a *listen-only* `flagsChanged` global monitor needs on macOS 26.6.2 could not be measured here: the test process inherited theuth.app's Accessibility trust (`AXIsProcessTrusted() = true` under the engine, `false` for the same binary under launchd), and no modifier was pressed during 98 s of listening (§2). The untrusted run was not performed because it is exactly the case that might show a TCC dialog.
- Every readable source says Accessibility: Apple's own doc sentence, Quinn/DTS ("an NSEvent global event monitor requires the Accessibility privilege"), two CGEventTap samples, one tutorial, and One's support page for the identical gesture (§3). No readable source supports the "modifier-only is exempt" folklore. **Plan for Accessibility** (one prompt, `AXIsProcessTrustedWithOptions` with the prompt option, shown from the companion's own UI); if the §7 manual test on this Mac shows `flagsChanged` arriving untrusted, the Swift wave can downgrade to "no permission for the hotkey, Accessibility only for typing/point-at".
- Design consequence measured here: TCC trusts the **responsible process**. A LoginItem helper launched by launchd/`SMAppService` (ppid 1) is its own responsible process and needs **its own** Accessibility row; a helper spawned by theuth.app inherits theuth.app's. Decide which one the companion is before writing the onboarding copy.
- Alternatives if Accessibility is unwanted: a CGEventTap listen-only tap (Input Monitoring, sandbox-compatible, has `CGPreflightListenEventAccess`/`CGRequestListenEventAccess`, DTS-recommended), or a key+modifier hot key via Carbon `RegisterEventHotKey` (KeyboardShortcuts; no dialog at all, but not a double-tap).

**S4 — permission-free breakpoint signals: YES.**
- `CGEventSource.secondsSinceLastEventType(.combinedSessionState, …)` for keyDown / mouseMoved / leftMouseDown / any-input, IOKit `HIDIdleTime`, `NSWorkspace` activate/deactivate observers and `frontmostApplication` all ran in an **untrusted launchd-spawned process** (run C) and in the engine's context with no dialog, no entitlement, and sane monotonic values (1.00 s/s drift, correct ordering between sources). Apple documents no permission for any of them (10.4+/10.6+).
- Two residuals, both observational not architectural: "drops on input" and the firing of the app-switch notifications were not seen because the user did not type/switch during the windows (manual steps 2 and 4, ~30 s each). The TCC log/db check could not run from this process (no Full Disk Access) — manual step 3.
- Use `HIDIdleTime` (hardware only) or require both when theuth itself posts synthetic input; CG's session state counted an event IOKit did not see (4.15 s gap in run A).

## 7 Manual steps for the user

All binaries are built; nothing below needs Xcode, sudo (except step 3) or the repo. Each step ≤ 30 s.

**1. S3 permission class (the decisive test — "manual step — not run").** Run the monitor as an *untrusted* process (launchd is the parent, so Terminal's or theuth's own trust is not inherited):
```
launchctl submit -l io.theuth.spike130.tap -o /tmp/130-spikes/SB/tap-manual.out -e /tmp/130-spikes/SB/tap-manual.err -- /tmp/130-spikes/SB/Tap/.build/release/Tap --seconds 20
```
Within the 20 s: tap ⌥ twice quickly (alone), then hold ⌥ for a second, then press ⌥⌘. Then:
```
sleep 22; cat /tmp/130-spikes/SB/tap-manual.out; launchctl remove io.theuth.spike130.tap
```
Read it: the header must say `AXIsProcessTrusted() = false` / `CGPreflightListenEventAccess() = false` (otherwise the run was trusted and proves nothing).
- Lines `global flagsChanged keyCode=58 …` (left ⌥; 61 = right ⌥) and one `<<< DOUBLE-TAP OPTION` → modifier-only global monitoring is **permission-free** on this Mac; `events=0` with your presses → it is **silently blocked** (Accessibility needed, as documented);
- a dialog "…would like to receive keystrokes from any application" → the class is **Input Monitoring**; the Accessibility prompt ("…would like to control this computer using accessibility features") → **Accessibility**. Click *Deny*/close either one — nothing is granted, the binary is in /tmp and the launchd job is removed by the last command. If you want the row gone from System Settings afterwards: `tccutil reset Accessibility` / `tccutil reset ListenEvent` would reset the *whole* class — don't; just delete the row in System Settings if it appears.

**2. S4 "values drop on input" (15 s):** `/tmp/130-spikes/SB/Idle/.build/release/Idle --seconds 15` from any shell; move the mouse at ~5 s, click at ~8 s, press a key at ~10 s. Expect `mouseMoved`/`anyInput`/`HIDIdleTime` to fall below 1 and climb again, `leftMouseDown` to reset at the click, `keyDown` to reset at the key. No dialog expected.

**3. TCC audit (needs an admin shell; the engine has no Full Disk Access):**
```
sudo log show --last 1h --style compact --predicate 'subsystem == "com.apple.TCC"' | grep -iE '130-spikes|release/Tap|release/Idle|ListenEvent|Accessibility' | head
```
Expect no rows naming the spike binaries (and, after step 1, at most one row that tells you which service was consulted — that is the answer to S3 as well).

**4. App-switch observer (15 s):** `/tmp/130-spikes/SB/Idle/.build/release/Idle --seconds 15`, ⌘-Tab to another app and back. Expect `didDeactivateApplication io.theuth.mac` / `didActivateApplication <other bundle id>` and the reverse pair, with no dialog.

Clean-up when done: `rm -rf /tmp/130-spikes/SB` (both launchd jobs were already removed; `launchctl list | grep spike130` should print nothing).
