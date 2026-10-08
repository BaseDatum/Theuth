# 138-DIVIDERS as built — finger-draggable dividers (iPad)

Branch `fix/touch-dividers` on base `07f8e1f3`. Ask (Sky, 2026-10-08): "It's very difficult almost impossible to resize chat session panes on iPad. Like you can't actually grab the dividers."

## Mechanism (base `components/chat/Panes.tsx:71-100`, copied at three more handles)
1. The whole target was the 4 px line (`w-1`/`h-1`); a fingertip resolves to one point, so it missed and the touch focused the neighbouring pane instead.
2. No `touch-action: none`: WebKit took the touch as a pan and sent `pointercancel` after a few px; `preventDefault` on `pointerdown` does not stop that.
3. `pointercancel`/`lostpointercapture` unhandled, `pointermove`/`pointerup` on `window`: after a cancelled drag the listeners stayed and the next finger anywhere kept resizing.
4. Hover-only highlight: nothing on touch said "draggable" or "dragging".
Same shape at `ArtifactPanel.tsx` (sheet left edge), `codemap/CodeMapHost.tsx` (separator), `notes/NoteChat.tsx` (panel top edge).

## Built
- `lib/resize-drag.ts` (pure, tested): `createResizeDrag({ onStart?, onMove(x, y), onEnd? })` → `{ down(e): boolean, move(e), end(e?), live }`. Starts on `isPrimary` only (mouse: `button === 0`), follows one pointer id, ignores other pointers, ends once on up/cancel/lost-capture/unmount; nothing emits after the end. Also `clamp`, `clampRatio` (0.15–0.85), `RATIO_STEP` 0.05, `PX_STEP` 16, `keyDelta(key, orientation)`.
- `components/ui/resize-handle.tsx` `ResizeHandle` props: `orientation` (`vertical` | `horizontal`, of the line), `placement` (`center` | `start` | `end`: where the hit zone lies), `className` (line thickness/idle colours; defaults `w-1`/`h-1`, `bg-border-subtle hover:bg-fg-accent/60`), `value {now,min,max}` → `aria-valuenow/min/max`, `aria-label`, `onStart(e, handleEl)`, `onMove(clientX, clientY)`, `onEnd()`, `onNudge(±1)` (`+1` = line moves right/down). The handle owns gesture, hit area, visuals, a11y; callers keep their sizing math and storage.
- Gesture glue: `setPointerCapture` on the hit zone; `pointermove`/`pointerup`/`pointercancel`/`lostpointercapture` listeners on that element only, attached at down and removed at end (never `window`); `touch-none select-none [-webkit-touch-callout:none]`; `documentElement` cursor (`col-resize`/`row-resize`) + `user-select:none` while live, restored on end; unmount mid-drag ends it.
- Hit zone: invisible, absolutely positioned child, 10 px (fine) / 32 px (`pointer-coarse:`) across, full length; `relative z-10` on the handle so it sits above pane content, below popovers (z-50).
- Visuals: grip pill (`bg-fg-tertiary`, 4×32 or 32×4, `rounded-full`) on `pointer-coarse` only; while dragging the line and pill take `bg-fg-accent` (`data-dragging`). Fine pointers keep the site's hover colour. Focus: `focus-visible:ring-2 focus-visible:ring-ring`.
- A11y: `role="separator"`, `aria-orientation`, `tabIndex={0}`, arrow keys along the line's axis call `onNudge`.

## Per-site
| Site | Line | Placement | Sizing (unchanged math) | Nudge |
|---|---|---|---|---|
| `Panes.tsx` `Divider` (also used by `ControlCenter.tsx`, `RoutineDetail.tsx`) | 4 px, `bg-border-subtle` | center | parent rect → `clampRatio` → `onRatio` (`usePanes.resize`) | ±0.05 ratio; `aria-valuenow` = ratio×100, 15–85 — only when the new optional `ratio` prop is passed (Branch passes it; the two other callers are off-limits tonight and keep drag-only) |
| `ArtifactPanel.tsx` | 6 px, transparent, `hover:bg-border-strong`, `hidden sm:block` | start (extends LEFT, outward over the overlay) | `clamp(innerWidth − x, MIN_W, innerWidth/2)` → `setWidth` | ∓16 px |
| `CodeMapHost.tsx` | 6 px, `bg-border-subtle` | center | `clampW(paneRight − x)` → `setWidth` (localStorage effect unchanged) | ∓16 px |
| `NoteChat.tsx` | 6 px, transparent, `hover:bg-layer-2` | start (extends UP over the editor, never down over the header buttons) | `clamp(startH + startY − y, MIN_HEIGHT, paneH − 120)` → `set(note, {height})` | ∓16 px |

Decisions: centred fine-pointer zones bleed 2–3 px into the adjacent pane edge (brief's default kept; macOS overlay scrollbars sit inside that edge only while scrolling). `Divider.ratio` optional so the two untouchable callers still compile. Coarse-pointer grip hides on fine pointers rather than fading, to keep the 4 px line as it was.

## Harness (throwaway, `/tmp/138-harness`, not committed; headless Chrome via CDP, touch emulation; shots in `/tmp/138-divider-shots/`)
```
PASS (a) new Divider: finger at line+12px dragged 200px → divider moved 205.5px (ratio 0.680)
PASS (b) base Divider (control): same gesture → divider moved 0.0px (4px strip missed)
PASS (b') base Divider dead-on the 4px line → moved only 19.4px of 200 (browser took the pan, pointercancel)
PASS (c) new Divider: cancelled after 60.1px; later unrelated 150px touch moved it 0.0px; data-dragging cleared
PASS (c') base Divider (control): after cancel, an unrelated 150px touch moved it 310.1px (leaked listeners)
PASS (d) mouse drag 200px works; hover rgba(212,196,168,.5) → accent/60; dragging rgb(180,83,30) + data-dragging + html cursor col-resize; cursor restored; grip display:none on fine
PASS (d) ArrowLeft 0.66 → 0.61, aria-valuenow=61
PASS (e) horizontal handle placement=start: finger 12px above the line dragged 100px → height 200 → 300; 12px below (header side) → no resize
```
Screenshots: `touch-1180x820-{light,dark}.png` (grip visible on both handles), `fine-1280x800-light-{hover,dragging}.png`, `fine-1280x800-dark.png`. The ~5 px px-vs-ratio skew in (a)/(d) is the harness page's own pane padding; the ratio math is exact.

## Gates (under build-lock)
`bun run check`: 0 errors (exit 0). `bunx vitest run src/lib/resize-drag.test.ts src/lib/panes.test.ts src/hooks`: 4 files, 20 tests passed (8 new in `resize-drag.test.ts`, `panes.test.ts` green).

## Not verified
Real iPad (Sky checks after `just release-ios`): WKWebView `touch-action` + pointer capture behaviour, the 32 px zone against a real fingertip, Stage Manager windows; iPad trackpad (fine pointer, hover) on the artifact sheet edge; VoiceOver reading of the separators.
