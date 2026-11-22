// Touch swipe on a list row: the row follows the finger horizontally and an action fires past a threshold on
// release. Touch only; vertical scrolling always wins (`touch-action: pan-y` belongs on the row). Used by the
// sidebar's session rows (archive / delete); the Inbox cards have their own hook with their bulk actions.
import { type PointerEvent, useRef, useState } from 'react'

export function useRowSwipe({ onSwipeLeft, onSwipeRight, threshold = 88 }: { onSwipeLeft?: () => void; onSwipeRight?: () => void; threshold?: number }) {
  const start = useRef<{ x: number; y: number; axis: 'x' | 'y' | null } | null>(null)
  const [dx, setDx] = useState(0)

  const down = (e: PointerEvent) => {
    if (e.pointerType !== 'touch' || !e.isPrimary) return
    start.current = { x: e.clientX, y: e.clientY, axis: null }
  }
  const move = (e: PointerEvent) => {
    const s = start.current
    if (!s) return
    const ddx = e.clientX - s.x
    const ddy = e.clientY - s.y
    if (s.axis === null && (Math.abs(ddx) > 12 || Math.abs(ddy) > 12)) s.axis = Math.abs(ddx) > Math.abs(ddy) ? 'x' : 'y'
    if (s.axis === 'y') {
      start.current = null
      setDx(0)
      return
    }
    if (s.axis === 'x') setDx(ddx)
  }
  const up = () => {
    const s = start.current
    if (!s) return
    start.current = null
    const d = dx
    setDx(0)
    if (s.axis !== 'x') return
    if (d > threshold) onSwipeRight?.()
    else if (d < -threshold) onSwipeLeft?.()
  }

  return {
    handlers: { onPointerDown: down, onPointerMove: move, onPointerUp: up, onPointerCancel: up },
    dx,
    /** Translate + snap-back for the row; keeps the page's vertical scroll. */
    style: {
      transform: dx !== 0 ? `translateX(${dx}px)` : undefined,
      transition: dx === 0 ? 'transform 150ms ease-out' : undefined,
      touchAction: 'pan-y' as const,
    },
  }
}
