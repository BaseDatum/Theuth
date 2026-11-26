// Wheel and pinch over a card body. Over a card that is not entered they always belong to the board (pan, zoom): its
// body takes no pointer at all, so an inner pan/zoom surface (a diagram's `DiagramFrame`) never sees them. Over an
// entered card, an inner pan/zoom surface takes them (zoom about the pointer), and a scroll region that can still move
// in the gesture's direction scrolls; at its edge the gesture falls through to the board. Never both at once.

/** An inner pan/zoom surface (`DiagramFrame`'s `Surface`, role="application"). */
export const ZOOM_SURFACE = '[role="application"]'

/** One scroll container: its scroll offsets and sizes, and whether its overflow lets it scroll on each axis. */
export type Scroller = { top: number; left: number; sh: number; ch: number; sw: number; cw: number; x: boolean; y: boolean }

/** Can `s` scroll further by (dx, dy)? */
export function canScroll(s: Scroller, dx: number, dy: number): boolean {
  if (dy > 0 && s.y && s.top + s.ch < s.sh - 1) return true
  if (dy < 0 && s.y && s.top > 0) return true
  if (dx > 0 && s.x && s.left + s.cw < s.sw - 1) return true
  if (dx < 0 && s.x && s.left > 0) return true
  return false
}

/**
 * Which scroller (index into `chain`, innermost first) a wheel or drag by (dx, dy) scrolls, or -1 when the board takes
 * it: always over a card that is not entered, and when nothing under the pointer can scroll that way.
 */
export function scrollTarget(entered: boolean, chain: readonly Scroller[], dx: number, dy: number): number {
  if (!entered) return -1
  return chain.findIndex((s) => canScroll(s, dx, dy))
}

/**
 * Who takes a wheel (or a trackpad pinch: `pinch`, the wheel's ctrlKey) by (dx, dy) over a card body: the board, or
 * the content under the pointer (`inner`: the card then stops it before the board's zoom/pan). `zoomSurface`: the
 * pointer is over an inner pan/zoom surface; `chain`: the scroll containers under it, innermost first.
 */
export function wheelRoute(entered: boolean, w: { zoomSurface: boolean; pinch: boolean; chain: readonly Scroller[]; dx: number; dy: number }): 'board' | 'inner' {
  if (!entered) return 'board'
  if (w.zoomSurface) return 'inner'
  if (w.pinch) return 'board'
  return scrollTarget(true, w.chain, w.dx, w.dy) >= 0 ? 'inner' : 'board'
}

/** The scroll containers from `target` up to (not including) `root`, innermost first. */
export function scrollChain(target: Element | null, root: Element): Scroller[] {
  const out: Scroller[] = []
  for (let el = target; el && el !== root; el = el.parentElement) {
    if (!(el instanceof HTMLElement) || (el.scrollHeight <= el.clientHeight + 1 && el.scrollWidth <= el.clientWidth + 1)) continue
    const s = getComputedStyle(el)
    const x = /auto|scroll/.test(s.overflowX)
    const y = /auto|scroll/.test(s.overflowY)
    if (x || y) out.push({ top: el.scrollTop, left: el.scrollLeft, sh: el.scrollHeight, ch: el.clientHeight, sw: el.scrollWidth, cw: el.clientWidth, x, y })
  }
  return out
}

/** Does anything inside `body` hide content behind a scroll (an un-entered card shows a "scroll ↓" hint then)? */
export function hidesOverflow(body: Element): boolean {
  for (const el of body.querySelectorAll<HTMLElement>('*')) {
    if (el.scrollHeight <= el.clientHeight + 1 || el.clientHeight === 0) continue
    if (/auto|scroll/.test(getComputedStyle(el).overflowY)) return true
  }
  return false
}
