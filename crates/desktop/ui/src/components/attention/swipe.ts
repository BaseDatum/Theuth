// Touch swipe on an inbox list card (207 follow-up): swipe right dismisses, swipe left snoozes for an hour
// (the smallest preset). Touch only — a mouse drag keeps selecting text, and vertical scrolling is untouched
// (`touch-action: pan-y` on the card).
import { type PointerEvent, useRef, useState } from 'react'
import { snoozeUntil } from '@/components/hud/model'
import type { AttentionItem } from '@/rpc/bindings/method.roster.output'
import { useAttentionBulk } from './queries'

/** Travel before the action fires on release. */
export const SWIPE_THRESHOLD = 88

export function useCardSwipe(item: AttentionItem, enabled: boolean) {
  const bulk = useAttentionBulk()
  const start = useRef<{ x: number; y: number; axis: 'x' | 'y' | null } | null>(null)
  const [dx, setDx] = useState(0)

  const down = (e: PointerEvent) => {
    if (!enabled || e.pointerType !== 'touch' || !e.isPrimary) return
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
    if (d > SWIPE_THRESHOLD) void bulk.dismiss([item.id])
    else if (d < -SWIPE_THRESHOLD) void bulk.snooze([item.id], snoozeUntil('hour'))
  }

  return { handlers: { onPointerDown: down, onPointerMove: move, onPointerUp: up, onPointerCancel: up }, dx }
}
