import { type KeyboardEvent, type PointerEvent, useEffect, useRef, useState } from 'react'
import { createResizeDrag, type DragPointer, keyDelta } from '@/lib/resize-drag'
import { cn } from '@/lib/utils'

export type ResizeHandleProps = {
  /** Of the visible line: a `vertical` line sits between left/right siblings and drags along x. */
  orientation: 'vertical' | 'horizontal'
  /**
   * Where the invisible hit zone (10 px, 32 px on coarse pointers) lies: centred on the line, or flush with the line's
   * far edge so it only extends toward the `start` (left/up) or the `end` (right/down) — away from buttons and scrollbars.
   */
  placement?: 'center' | 'start' | 'end'
  /** The line's look: thickness and idle colours (defaults `w-1`/`h-1`, `bg-border-subtle hover:bg-fg-accent/60`). */
  className?: string
  /** `aria-valuenow/min/max`, when the caller has a number for them. */
  value?: { now: number; min: number; max: number }
  'aria-label'?: string
  /** The press that starts a drag, with the handle's root element (measure the parent, record starting sizes). */
  onStart?(e: DragPointer, handle: HTMLDivElement): void
  onMove(clientX: number, clientY: number): void
  onEnd?(): void
  /** Arrow keys: `+1` = the line should move toward the end (right/down), `-1` toward the start. */
  onNudge?(delta: -1 | 1): void
}

/**
 * 138-DIVIDERS: the one resize handle behind every draggable divider (split panes, the artifact sheet's edge, the
 * code-map separator, a note's chat panel). It owns the gesture, the hit area, the visuals and the accessibility;
 * the caller owns the sizing math. Finger-safe: `touch-action: none` on the hit zone so WebKit never turns the drag
 * into a scroll, pointer capture plus `pointercancel`/`lostpointercapture` handling so a cancelled touch ends the
 * drag instead of leaving listeners that resize on the next finger, and a grip pill on coarse pointers where there is
 * no hover to hint at it.
 */
export function ResizeHandle({ orientation, placement = 'center', className, value, onStart, onMove, onEnd, onNudge, 'aria-label': label }: ResizeHandleProps) {
  const vertical = orientation === 'vertical'
  const [dragging, setDragging] = useState(false)
  const root = useRef<HTMLDivElement>(null)
  const cb = useRef({ onStart, onMove, onEnd })
  cb.current = { onStart, onMove, onEnd }
  /** DOM teardown for the live drag (listeners, capture, document cursor); the core calls it exactly once per drag. */
  const stop = useRef<(() => void) | null>(null)
  const drag = useRef(
    createResizeDrag<DragPointer>({
      onStart: (e) => cb.current.onStart?.(e, root.current!),
      onMove: (x, y) => cb.current.onMove(x, y),
      onEnd: () => {
        stop.current?.()
        stop.current = null
        setDragging(false)
        cb.current.onEnd?.()
      },
    }),
  )
  useEffect(() => () => drag.current.end(), [])

  const down = (e: PointerEvent<HTMLDivElement>) => {
    if (!drag.current.down(e)) return
    e.preventDefault()
    const el = e.currentTarget
    const id = e.pointerId
    const style = document.documentElement.style
    const prev = { cursor: style.cursor, userSelect: style.userSelect, webkitUserSelect: style.webkitUserSelect }
    style.cursor = vertical ? 'col-resize' : 'row-resize'
    style.userSelect = style.webkitUserSelect = 'none'
    const move = (ev: globalThis.PointerEvent) => drag.current.move(ev)
    const end = (ev: globalThis.PointerEvent) => drag.current.end(ev)
    el.addEventListener('pointermove', move)
    el.addEventListener('pointerup', end)
    el.addEventListener('pointercancel', end)
    el.addEventListener('lostpointercapture', end)
    stop.current = () => {
      el.removeEventListener('pointermove', move)
      el.removeEventListener('pointerup', end)
      el.removeEventListener('pointercancel', end)
      el.removeEventListener('lostpointercapture', end)
      if (el.hasPointerCapture(id)) el.releasePointerCapture(id)
      style.cursor = prev.cursor
      style.userSelect = prev.userSelect
      style.webkitUserSelect = prev.webkitUserSelect
    }
    el.setPointerCapture(id)
    setDragging(true)
  }

  const key = (e: KeyboardEvent<HTMLDivElement>) => {
    const d = keyDelta(e.key, orientation)
    if (d === 0 || !onNudge) return
    e.preventDefault()
    onNudge(d)
  }

  return (
    <div
      ref={root}
      role="separator"
      aria-orientation={orientation}
      aria-label={label}
      aria-valuenow={value?.now}
      aria-valuemin={value?.min}
      aria-valuemax={value?.max}
      tabIndex={0}
      data-dragging={dragging || undefined}
      onKeyDown={key}
      className={cn(
        'relative z-10 shrink-0 touch-none select-none outline-none transition-colors [-webkit-touch-callout:none] focus-visible:ring-2 focus-visible:ring-ring',
        vertical ? 'w-1 cursor-col-resize' : 'h-1 cursor-row-resize',
        'bg-border-subtle hover:bg-fg-accent/60',
        className,
        dragging && 'bg-fg-accent hover:bg-fg-accent',
      )}
    >
      <div
        onPointerDown={down}
        className={cn(
          'absolute touch-none select-none [-webkit-touch-callout:none]',
          vertical ? 'inset-y-0 w-2.5 pointer-coarse:w-8' : 'inset-x-0 h-2.5 pointer-coarse:h-8',
          vertical
            ? { center: 'left-1/2 -translate-x-1/2', start: 'right-0', end: 'left-0' }[placement]
            : { center: 'top-1/2 -translate-y-1/2', start: 'bottom-0', end: 'top-0' }[placement],
        )}
      />
      <div
        aria-hidden
        className={cn(
          'pointer-events-none absolute top-1/2 left-1/2 hidden -translate-x-1/2 -translate-y-1/2 rounded-full pointer-coarse:block',
          vertical ? 'h-8 w-1' : 'h-1 w-8',
          dragging ? 'bg-fg-accent' : 'bg-fg-tertiary',
        )}
      />
    </div>
  )
}
