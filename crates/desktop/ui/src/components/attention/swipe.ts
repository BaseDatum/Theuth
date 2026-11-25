// Touch swipe on an inbox list card (207 follow-up): swipe right dismisses, swipe left snoozes for an hour
// (the smallest preset). Touch only — a mouse drag keeps selecting text, and vertical scrolling is untouched
// (`touch-action: pan-y` on the card).
import { type PointerEvent, useRef, useState } from 'react'
import { snoozeUntil } from '@/components/hud/model'
import type { AttentionItem } from '@/rpc/bindings/method.roster.output'
import { useAttentionBulk } from './queries'

/** Movement before a gesture commits to an axis. Horizontal intent wins a 3:2 tie, so a diagonal swipe does not turn into a scroll. */
export function swipeAxis(dx: number, dy: number): 'x' | 'y' | null {
  const x = Math.abs(dx)
  const y = Math.abs(dy)
  if (x <= 12 && y <= 12) return null
  return x * 1.5 >= y ? 'x' : 'y'
}

/** A deliberate action must travel the whole card, not a fixed fraction of it. */
export function isFullSwipe(distance: number, width: number): boolean {
  return width > 0 && Math.abs(distance) >= width
}

export function useCardSwipe(item: AttentionItem, enabled: boolean) {
  const bulk = useAttentionBulk()
  const start = useRef<{ x: number; y: number; width: number; axis: 'x' | 'y' | null } | null>(null)
  const [dx, setDx] = useState(0)

  const down = (e: PointerEvent) => {
    if (!enabled || e.pointerType !== 'touch' || !e.isPrimary) return
    start.current = { x: e.clientX, y: e.clientY, width: e.currentTarget.clientWidth, axis: null }
  }
  const move = (e: PointerEvent) => {
    const s = start.current
    if (!s) return
    const ddx = e.clientX - s.x
    const ddy = e.clientY - s.y
    if (s.axis === null) s.axis = swipeAxis(ddx, ddy)
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
    if (!isFullSwipe(d, s.width)) return
    if (d > 0) void bulk.dismiss([item.id])
    else void bulk.snooze([item.id], snoozeUntil('hour'))
  }

  const cancel = () => {
    start.current = null
    setDx(0)
  }

  return { handlers: { onPointerDown: down, onPointerMove: move, onPointerUp: up, onPointerCancel: cancel }, dx }
}
