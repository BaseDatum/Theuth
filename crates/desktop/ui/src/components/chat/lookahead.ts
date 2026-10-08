// Design 135 Revision 1 §R2.1: the mounted window is measured in viewports, not rows, and biased toward the scroll
// direction — a fling at 4 000 px/s reaches a row two viewports ahead long after its content mounted; the viewport
// behind is the keep-alive, so a short back-and-forth never re-mounts anything.

/** Viewports mounted ahead of / behind the viewport in the scroll direction, and each side at rest. */
export const AHEAD = 2
export const BEHIND = 1
export const REST = 1.5

export type Direction = 'forward' | 'backward' | null

/** `[from, to]` px of the window around the viewport `[top, top + h]` scrolling `dir`, clamped to `[0, total]`. */
export function window(top: number, h: number, dir: Direction, total: number): [number, number] {
  const [up, down] = dir === 'forward' ? [BEHIND, AHEAD] : dir === 'backward' ? [AHEAD, BEHIND] : [REST, REST]
  return [Math.max(0, top - up * h), Math.min(total, top + h + down * h)]
}
