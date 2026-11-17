# startup-steps — the startup screen in the shutdown overlay's look

Branch `feat/startup-matches-shutdown`, cut from `887e5f5f3`.

## Sky's words (2026-11-17 11:27)

> also we have a really nice shutdown sequence in the desktop UI, the startup visual sequence should match.

## What changed (crates/desktop/ui/src)

- `components/launch/LaunchCard.tsx` (new): the shutdown card's pieces, shared. `LaunchCard` (440 px frame, header with
  spinner or `CircleXIcon text-danger`, `h2` title), `StepList` (`<ol aria-live="polite">`), `StepRow` (done / active /
  pending icons as before, plus `failed`: `CircleXIcon text-danger`, label `text-sm text-danger`, sr-only `failed`;
  optional detail, items, `children`). `LaunchStep` is the shared step type.
- `components/launch/QuitOverlay.tsx`: draws its card with `LaunchCard` / `StepList` / `StepRow`; the drain bar is the
  row's `children`. Markup byte-identical to `887e5f5f3` (snapshot test, screenshots). `QuitOverlay.css` unchanged.
- `components/launch/startup-progress.ts` (new, pure): `startupView(input)` → title, failed, the three steps, Relaunch,
  log auto-open; `lastLogLine(log)`; `STUCK_MS = 8_000`.
- `components/launch/StartupScreen.tsx`: `LocalStartup` (state + timers) over `LocalStartupView` (markup, exported for
  the tests); `LinkStartup` in the same card.
- `dev/launch-fixture.html` + `.tsx` (new): the real screens against stubbed stores and `mockIPC`, `?scene=`.
- Tests: `components/launch/startup-progress.test.ts`, `components/launch/launch-screens.test.ts` (+ snapshot).

## Decisions

1. **`owned` without touching `App.tsx`.** The screen asks `connection()` again once the client exists. The shell
   caches the connection after its first success (`launch.rs` `Launch::connection`), so the second call returns the
   cached value and starts nothing. `App.tsx` and `store/connection.ts` are unchanged. Accepted race: a call still
   queued on the shell's mutex behind a Relaunch that fails would probe/spawn once more. That needs a Relaunch click
   within milliseconds of the client appearing.
2. **Steps.** Engine: active until the client exists; while active its detail is the last non-empty log line (CSI, OSC
   and other control sequences stripped, `\r` redraws keep the last draw, capped at 200 chars, CSS-truncated to one
   line). Once it is done the detail is empty, or `Using the engine that is already running` when `owned === false`.
   Connecting: active from the client until the socket is `open`. Loading: active from `open` until `info` (the screen
   does wait for it: `routerMounted` requires `info !== null`).
3. **Failure.** Any `error` fails the step that was active and puts the error in its detail (unclipped). With no client
   that step is the engine, with the socket not open it is Connecting, once open it is Loading. The header becomes the
   danger icon with `theuth could not start`.
4. **Reconnecting title** only when `state === 'reconnecting'` and `info` had answered (the engine was up). A first
   socket that is still retrying keeps `Starting theuth`. Relaunch keeps the old rule: an error or `reconnecting`.
5. **Launch log disclosure.** `useDisclosure(disclosureKey('startup', 'launch-log'), view.logOpen)`. It is store-backed,
   so a remount (engine switch, StrictMode, the router giving way after a drop) keeps what the reader chose, and the
   auto-open only applies while the reader has not toggled it. It auto-opens on failure, or once the engine step has
   been active `STUCK_MS` since the screen mounted. One timer tick at 8 s; `waited` keeps its value after the engine
   answers, so a log that opened itself stays open. It is rendered only when the log is non-empty (as before). The
   trigger reads `Show launch log` / `Hide launch log`, with the chevron pattern from `settings/Memory.tsx`. The `<pre>`
   styling is unchanged and it auto-scrolls on new output and on open.
   Review R1: the pane shows `cleanLog(log)`, which turns `\r\n` into `\n` and drops CSI, OSC (never past a line end),
   other `ESC` sequences (`ESC 7`, `ESC ( B`), lone `\r`, the other C0 controls except `\t`/`\n`, and DEL. It shares its
   regex with the step detail's `lastLogLine`. The raw log is still what is kept and what the shell streams.
6. **Link screen.** Title `Connecting to <name>` with the spinner even on failure: the link keeps retrying, and D5 only
   asks for the step to fail. `Reaching <name>` carries today's mesh/node sentence. On failure `FailureText` (with
   `className="text-sm"`) renders inside the failed row. The four buttons are unchanged and wrap in the 440 px card. The
   outer `h-band` container is unchanged.
7. **Card label.** The startup cards are `role="region"` with `aria-labelledby` the `h2` (`useId`). The quit card keeps
   `role="alertdialog"` and `quit-title`. Callers pass `aria-labelledby` themselves, so the quit card's attribute order
   (and its markup) is unchanged.
8. **Local frame.** Same centring as the overlay (`p-4` added, `h-dvh` kept, no blur), `data-tauri-drag-region` on mac.

## Tests and gates (from crates/desktop/ui, under build-lock)

- Quit snapshot recorded on the base code first (commit `ac10fa931`, before any refactor), then passed unchanged after
  the refactor (vitest runs with `CI=true`, so a mismatch fails instead of rewriting).
- `bunx tsc -b --noEmit`: baseline 0 `error TS`, after 0 `error TS`.
- `bunx vitest run src/components/launch/ src/lib/quit-progress.test.ts src/lib/no-emoji.test.ts src/components/disclosure.test.ts`
  (gate corrected to add the disclosure ratchet): 5 files, 62 tests passed after R1.
- Full suite on the final tree (after R1; `BUILD_SLOTS=1 … bunx vitest run src`, `disclosure.test.ts` included): 359
  files, 2529 tests passed, 0 failed (`/tmp/startup-steps/vitest-full.txt`). Before R1 it was 359 files / 2528 passed.
- Earlier, one accidental full run on the base code (`vitest run -u <file>` took the path as `-u`'s argument): 2508
  passed.

## Proof

`/tmp/startup-steps/proof.md`; screenshots `/tmp/startup-steps/shots/<base|fix>-<scene>-<light|dark>.png` from the
fixture in headless Chrome (1280×800 @2x, `&still` pauses animations). The quit overlay before and after has identical
PNG md5s in both themes.
