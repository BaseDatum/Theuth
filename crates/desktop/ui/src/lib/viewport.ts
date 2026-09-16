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

/** Smaller collapses are browser chrome / rounding, not a keyboard. */
const SLACK = 50

const editing = (): boolean => {
  const el = document.activeElement
  return el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement || (el instanceof HTMLElement && el.isContentEditable)
}

/**
 * The screen's block edge: the one full-height reading that survives both
 * keyboard modes (`innerHeight` itself shrinks in one of them).
 */
const screenBlock = (): number => {
  const { width: w, height: h } = window.screen
  return matchMedia('(orientation: landscape)').matches ? Math.min(w, h) : Math.max(w, h)
}

/** Read the viewports and write the variables; exported for tests. */
export function applyViewport(): void {
  const layout = window.innerHeight
  const visible = Math.round(window.visualViewport?.height ?? layout)
  // ponytail: a touch window shorter than the screen (iPad Stage Manager) reads as keyboard-open while editing; only `--safe-bottom` is affected.
  const full = touch() ? Math.max(layout, screenBlock()) : layout
  const open = editing() && visible < full - SLACK
  const root = document.documentElement
  root.style.setProperty('--visible-band', `${open ? visible : layout}px`)
  root.toggleAttribute('data-keyboard-open', open)
  if (window.scrollY || window.scrollX) window.scrollTo(0, 0)
}

/** Start tracking; returns the disposer. Call once before the first render. */
export function trackViewport(): () => void {
  let frame = 0
  const schedule = () => {
    if (frame) return
    frame = requestAnimationFrame(() => {
      frame = 0
      applyViewport()
    })
  }
  const targets: [EventTarget | null, string[]][] = [
    [window, ['resize', 'orientationchange', 'focusin', 'focusout', 'pageshow']],
    [window.visualViewport, ['resize', 'scroll']],
  ]
  for (const [t, names] of targets) for (const n of names) t?.addEventListener(n, schedule)
  applyViewport()
  return () => {
    cancelAnimationFrame(frame)
    for (const [t, names] of targets) for (const n of names) t?.removeEventListener(n, schedule)
  }
}
