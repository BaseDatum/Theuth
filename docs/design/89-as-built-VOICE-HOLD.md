# 89 VOICE-HOLD — as built

The composer's hold-to-record voice note, rebuilt: hold, slide left to cancel, slide up to lock, a floating orb + lock pill over the page, a locked bar with Delete and Send. Desktop and phone share it. Branch `feat/89-voice-hold`, based on main `fca080be`.

## Gesture table

| Input | Reducer (`lib/hold.ts`) | What the user sees | Haptic |
|---|---|---|---|
| Press | `idle → pressed` | mic button scales to 1.12 (120 ms ease-out) | `tick` |
| Release within `TAP_SLOP` before `HOLD_MS` | `pressed → idle`, verb `tap` | dictation, as before | — |
| Held ≥ `HOLD_MS` (200 ms) | `pressed → held{axis:none}`, verb `record` | orb (64 px, `bg-danger`) pops in on the button with a level halo; lock pill above it; row: red dot + `m:ss` left, wave in the box with "‹ slide to cancel" where the placeholder was; `+`, Talk, Send/Steer/Stop hidden | `tick` |
| Travel ≥ `AXIS_PX` (12 px) on one axis | `held.axis` commits to `x` or `y`; the other axis reads 0 for the rest of the hold | — | — |
| Slide left (axis x) | `dx` tracked | hint moves `translateX(dx)` and fades `1 − (−dx)/96`; orb stays put | — |
| Slide left ≥ `CANCEL_PX` (96 px), on the move | `held → idle`, verb `cancel` | hint slides out (150 ms), orb + halo shrink to 0 (200 ms), bin pops into the timer's place and wiggles once; row resets after 420 ms; the release that follows does nothing | `cancel` |
| Slide up (axis y) | `dy` tracked | pill rises `translateY(−dy × 0.45)`, chevron fades, open lock fills toward `fg-accent` | — |
| Slide up ≥ `LOCK_PX` (72 px), on the move | `held → locked`, verb `lock` | lock icon snaps to `Lock` with a 1 → 1.15 → 1 bounce (180 ms), then pill fades and the orb shrinks into the button's place (220 ms); row becomes `[Delete] [● m:ss] [wave] [Send]` | `lock` |
| Release anywhere while held (outside the button included) | `held → idle`, verb `send` | orb shrinks into the button (160 ms); `onStop(false)`; transcribing follows | `send` |
| Release < `MIN_NOTE_MS` (800 ms) after `rec.start()` | (Composer, not the reducer) | same shrink; no upload; toast "Hold to record, release to send" (2 s) | `send` |
| Locked: tap Send / tap Delete / Esc | `locked → idle` via `reset` once `recording` ends | `onStop(false)` / `onStop(true)` / `onStop(true)` | — |
| `pointercancel`, window `blur`, page hidden, while held | `held → locked`, verb `lock` | as lock: the note goes on hands-free | `lock` |
| Same interruptions while merely pressed | `pressed → idle` | nothing | — |

Constants: `HOLD_MS = 200`, `LOCK_PX = 72`, `CANCEL_PX = 96`, `AXIS_PX = 12`, `TAP_SLOP = 10`, `MIN_NOTE_MS = 800`. The reducer stays pure and duration-free; `MicButton` owns the timer, the pointer capture and the DOM; `ring()` and its tests are gone with the ring.

## Floating layer anatomy (`components/chat/VoiceHold.tsx`)

`VoiceLayer` renders through `createPortal(document.body)` a `fixed inset-0 pointer-events-none z-70` layer while the phase is held or locked (plus the exit animation), anchored at the mic button's `getBoundingClientRect()` centre read at `record` and refreshed on window `resize` / capture-phase `scroll`.

