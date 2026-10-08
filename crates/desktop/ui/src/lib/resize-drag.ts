/**
 * 138-DIVIDERS: the gesture core behind `components/ui/resize-handle.tsx`, DOM-free so it runs under vitest's node
 * environment. One drag per handle: it starts on a primary pointer only (and for a mouse, the main button), follows
 * that one pointer id, and ends on whichever of `pointerup` / `pointercancel` / `lostpointercapture` arrives first;
 * anything from another pointer, or after the end, is ignored — so a cancelled touch never leaves a live drag behind.
 */

export type DragPointer = { pointerId: number; isPrimary: boolean; button: number; pointerType: string; clientX: number; clientY: number }

export type DragCallbacks<E extends DragPointer> = {
  onStart?(e: E): void
  onMove(clientX: number, clientY: number): void
  onEnd?(): void
}

export type ResizeDrag<E extends DragPointer> = {
  /** `pointerdown`: true when a drag began (the caller then captures the pointer and attaches the move/end listeners). */
  down(e: E): boolean
  /** `pointermove`: forwards only the live pointer's position. */
  move(e: E): void
  /** `pointerup` / `pointercancel` / `lostpointercapture` (or unmount, with no event): ends the live drag, once. */
  end(e?: Pick<DragPointer, 'pointerId'>): void
  readonly live: boolean
}

export function createResizeDrag<E extends DragPointer>(cb: DragCallbacks<E>): ResizeDrag<E> {
  let id: number | null = null
  return {
    get live() {
      return id !== null
    },
    down(e) {
      if (id !== null || !e.isPrimary || (e.pointerType === 'mouse' && e.button !== 0)) return false
      id = e.pointerId
      cb.onStart?.(e)
      return true
    },
    move(e) {
      if (id === null || e.pointerId !== id) return
      cb.onMove(e.clientX, e.clientY)
    },
    end(e) {
      if (id === null || (e && e.pointerId !== id)) return
      id = null
      cb.onEnd?.()
    },
  }
}

export const clamp = (v: number, min: number, max: number) => Math.min(max, Math.max(min, v))

/** Split-pane ratios (`Panes`): the first child's share, kept between 15 % and 85 %. */
export const RATIO_MIN = 0.15
export const RATIO_MAX = 0.85
export const RATIO_STEP = 0.05
/** Arrow-key step for width/height handles. */
export const PX_STEP = 16

export const clampRatio = (r: number) => clamp(r, RATIO_MIN, RATIO_MAX)

/**
 * Arrow keys on a separator: `+1` moves the line toward the end (right for a vertical line, down for a horizontal
 * one), `-1` toward the start, `0` for any other key. Callers translate the direction into their own size change.
 */
export function keyDelta(key: string, orientation: 'vertical' | 'horizontal'): -1 | 0 | 1 {
  if (orientation === 'vertical') return key === 'ArrowLeft' ? -1 : key === 'ArrowRight' ? 1 : 0
  return key === 'ArrowUp' ? -1 : key === 'ArrowDown' ? 1 : 0
}
