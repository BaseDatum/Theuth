// 133 STOP-SCOPE §4: Esc on a running turn. The first keydown stops the root at once (as ever). Keeping the key
// down (the browser repeats `keydown` with `repeat: true`) fills a ring on the stop button over `HOLD_MS` and then
// stops the whole tree, once; releasing earlier does nothing more. Three taps inside `TAP_WINDOW_MS` stop the
// tree as well — the TUI's only form, since terminals have no key-up. Pure: key facts and clock in, one verb out.

/** A held Esc stops the tree after this long. */
export const HOLD_MS = 700
/** Three taps inside this window stop the tree. */
export const TAP_WINDOW_MS = 1000
export const TAPS = 3

export type Stop = {
  /** Times of the recent non-repeat keydowns, oldest first; pruned to `TAP_WINDOW_MS`. */
  taps: number[]
  /** When the current press began; `null` between presses. */
  down: number | null
  /** This press already stopped the tree (by hold or by tap count): no second tree stop until the key is released. */
  fired: boolean
}

export type Act = { type: 'down'; t: number; repeat: boolean } | { type: 'up'; t: number } | { type: 'tick'; t: number }

/** `root` on the first keydown of a press; `tree` at most once per press. */
export type Verb = 'root' | 'tree' | null

export const IDLE: Stop = { taps: [], down: null, fired: false }

export function step(s: Stop, a: Act): [Stop, Verb] {
  switch (a.type) {
    case 'down': {
      if (a.repeat) return s.down === null ? [s, null] : tick(s, a.t)
      const taps = [...s.taps.filter((t) => a.t - t < TAP_WINDOW_MS), a.t]
      if (taps.length >= TAPS) return [{ taps: [], down: a.t, fired: true }, 'tree']
      return [{ taps, down: a.t, fired: false }, 'root']
    }
    case 'tick':
      return tick(s, a.t)
    case 'up':
      return [{ ...s, down: null, fired: false }, null]
  }
}

const tick = (s: Stop, t: number): [Stop, Verb] =>
  s.down !== null && !s.fired && t - s.down >= HOLD_MS ? [{ ...s, fired: true }, 'tree'] : [s, null]

/** How far the hold ring is, 0..1; 0 when nothing is held or the tree already stopped. */
export const progress = (s: Stop, t: number): number => (s.down === null || s.fired ? 0 : Math.min(1, (t - s.down) / HOLD_MS))
