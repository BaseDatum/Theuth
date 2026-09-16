/**
 * iOS keyboard (31 §8.1): WebKit never shrinks the layout viewport for the
 * keyboard, only the visual one, so a `100dvh` shell keeps its full height and
 * WebKit scrolls the page to show the focused field, pushing the header off
 * screen. Publish the visible part instead: `--visible-band` on `<html>` ends
 * at the visual viewport's document-relative bottom while an editable has
 * focus, and `data-keyboard-open` marks that state. Nothing here fires on a
 * desktop: the visual viewport equals the layout one.
 */

/** Smaller collapses are browser chrome / rounding, not a keyboard. */
const SLACK = 50

const editing = (): boolean => {
  const el = document.activeElement
  return el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement || (el instanceof HTMLElement && el.isContentEditable)
}

/** Read the viewports and write the variables; exported for tests. */
export function applyViewport(): void {
  const layout = window.innerHeight
  const vv = window.visualViewport
  const open = editing() && !!vv && vv.height < layout - SLACK
  // `pageTop` includes WebKit's automatic document scroll; `offsetTop` does not.
  const band = open ? Math.min(layout, Math.round(vv.height + vv.pageTop)) : layout
  const root = document.documentElement
  root.style.setProperty('--visible-band', `${band}px`)
  root.toggleAttribute('data-keyboard-open', open)
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
