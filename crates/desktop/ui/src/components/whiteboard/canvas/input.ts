// §8.4 input routing, as one pure table plus the palm window and the multi-finger tap recogniser.
import type { InkTool } from './model'
import type { Mode } from './store'

export const PALM_MS = 500
/** A multi-finger tap: every finger down within this window, all up within TAP_MS of the first, moved < TAP_SLOP. */
export const TAP_MS = 300
export const TAP_SLOP = 12

export type PointerKind = 'pen' | 'touch' | 'mouse'
/**
 * What a pointer-down does:
 * - `ink` / `erase`: B3's ink layer takes it (capture, no pan/select).
 * - `swallow`: stopped before React Flow and nothing happens (native ink draws it, or a read-only pen).
 * - `pass`: React Flow handles it (pan, select, drag, handles, link taps).
 * - `ignore`: stopped and dropped (palm window).
 */
export type Route = 'ink' | 'erase' | 'swallow' | 'pass' | 'ignore'

export type RouteState = {
  mode: Mode
  fingerInk: boolean
  nativeInk: boolean
  readOnly: boolean
  penDown: boolean
  /** ms until which touches are ignored (last pen-up + 500 ms) */
  palmUntil: number
  now: number
  /** A mouse pan gesture: Space held or a non-primary button. The mouse cannot drag-pan while it inks. */
  pan?: boolean
}

/**
 * The cluster's ink tool pick: choosing a pen arms `draw` (so a mouse or, with fingerInk, a finger inks; a Pencil inks
 * in every mode anyway). It latches: picking the armed pen again keeps it (a re-fired pick never switches it off);
 * another tool, select or Esc leaves it.
 */
export function pickInkTool(picked: InkTool): { tool: InkTool; mode: Mode } {
  return { tool: picked, mode: 'draw' }
}

export const palmActive = (s: Pick<RouteState, 'penDown' | 'palmUntil' | 'now'>) => s.penDown || s.now < s.palmUntil

/**
 * The samples of one stroke in time order, each once. They can arrive again or late: iPadOS WebKit fires `pointermove`
 * twice per Pencil sample (again when the touch's estimated force and tilt arrive), each with its `getCoalescedEvents()`,
 * which can repeat samples already taken; a page relay can resend. Taken as they come, the stroke steps back in time and
 * place, and the renderer (perfect-freehand) draws each step back as a sharp corner — a flat end and a round cap
 * mid-line. So a sample older than the last one taken, or one already taken at the same time and place, is dropped.
 */
export class StrokeSamples {
  #t = -Infinity
  #at = new Set<string>()

  /** True when the sample at (x, y), time `t` (ms, the event clock) is new. */
  take(x: number, y: number, t: number): boolean {
    if (t < this.#t) return false
    const k = `${x},${y}`
    if (t > this.#t) {
      this.#t = t
      this.#at.clear()
    } else if (this.#at.has(k)) return false
    this.#at.add(k)
    return true
  }
}

export function routePointer(kind: PointerKind, s: RouteState): Route {
  switch (kind) {
    case 'pen':
      if (s.readOnly) return 'swallow'
      if (s.mode === 'link') return 'pass' // the Pencil is a pointer in link mode only
      if (s.mode === 'erase') return 'erase'
      if (s.nativeInk) return 'swallow' // the plugin delivers the stroke
      return 'ink'
    case 'touch':
      if (palmActive(s)) return 'ignore'
      if (!s.readOnly && s.fingerInk && s.mode === 'draw') return 'ink'
      if (!s.readOnly && s.fingerInk && s.mode === 'erase') return 'erase'
      return 'pass'
    case 'mouse':
      // Desktop has no Pencil: the mouse is the pen whenever an ink tool is chosen (R2 only gates fingers).
      if (s.pan || s.readOnly) return 'pass'
      if (s.mode === 'draw') return 'ink'
      if (s.mode === 'erase') return 'erase'
      return 'pass'
  }
}

export const pointerKind = (t: string): PointerKind => (t === 'pen' ? 'pen' : t === 'touch' ? 'touch' : 'mouse')

/**
 * Two-finger tap = undo, three-finger tap = redo. Fed with touch pointer events; `up` returns the finger count of a
 * completed multi-finger tap (2 or 3), else 0.
 */
export class MultiTap {
  #down = new Map<number, { x: number; y: number; t: number }>()
  #max = 0
  #start = 0
  #moved = false

  down(id: number, x: number, y: number, t: number) {
    if (this.#down.size === 0) {
      this.#max = 0
      this.#start = t
      this.#moved = false
    }
    this.#down.set(id, { x, y, t })
    this.#max = Math.max(this.#max, this.#down.size)
  }

  move(id: number, x: number, y: number) {
    const d = this.#down.get(id)
    if (d && Math.hypot(x - d.x, y - d.y) > TAP_SLOP) this.#moved = true
  }

  up(id: number, t: number): number {
    if (!this.#down.delete(id)) return 0
    if (this.#down.size > 0) return 0
    const n = this.#max
    return !this.#moved && t - this.#start <= TAP_MS && (n === 2 || n === 3) ? n : 0
  }

  cancel() {
    this.#down.clear()
    this.#moved = true
  }
}
