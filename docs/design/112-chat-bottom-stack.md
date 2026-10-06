# 112 – Chat bottom stack: dots above the to-do; no phantom keyboard gap on a windowed iPad

Status: as-built · 2026-10-06 · amends 93 (strip placement) and 31 §8.1 (`lib/viewport.ts`). Branch `fix/chat-bottom-stack` in `~/Develop/theuth-chat-bottom`, base `5044521e` (main). Desktop UI only; no Rust, no `platform.ts`/`hotkeys.ts`/sidebar.

Commits (oldest first):
- `caa2c8f5` fix(desktop): subagent strip above the to-do card (112 §1)
- `4a69fe83` fix(desktop): keyboard-open against the window's idle height, not the screen (112 §2)
- docs(design): 112 + the dated section in 93's as-built (this note)

## 1. Strip order (Sky, 2026-10-06 13:52)

"Instead of the dots being between the to-do and the chat input, they should be above the to-do if the to-do is rendered, otherwise directly on top of the chat input."

- `src/routes/c.$session.tsx:138-140`: `<SubagentStrip/>`, `<TodoCard/>`, `<Composer/>` (was `TodoCard`, `SubagentStrip`, `Composer`). Stack: transcript · dots · to-do · composer.
- Spacing audit: the strip is one `h-7 shrink-0` row with no margin or border (the 8 px dot sits centred, 10 px of air each side); the card is `px-4 pb-2`; the composer `px-4 pb-2`. Strip absent → to-do · composer as before; to-do absent → dots · composer as before; both present → dots · card · 8 px · composer. No class changed in `SubagentStrip.tsx` or `TodoCard.tsx`; only the strip's doc comment. No test asserted the order (`TodoCard.test.ts` covers `summarize`/`note` only).
- 93's as-built got a dated section recording the ruling.

## 2. Phantom keyboard gap on iPad with a hardware keyboard

"If I tap inside the chat input on iPad and there's an external keyboard attached … a big space appears below the text input instead of it being at the bottom of the window like usual."

### Where the gap came from

`applyViewport` (`src/lib/viewport.ts`) wrote `--visible-band` (the shell's `h-band` height, `styles.css:441`) as `visualViewport.height` whenever `editing() && visible < full - 50`, with `full = max(innerHeight, screen height)` on touch. Two things fired together on a windowed iPad:

1. A Stage Manager window is always shorter than the screen, so the comparison was true on every focus (the line-34 comment named it).
2. With a hardware keyboard WebKit still shrinks the *visual* viewport by the floating shortcuts bar: ~69 px on an iPad Pro (727 → 658), bugs.webkit.org/247410, open since 2022. That is past the old `SLACK = 50`.

So `--visible-band` = `innerHeight − ~69`, the shell shrank by that much, and the window showed page background under the composer. `html[data-keyboard-open]` only zeroes `--safe-bottom`, not the cause.

### Fix

- Baseline per window, not per screen: `base = { width, height }` is the tallest of `innerHeight`/`visualViewport.height` read while no editable had focus. It is re-based on every idle call (so a Stage Manager / iPadOS resize while idle is simply the new height), on a width change even mid-edit (rotation, window resize — a keyboard never changes the width), and whenever a reading grows past it mid-edit (a keyboard never grows the window). `full = touch() ? base.height : innerHeight`; `open = editing && visible < full − SLACK`.
- `SLACK = 120` (was 50): above the 69 px shortcuts bar with margin; below any real keyboard (the smallest, an iPhone in landscape, is ~160 pt+; portrait ~260–340 pt). The number is documented at the constant.
- Both WebKit modes still work on iPhone: visual-only shrink (`innerHeight` unchanged = baseline, visual collapses) and `interactive-widget=resizes-content` (`index.html:6`; `innerHeight` collapses, baseline keeps the idle height). `screenBlock()` and the `orientation` media query are gone.
- Desktop (`touch()` false) is unchanged: `full = innerHeight`, the baseline is not consulted.

`ponytail:` ceiling (comment in the file): a height-only window resize *while editing* on a touch device still reads as a keyboard until blur — `resize` carries no cause. Upgrade path if it bites: treat a shrink where `innerHeight` and `visualViewport.height` move by the same amount while `visualViewport.offsetTop` stays 0 as a window resize.

### Tests — `src/lib/viewport.test.ts` (new)

Node environment with stubbed `window`/`document`/`matchMedia`/`HTML*Element` globals, the pattern `platform.test.ts` uses — there is no jsdom in the tree and adding a dependency for one file was not worth it (the brief said jsdom; this is the deviation). Cases: phone visual-viewport shrink opens and closes on blur; phone layout shrink (`resizes-content`) opens; windowed iPad 1024×727, focus with no change and then the 69 px shortcuts-bar shrink → not open, band 727; idle resize 1000 → 600 re-bases (focus not open; a 300 px visual then opens); rotation mid-edit re-bases; desktop unchanged.

### What Sky should see on the iPad (not runnable here)

Windowed (Stage Manager) or full-screen, Magic Keyboard attached:
1. Tap the composer. The composer stays at the bottom of the window; no band of background under it; the shortcuts bar (if iPadOS shows one) floats over the page rather than pushing the shell up. In Web Inspector, `document.documentElement.style.getPropertyValue('--visible-band')` equals `window.innerHeight + 'px'` and `html` has no `data-keyboard-open`.
2. Detach the keyboard (or hide it with the ⌨ key so the on-screen keyboard appears): the shell shrinks to the band above the keyboard, composer resting on the keys, as before.
3. Resize the window while nothing is focused, then tap the composer: still no gap. Rotate while typing: no gap after the rotation.
4. iPhone: unchanged — keyboard up, composer on the keys; keyboard down, composer on the home indicator.

If 1 still shows a gap, read `window.innerHeight − window.visualViewport.height` while focused: the shortcuts bar on that iPad is larger than 120 px and `SLACK` needs to move above it (the smallest real keyboard is still the ceiling).

## Gates

In `crates/desktop/ui` (`bun install` here and in `packages/apps` first):

1. `just ts-desktop` (cold, 536 s, `CARGO_TARGET_DIR=~/Develop/theuth-chat-bottom-target`)
```
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 36 filtered out; finished in 4.22s
```
2. `bun run check` — with Sky's stale copied set first: 0 errors in owned files, 117 total in unowned files (`routines/*`, `attention/*`, `settings/*`, `reduce.*`, `api.ts` — the known stale-binding set). Against the generated set:
```
$ tsc -b --noEmit
tsc EXIT=0
```
3. `CI=1 bun run test < /dev/null`
```
 Test Files  143 passed (143)
      Tests  831 passed (831)
```
(first full run: `surface/catalog.test.ts` failed at file load with 0 test failures — `1 failed | 142 passed`, `Tests 791 passed (791)`; alone `40 passed`, and the second full run above was clean. Load flake, not touched by this branch. `pairing.test.ts` / `mobile-zoom` passed in both runs.)
4. `bun run build`
```
build EXIT=0
```