- **Halo**: 64 px circle, `bg-danger` at 0.22 opacity, behind the orb; `scale(1 + 0.7 × level)` where `level` is `dbToLevel(rmsDb(frame.timeDomain))` from the recorder's `FrameSource<VisualFrame>` (the wave's own analyser; no second audio graph), with fast attack / 0.85 decay so it never jitters. Static at scale 1 under reduced motion.
- **Orb**: 64 px `bg-danger` circle, `Mic` icon in `text-primary-foreground`, `shadow-lg`; enters with `voice-pop` (0.6 → 1, 220 ms, `cubic-bezier(0.34, 1.56, 0.64, 1)`); exits by transition — cancel `scale(0)` 200 ms ease-in, send `scale(0.56)` 160 ms, lock `scale(0.56)` 220 ms after the 180 ms bounce.
- **Lock pill**: 40 × 88 px, `rounded-full bg-popover border border-border-subtle shadow-lg`, bottom edge 16 px above the orb; `LockOpen` 20 px at the top whose colour is `color-mix(fg-accent dy/72, fg-secondary)`, `ChevronUp` 16 px `text-fg-tertiary` bobbing ±3 px / 1.2 s (`voice-bob`) with opacity `1 − dy/72`; enters with `@starting-style` (translate 8 px + fade, 220 ms); on lock swaps to `Lock text-fg-accent` with `voice-bounce`.
- Pointer moves go straight to element styles through `LayerHandle.track(h)` (pill transform, chevron opacity, lock colour) and the hint ref (transform + opacity); React state changes only on phase changes.
- **Row pieces** (in place, no portal): `RecordingMeta` (dot + `m:ss`, or the `voice-trash` bin while `cancelled`), `CancelHint` (`ChevronLeft` 14 px + "slide to cancel" with the `voice-hint` leftwards 1.6 s shimmer, clipped to the text box so it slides out inside it), `DeleteNote` (ghost round `Trash2`, "Delete recording", hover `text-danger`), `SendNote` (round `bg-fg-accent`, `ArrowUp` 18 px, "Send voice note"). `SendNote` takes the mic's slot under its own React `key`, so the `click` that ends the locking pointer sequence cannot land on it.
- `document.body.voice-hold` (`user-select: none; overscroll-behavior: none`) while held or locked; the mic button keeps `touch-none select-none`, `onContextMenu` prevented, and `data-drag-row` (the existing `-webkit-touch-callout: none` hook).
- Reduced motion: every `voice-*` animation and the shimmer are off, `ms()` zeroes the exit timers and transition durations, the halo is static, the press transition is `motion-reduce:transition-none`. Verified: `document.getAnimations().length === 0` while held under emulated `prefers-reduced-motion: reduce`.
- Theme: tokens only (`bg-danger`, `text-fg-accent`, `bg-popover`, `border-border-subtle`, `text-fg-secondary/tertiary`, `text-primary-foreground`).

## Composer (`components/chat/Composer.tsx`)

`record()` stamps `startedAt = performance.now()` before `rec.start()`; `rec.onstop` discards (no upload) with the toast when `!discard && now − startedAt < MIN_NOTE_MS`. `MicButton` gets `source={noteSource}`, the `hint` ref and `onStage={setStage}`; the row keys off `stage` (`idle | held | locked | cancelled`): `+` hidden, `RecordingMeta` left of the box, `DeleteNote` when locked, `CancelHint` over the box while held/cancelled, Talk and the trailing Send/Steer/Stop only while idle. Escape still deletes (unchanged handler).

## Haptics (`lib/haptics.ts`)

| Effect | Browser with `navigator.vibrate` (Android) | Tauri on a phone (`isTauri() && touch()`) | Desktop |
|---|---|---|---|
| `tick` | 10 ms | `impactFeedback('light')` | — |
| `lock` | 20 ms | `impactFeedback('medium')` | — |
| `cancel` | `[10, 40, 10]` | `notificationFeedback('warning')` | — |
| `send` | 10 ms | `impactFeedback('light')` | — |

