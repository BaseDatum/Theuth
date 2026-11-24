/**
 * iOS keyboard (31 §8.1; the same fix as jarvis `lib/appHeight.ts`): WebKit
 * reveals the focused field by scrolling the page, and depending on the
 * container shrinks either only the visual viewport or the layout one too.
 * The shell must never scroll: `--visible-band` on `<html>` is the visual
 * viewport height while an editable has focus and the keyboard is up
 * (`data-keyboard-open`), else the layout height, and any page scroll WebKit
 * did is undone so the shrunk shell sits on the visible band. Nothing here
 * fires on a desktop: the visual viewport equals the layout one.
 */
import { touch } from '@/lib/platform'

/**
 * Smaller collapses are not a keyboard: browser chrome, rounding, and the iPad shortcuts bar a hardware
 * keyboard leaves behind — WebKit shrinks the visual viewport by it even though the bar floats (~69 px on an
 * iPad Pro, bugs.webkit.org/247410). The smallest real keyboard (iPhone, landscape) is well past this.
 */
const SLACK = 120

const editing = (): boolean => {
  const el = document.activeElement
  return el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement || (el instanceof HTMLElement && el.isContentEditable)
}

/**
 * The window's full height (112 §2): the tallest viewport reading taken while no editable had focus, at the
 * current width. Per window, not per screen: a Stage Manager window is always shorter than the screen and used
 * to read as keyboard-open whenever an editable had focus.
 */
let base = { width: 0, height: 0 }

/** Read the viewports and write the variables; exported for tests. */
export function applyViewport(): void {
  const layout = window.innerHeight
  const visible = Math.round(window.visualViewport?.height ?? layout)
  const tall = Math.max(layout, visible)
  const edit = editing()
  // A keyboard never widens the window or grows it: a new width (rotation, window resize) or a taller reading
  // re-bases even mid-edit. ponytail: a height-only window resize while editing still reads as a keyboard
  // until blur; `resize` carries no cause, so there is nothing cheaper to tell them apart.
  if (!edit || window.innerWidth !== base.width || tall > base.height) base = { width: window.innerWidth, height: tall }
  const full = touch() ? base.height : layout
  const open = edit && visible < full - SLACK
  const root = document.documentElement
  root.style.setProperty('--visible-band', `${open ? visible : layout}px`)
  root.toggleAttribute('data-keyboard-open', open)
  if (window.scrollY || window.scrollX) window.scrollTo(0, 0)
}

/**
 * WebKit can report the old `innerHeight` / `visualViewport` while a resize is still settling (an iPadOS Stage Manager
 * or Split View window finding its size, a resume, a rotation), and send no event once the value is final. One more
 * reading after the burst catches it.
 */
export const SETTLE_MS = 300

let remeasure = (): void => {}

/** Read the viewport again soon, with no window event: the router calls it on every route change (opening a chat). */
export const remeasureViewport = (): void => remeasure()

/** Start tracking; returns the disposer. Call once before the first render. */
export function trackViewport(): () => void {
  let frame = 0
  let settle: ReturnType<typeof setTimeout> | undefined
  const schedule = () => {
    if (!frame)
      frame = requestAnimationFrame(() => {
        frame = 0
        applyViewport()
      })
    clearTimeout(settle)
    settle = setTimeout(applyViewport, SETTLE_MS)
  }
  const targets: [EventTarget | null | undefined, string[]][] = [
    [window, ['resize', 'orientationchange', 'focusin', 'focusout', 'pageshow']],
    [window.visualViewport, ['resize', 'scroll']],
    [globalThis.document, ['visibilitychange']],
  ]
  for (const [t, names] of targets) for (const n of names) t?.addEventListener(n, schedule)
  remeasure = schedule
  applyViewport()
  return () => {
    cancelAnimationFrame(frame)
    clearTimeout(settle)
    remeasure = () => {}
    for (const [t, names] of targets) for (const n of names) t?.removeEventListener(n, schedule)
  }
}
