# 96 POINT-AND-SAY — as built, PANEL follow-up (voice panel states and lifetime, panel position, draw-only-while-mouse-held)

## Branch + base sha

`feat/96b-panel` in `/Users/Sky.Moore/Develop/theuth-wt-96b-panel`. Base `ead30467` = `main` (52a70f15 "Merge feat/96-daemon" + one site fix), which contains design 96, its three as-built notes and the first UI build (`96-as-built-UI.md`). Bindings generated here with `just ts-desktop` (gitignored).

## Commits (oldest first)

- `374d5cb9` feat(desktop): voice panel states and lifetime — working/speaking/done on local playback, error, placing (96 PANEL)
- `9971989d` feat(desktop): panel position and 'Draw the trail' settings; point_configure; pen-aware overlay (96 PANEL)
- `162e26e8` docs(design): 96 §5.1/§5.3/§5.4/§14 amended for the panel follow-up (PANEL)
- (this note) docs(design): 96 as-built FIX-PANEL

## What changed (all under `crates/desktop/ui/src/` unless noted)

- `lib/voice-key.ts`
  - `:37-47` `Settings.annotate: Annotate` (`'always' | 'mouse'`, default `'always'`), persisted with the rest under `localStorage['voiceKey']`.
  - `:75` phases `idle | listening | sending | working | speaking | done | hint | error | placing`; `State.spoken` (`:96-97`, the entry last played here); `Event` (`:117-141`): `approval { on }`, `queued` (engine `SpeechStarted`), `playing { entry }` / `silent` (this webview's playback), `spoken`, `place` / `placed`; `replying(phase)` (`:144`).
  - `reduce` (`:147-184`): `sent → working`; `delta`/`approval`/`error` only inside a reply; `error → error` phase; `queued: done → working`; `playing: working|done → speaking`; `silent: speaking → working`; `spoken: working → done`; `place: idle → placing`; `placed: placing → idle`.
  - `statusLine(slice, tool, approval)` (`:187-210`), `lastSentence(text)` (`:213-216`).
  - Constants (`:224-238`): `MOUSE_HINT`, `DONE_MS = 2000` (was 6000), `ERROR_MS = 4000`, `SPEAK_WAIT_MS = 1500` (was 4000, see Deviations).
  - Lifetime (`:372-406`): `turn(finished)` ← `TurnStarted`/`TurnFinished`; `playback(entry | null)` ← the panel's `speech.useSpeaking()`; `settle()` arms the `SPEAK_WAIT_MS` grace only when `turnDone && !playing` and ends the reply (`onSpoken` → `done` → `DONE_MS` → `point_done()`) if nothing started playing meanwhile. `SpeechFinished` is no longer listened to.
  - `onStart` (`:283-302`): leaves `placing` (tells main), stops a still-playing answer (`stopSpeech`), resets `turnDone`/approvals.
  - `Error` (`:481-489`): `error` phase, `ERROR_MS`, then `stopSpeech` + `point_cancel()`. `ApprovalRequested`/`ApprovalResolved` (`:473-480`): ids tracked; `approval.on` flips back when the last one resolves.
  - Placing (`:408-431`, `:461-465`): `voice-key:place { on, reset }` listener; `donePlacing()` → `placed` + `voice-key:placed` to main; `resetPlace()` = `islandPlace({})` → `clearPosition(key)`. `cancel()` while placing = Done.
- `lib/voice-key.test.ts` — see Tests written.
- `components/voice/VoicePanel.tsx` (rewritten, 319 lines): feeds `playback(speech.useSpeaking()?.entry)` (`:93-94`); height = `fit` (measured from the subtitles' content through a `ResizeObserver` ref, ≤ 132) while `speaking`, 44 otherwise; `islandShow` debounced 40 ms per settled size (`:97-120`); the drag-end saver while `placing` (`:122-140`, the `Island.tsx:127-143` logic: `onMoved` → 300 ms → `setPosition(key, [x, y])`); rows: hint, placing (`data-tauri-drag-region`, "Drag me where you want the voice panel", Done), error (red), working (`LoaderCircle` spinner + `statusLine` + "— Open chat" on approval + Stop), speaking (`VoiceWave h-7 w-16` + Stop; subtitles below), done (`LastWords` muted or "Done" + Open chat), listening/sending (as before; `MOUSE_HINT` in mouse mode; "Sending…" when no words yet).
- `lib/tauri.ts:119-130` `PointSample.pen`, `PointSamples.pen`, `Annotate`, `VoiceKeyPlace`; `:144-146` `pointConfigure(annotate)` → `point_configure`; `:156-160` `voiceKeyPlace` (`emitTo('island', 'voice-key:place')`), `onVoiceKeyPlace`, `voiceKeyPlaced` (`emitTo('main', 'voice-key:placed')`), `onVoiceKeyPlaced`; `:7` `emitTo` import.
- `lib/meeting-settings.ts:34-35, 64-67` `clearPosition(key)` (additive).
- `components/settings/VoiceKey.tsx:54-82` rows "Draw the trail" (Select: "Always while the key is held" / "Only while the left mouse button is held") and "Panel position" ("Show panel to move it" ↔ "Hide panel", "Reset to default"); `:20-30` the `voice-key:placed` listener flips the button back; leaving the tab hides a panel it showed.
- `App.tsx:274, 330-337` `usePointConfigure()` right after `useVoiceKeyBinding()`: `point_configure(annotate)` at boot and on every change (a rejection — the shell without SHELL2 — is `console.warn`ed).
- `routes/overlay.tsx:27-30, 59-60, 67-84` `pen === false` → the polyline breaks (`down = false`), the points are not drawn, the ring follows at `globalAlpha 0.4`; `point:start` writes `html[data-annotate]` from `annotateMode()`.
- `lib/trail.ts:13-21` `annotateMode()` reads `localStorage['voiceKey'].annotate` without importing the store (the overlay stays import-free).
- `styles.css:520-523` `html[data-annotate="mouse"] { cursor: crosshair }`.
- `docs/design/96-point-and-say.md` §5.1 (table rewritten + the lifetime rule), §5.3 and §5.4 (amendment paragraphs), §14 (amendment paragraph).

## Tests written

`lib/voice-key.test.ts` (17; the previous 10 kept, two of them rewritten for the new events):
- `sent → working collects deltas and the approval; playback flips working ↔ speaking; spoken → done; stray events are ignored`
- `the desk's Error is its own phase with the message; it is ignored outside a reply`
- `the reply is over when the turn has finished AND nothing plays here; the engine finishing synthesis plays no part` (fake timers: `turn(true)` while playing → stays `speaking` past every timer; `playback(null)` → `working` → `done` at exactly `SPEAK_WAIT_MS` → `idle` after `DONE_MS`)
- `a block that starts inside the grace keeps the panel; playback ending before the turn does not end the reply` (plus a late block while `done` cancels the conceal)
- `a turn with nothing to say (voice off, empty block) ends after the grace alone`
- `the working status line follows the slice: waiting, reasoning, the running tool, vision, streaming; approval wins` (+ `lastSentence`)
- `placing: entered from idle only, left by Done or a press`
- `the trail setting defaults to always and persists next to the other voice-key settings` (stubbed storage, module re-imported)

## Gates

1. `just ts-desktop` (background job, `cargo test -p rpc --features ts --lib --quiet export_bindings`): `test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 36 filtered out; finished in 3.28s` → `EXIT=0`; 467 files under `src/rpc/bindings/`.
2. `cd crates/desktop/ui && bun install && bun run check` → `$ tsc -b --noEmit` with no errors (after `bun install` in `packages/apps` as the UI note required).
3. `cd crates/desktop/ui && CI=1 bun run test < /dev/null` → `Test Files  134 passed (134)` / `Tests  760 passed (760)` (first run; `pairing.test.ts` did not flake this time).

No Rust gates, no `cargo tauri`, no desktop app launched.

## Deviations

- **Playback is read through `speech.useSpeaking()` in the panel, not `speech.idle()`.** `idle()` and `subscribe` are not exported from `lib/speech.ts` (not my seam); `useSpeaking()` is the only subscriber. `VoicePanel` forwards it to `voiceKey.playback(entry | null)`; the orchestrator holds `playing`. The panel is mounted for every non-idle phase, so nothing is missed between `sent` and `idle`.
- **`SPEAK_WAIT_MS` is 1.5 s, not 4 s, and it is the single grace for "turn done and nothing plays".** `speak` answers its id as soon as the voice actor accepts (`rpc/src/api.rs:4584-4602`) and `setSpeaking` follows that round trip; the only gap between the turn's end (or a block's end) and the next block's `useSpeaking()` is that RPC. 4 s would have added to the 2 s `done` after every reply. A `speak` slower than 1.5 s shows `done` for a moment and reopens on `SpeechStarted`/playback (tested).
- **The engine's `SpeechStarted` is still listened to** (`queued`): it leaves `done` early and restarts the grace; it never ends anything. `SpeechFinished` is gone from the panel.
- **A press mid-reply stops the old answer** (`speech.stop()` + `stop_speaking(null)` in `onStart`). The brief says "unchanged"; the brief's new question would otherwise be dictated over the panel's own TTS. One line; reverse it if Sky wants the overlap.
- **A Stop square in `working` too.** The brief lists only a spinner and the status line; without it a non-activating panel has no way to dismiss a long tool run (Esc needs key focus). Esc stays wired.
- **`done` keeps an "Open chat" link** (the spec's §5.1 had it on `done`; the brief's `done` row lists only the sentence). It fits the strip.
- **"Sending…"** appears only when no words are on screen yet; the muted transcript + dots remain the sending look (the brief's "text muted ('Sending…')" read both ways).
- **The drag-end saver is duplicated, not extracted from `Island.tsx`.** `components/meeting/Island.tsx` is outside the seam; the 12 lines (`onMoved` → `MOVE_SAVE_MS` → `outerPosition().toLogical(scaleFactor())` → `setPosition`) are copied with a pointer to the original (the UI build did the same for `speakerSource`).
- **"Reset to default" runs in the island webview** (`voice-key:place { reset: true }`), not in Settings: `positions` lives in each webview's own zustand copy, and the island is the one that places and saves. `islandPlace({})` both answers the key and places at the notch default, so the reset is one call plus `clearPosition(key)`. Settings' button sends `{ on: placing, reset: true }` so a visible placing panel jumps to the default and stays up.
- **The Settings button reads "Hide panel" while placing** (sends `{ on: false }`); the brief named only the panel's Done. Both end `placing` and flip the button through `voice-key:placed`.
- **Mouse-mode hint replaces the default hint on the first three uses** ("Hold the left mouse button to draw" instead of "Hold to talk · double-tap to lock"); both do not fit beside the wave and the clock at 440 px. Same `hintsShown` counter.
- **`annotateMode()` parses `localStorage` directly in `lib/trail.ts`** so the overlay keeps booting without the store graph; the key and field are the ones `useVoiceKeySettings` writes.
- **`ApprovalResolved` carries no session** (`crates/tools/src/approval.rs:56-63`); the panel matches ids it saw in `ApprovalRequested` for the desk.

## Needs from siblings

- SHELL2: `point_configure(annotate: "always" | "mouse")` and `pen` on `point:samples` / `point:stop.samples[i]` as the brief states; the panel already sends and reads them. Until merged, `point_configure` rejects at boot ("command not found") and is only `console.warn`ed; `pen` is `undefined` → the overlay draws as before (`pen === false` is the only lift).
- DAEMON2: `Sample.pen` in the generated binding; the panel passes `samples` through unchanged (type-checks today because `samples` is a variable, not a literal).
- Pre-existing, not this seam (for Sky/the orchestrator): `lib/meeting-settings.ts` writes the whole object from each webview's in-memory copy ("each webview persists what it reads"), so a save from the main webview (`setModel`, `setSplit`, `setHotkey`, `setHidden`) rewrites `positions` from main's stale copy and can undo a drag saved by the island. The new "Reset to default" and drag-to-place save from the island like the meeting island's drag does, so they are no worse — but the same clobber applies to them. A `storage` event listener or re-reading storage before `save` in `meeting-settings.ts` would fix it for both.

## Not built / Not verified

- Panel visuals and the lifetime on the live app (strip ↔ grown sizes, the status line's wording per activity, the 1.5 s + 2 s timing, `error` in red), the drag-to-place flow (`data-tauri-drag-region` in a non-activating `NSPanel`, the saved spot on the next show), mouse-mode drawing (pen lift, the 40 percent ring, the crosshair) — integration only; no desktop app launched here, the `/island` route mounts only once an engine connects.
- `point_configure`'s acceptance by SHELL2's shell and `pen` on the wire (SHELL2 in parallel).
- WebAudio playback in the island webview (spike 1) — unchanged code path; if it never starts, `useSpeaking()` stays null and every reply ends `SPEAK_WAIT_MS` + `DONE_MS` after `TurnFinished` (the panel would then conceal 3.5 s after the turn with the text never shown as subtitles).
- `ResizeObserver` fit measurement and the 40 ms `islandShow` debounce were not exercised (no DOM in the test runner).