The plugin is a lazy `import()` inside try/catch; nothing is awaited in the gesture; nothing throws (`lib/haptics.test.ts`). Wiring, copy of the barcode-scanner precedent: `crates/desktop/Cargo.toml` `tauri-plugin-haptics = "2"` in the iOS/Android target table; `src/lib.rs` `.plugin(tauri_plugin_haptics::init())` inside `#[cfg(mobile)] mod mobile`; `capabilities/mobile.json` `"haptics:default"`; `ui/package.json` `"@tauri-apps/plugin-haptics": "^2"` (2.4.1 installed). `Cargo.lock` gained `tauri-plugin-haptics 2.3.3`; no other package changed version (cargo re-rendered some dependency lists).

## Tests

`lib/hold.test.ts` rewritten (9 tests): tap; record at timer; axis commit at 12 px; lock on the move at 72 px up; cancel on the move at 96 px left; diagonal — the committed axis wins, both crossing in one move → the larger travel; release anywhere in held → send; interrupt → lock (held) / drop (pressed); reset from every phase. `lib/haptics.test.ts` (3 tests): no-throw without `vibrate`, no-throw with a failing plugin import, patterns reach `vibrate`.

## Gates

```
just ts-desktop                      → test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 36 filtered out; finished in 2.70s
                                        EXIT=0
bun install                          → 619 packages installed [5.70s]
bun run check                        → $ tsc -b --noEmit
                                        check EXIT=0
CI=1 bun run test < /dev/null        →  Test Files  131 passed (131)
                                              Tests  725 passed (725)
                                        test EXIT=0
cargo check -p desktop               → warning: the following packages contain code that will be rejected by a future version of Rust: block v0.1.6
                                        EXIT=0
```

No flaky rerun was needed (`src/link/pairing.test.ts` passed in the full run).

## Visual self-check

A throwaway Vite page (`dev-voice.html` + `src/dev-voice.tsx`, a composer-like row with a synthetic `FrameSource`; deleted, not committed) driven by headless Chromium through pointer events at 420 × 400: hold → orb/halo/pill/dot/timer/hint; slide up 48 px → pill at `translateY(-21.6px)`, chevron 0.33; up 74 px → lock bounce, locked bar `[Delete recording] [● m:ss] [wave] [Send voice note]`, the ending click did not send; Send → idle; hold + slide left 60 px → hint at `translateX(-60px)`, faded; 98 px → bin in the timer's place, row idle after; tap → `tap`; hold + diagonal release outside → `send`; hold + window `blur` → locked; Delete → idle; reduced motion → 0 running animations. No `?sim=` path was added to the tree.

## Not verified here

- iOS/Android build (`tauri-plugin-haptics` compiles only for those targets; the macOS `cargo check` cannot exercise it) and real device haptics (Taptic strengths, Android vibrate patterns).
- Real microphone: the harness used a synthetic frame source; the `MIN_NOTE_MS` toast path ran only through the code path reading (`rec.onstop`), not a live `MediaRecorder`.
- Touch on a real phone: pointer capture + `touch-action: none` on iOS WebKit followed the finger in the previous build; unchanged here.

## Deviations

- The Send button's icon colour is `text-primary-foreground` (the tree's pairing with `bg-fg-accent`, as on the composer's Send); the brief's `text-bg-page` is not a Tailwind utility in this theme.
- The `+` (attach) button and the trailing Send/Steer/Stop hide while held too, not only while locked: the dot + timer take `+`'s place, and the row would otherwise overflow on a phone. They return on reset.
- The cancel hint is clipped to the text box as it slides left (otherwise it crossed the dot/timer). Not in the brief; purely cosmetic.
- `Cargo.lock` changed (one new package) although not on the file list — unavoidable for the Cargo.toml dependency.
- Mic `aria-label` while held is "Recording — release to send"; locked has no mic button (Send/Delete carry their own labels).

## Follow-ups to tune after a device test

`LOCK_PX`/`CANCEL_PX` against thumb reach on small phones; halo gain (0.7) and decay (0.85) against a real mic; whether the hint needs a backdrop over the wave; Space-hold on the keyboard (brief's follow-up).
