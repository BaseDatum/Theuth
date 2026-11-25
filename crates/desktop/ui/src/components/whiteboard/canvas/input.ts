// §8.4 input routing, as one pure table plus the palm window and the multi-finger tap recogniser.
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
}

export const palmActive = (s: Pick<RouteState, 'penDown' | 'palmUntil' | 'now'>) => s.penDown || s.now < s.palmUntil

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
      if (!s.readOnly && s.mode === 'draw') return 'ink'
      if (!s.readOnly && s.mode === 'erase') return 'erase'
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
