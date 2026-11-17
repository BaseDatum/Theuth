# 198 phase 1 — build and test sheet for the release Mac 333

Design: `198-liquid-glass.md`. Branch `feat/198-liquid-glass`; test the exact sha the report names (`$SHA`).
Host: Xcode 26.0 with the iOS 26 simulator runtime, tauri-cli 2.11.4, checkout
`/Users/sky/Develop/skymoore/theuth` (remote `origin` = github skymoore/wth). Logs go to `/tmp/glass333/`.
Never touch the main checkout's tree beyond `git fetch` / `git worktree`; never push.

## 0. Worktree

```sh
export SHA=<sha from the report>
mkdir -p /tmp/glass333
cd /Users/sky/Develop/skymoore/theuth
git fetch origin feat/198-liquid-glass
git worktree add --detach /Users/sky/Develop/skymoore/theuth-glass "$SHA"
cd /Users/sky/Develop/skymoore/theuth-glass
git log -1 --format=%H   # expect: $SHA
```

## 1. Dependencies, bindings, wire

```sh
cd /Users/sky/Develop/skymoore/theuth-glass
(cd packages/apps && bun install --frozen-lockfile)
just ts-desktop 2>&1 | tee /tmp/glass333/ts.log
# = cargo test -p theuth --bin t --quiet export_bindings; bun install; bun run wire:generate; copy into ui/src/rpc/bindings
(cd crates/desktop/ui && bunx tsc -b --noEmit) 2>&1 | tee /tmp/glass333/tsc.log
(cd crates/desktop/ui && bunx vitest run src/lib/native-chrome.test.ts src/components/chat/native-chrome.test.ts) 2>&1 | tee /tmp/glass333/vitest.log
```

Expect: `test result: ok. 1 passed` in ts.log, `Generated … methods`; tsc prints nothing; vitest `Tests  15 passed (15)`.

## 2. Build for the iOS 26 simulator

```sh
cd /Users/sky/Develop/skymoore/theuth-glass/crates/desktop
rustup target add aarch64-apple-ios-sim
TAURI_FRONTEND_PATH=ui cargo tauri ios build --debug --target aarch64-sim --config tauri.ios.conf.json \
  2>&1 | tee /tmp/glass333/build.log
find gen/apple/build -name '*.app' -maxdepth 4 -print
```

Expect: the `Build Rust Code` phase compiles `tauri-plugin-glass`, whose build script copies the tauri-api Swift
package to `crates/tauri-plugin-glass/.tauri/tauri-api` (gitignored) and swift-rs builds the `tauri-plugin-glass`
package (`swift build` lines in the log, no `error:`); `** BUILD SUCCEEDED **`; one `theuth.app`.

If this tauri-cli refuses a simulator `build`, use dev mode instead (it builds, installs and launches; keep it
running for the steps below and stop it with Ctrl-C at the end):
`TAURI_FRONTEND_PATH=ui cargo tauri ios dev "iPhone 16 Pro" --config tauri.ios.conf.json 2>&1 | tee /tmp/glass333/dev.log`.

Failure triage: `grep -nE "error:|warning: .*glass|ld: " /tmp/glass333/build.log`. Swift errors point into
`crates/tauri-plugin-glass/ios/Sources/*.swift`; a link error naming `init_plugin_glass` means the swift-rs library
was not linked (check the `cargo:rustc-link-lib` lines of the plugin's build output under `target/`).

## 3. Simulator

```sh
xcrun simctl list runtimes | grep -i 'iOS 26'          # expect one iOS 26.x runtime
UDID=$(xcrun simctl create glass-198 "iPhone 16 Pro" "$(xcrun simctl list runtimes | awk '/iOS 26/{print $NF; exit}')")
xcrun simctl boot "$UDID"; open -a Simulator
xcrun simctl install "$UDID" "$(find gen/apple/build -name '*.app' -maxdepth 4 | head -1)"
xcrun simctl spawn "$UDID" log stream --level debug --predicate 'process == "theuth"' > /tmp/glass333/sim.log 2>&1 &
LOGPID=$!
xcrun simctl launch "$UDID" io.theuth.ios
```

Expect: the pairing screen (Scan QR · Paste key), no crash. Safari › Develop › Simulator › theuth gives the web
console for every step below (`glass …:` warnings come from `lib/native-chrome.ts`).

## 4. Pair with 333's own engine

The daemon must be running with link enabled (TUI `/link on`, or desktop Settings › Network › Devices).

```sh
t --link 2>&1 | tee /tmp/glass333/link.txt     # prints a QR and https://<relay>/link#1.<…>.<…>, waits for "paired:"
KEY=$(grep -oE 'https://[^ ]+/link#1\.[A-Za-z0-9_.-]+' /tmp/glass333/link.txt | head -1)
xcrun simctl openurl "$UDID" "$KEY"
```

Expect: the app comes forward and pairs; `t --link` prints `paired: iPhone` and exits. If `openurl` opens Safari
instead (no universal-link association in the simulator), paste: `printf %s "$KEY" | xcrun simctl pbcopy "$UDID"`,
then in the app long-press the Paste key box → Paste → Pair. Same expectation.

## 5. Open a chat with content

Tap the glass drawer button (top-left), pick a session with at least two screens of messages; or send
`Write 60 numbered lines, one short sentence each, about glaciers.` in a new chat and wait for the reply.

Expect (the phase-1 checks):

1. **Bar:** under the status bar, three glass controls: round `sidebar.left` left, the title pill with the session
   title and a small chevron centred, round `ellipsis` right. No web header row behind or below them.
2. **Under-scroll:** at the top of the history the first message starts just below the pill; scrolling up, text
   passes under the bar and the status bar and is visibly refracted by the glass. No empty 52 px band.
3. **Drawer:** tap → light haptic (device only; the simulator has none), the sidebar drawer slides in and the glass
   bar and button fade out; closing the drawer brings them back.
4. **Title:** tap → the chevron panel (model, effort, context, cost, memory, … rows) hangs just under the bar; the
   bar stays. Tap again → it closes.
5. **⋯:** tap → the Session menu dropdown opens under the ⋯ position; the bar stays. Pick Rename… → a dialog: the
   bar hides; cancel → it returns.
6. **Scroll-to-bottom:** scroll up more than 80 px → a round glass `arrow.down` fades in, centred, about 12 px above
   the composer; tap → the transcript jumps to the bottom and the button fades out. No web (outline) button ever.
7. **Keyboard:** tap the composer → keyboard up; scroll up → the button sits just above the composer, above the
   keyboard; dismiss the keyboard → it follows the composer down.
8. **Sheets:** open a full-screen surface (artifacts, terminal viewer, search) → bar and button hidden while open.
9. **Other pages:** Settings (from the drawer) → no glass bar (it hides once the chat header unmounts); back to a
   chat → it returns with that chat's title.
10. **Accessibility:** VoiceOver (Accessibility Inspector › Simulator) reads "Open sidebar", the title,
    "Session menu", "Scroll to bottom"; Settings › Accessibility › Larger Text at the largest size: the controls keep
    44 pt, long-press shows the large content viewer, the title stays one truncated line.

## 6. Screenshots and video

```sh
for mode in light dark; do
  xcrun simctl ui "$UDID" appearance $mode
  sleep 2
  xcrun simctl io "$UDID" screenshot /tmp/glass333/chat-$mode.png
done
xcrun simctl io "$UDID" recordVideo --codec h264 /tmp/glass333/scroll.mp4 &   # scroll the chat up and down ~10 s
VIDPID=$!; sleep 12; kill -INT $VIDPID
```

Expect: `chat-light.png` and `chat-dark.png` show the bar over chat text in each appearance (with the app's theme on
System; with Settings › General › Appearance forced, the glass follows the app's theme instead), `scroll.mp4` shows
the text sliding under the glass.

Fallback check (no build needed beyond this one): on an iOS 18 simulator, if one is installed, the same app shows the
web header and the web scroll button exactly as on main (`is_supported` is false there).

## 7. Collect and clean up

```sh
kill $LOGPID
ls ~/Library/Logs/DiagnosticReports | grep -i theuth     # expect nothing new
xcrun simctl shutdown "$UDID"; xcrun simctl delete "$UDID"
cd /Users/sky/Develop/skymoore/theuth && git worktree remove --force /Users/sky/Develop/skymoore/theuth-glass
```

Report: the sha, each step's result against its Expect line, `/tmp/glass333/*` (build.log, sim.log, vitest.log,
the two screenshots, scroll.mp4), and on any failure the matching log excerpt and crash report.
