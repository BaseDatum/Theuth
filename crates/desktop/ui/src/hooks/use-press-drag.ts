import type { PointerEvent, SyntheticEvent } from 'react'
import { useEffect, useRef } from 'react'
import { type Act, type Drag, HOLD_MS, IDLE, step } from '@/lib/session-drag'

/** What a press-drag source does with the gesture; the DOM glue below is shared by sidebar rows and board cards. */
export type PressDrag = {
  /** The press became a drag (mouse: SLOP travelled; finger: held still HOLD_MS) at the pointer's current spot. */
  onStart(x: number, y: number): void
  onMove(x: number, y: number): void
  /** `drop`: the pointer lifted after moving. Otherwise cancelled: `pointercancel`, an unmoved finger lifting, or `cancel()`. */
  onEnd(drop: boolean): void
  /** Fingers never start a drag here (the phone Sheet: phones do not tile). */
  disabledForFinger?: boolean
}

/**
 * Pointer-event drag, not HTML5 DnD: the WebKit webview keeps the native drag handler for file drops (Composer),
 * which swallows `dragstart`. The gesture is `lib/session-drag` (mouse/pen/trackpad: move; finger: hold, then move);
 * this hook owns the timer, pointer capture, and the iOS guards: a non-passive `touchmove` blocker while a finger drags
 * (a started scroll would fire `pointercancel`), the long-press `contextmenu` eaten while the gesture is live, the click
 * that follows a drag swallowed, and `data-drag-row` whose CSS (styles.css) mutes selection and the link callout.
 * Spread `props` on the source element; `cancel()` ends a live gesture from outside (Escape).
 */
export function usePressDrag(cb: PressDrag) {
  const cbRef = useRef(cb)
  cbRef.current = cb
  const state = useRef<Drag>(IDLE)
  const timer = useRef<number | undefined>(undefined)
  const press = useRef<{ el: HTMLElement; pointerId: number } | null>(null)
  const swallow = useRef(false)
  const block = useRef((e: TouchEvent) => {
    if (state.current.phase === 'dragging') e.preventDefault()
  }).current
  useEffect(
    () => () => {
      clearTimeout(timer.current)
      document.removeEventListener('touchmove', block)
    },
    [block],
  )

  const dispatch = (a: Act, x: number, y: number) => {
    const prev = state.current
    const [next, verb] = step(prev, a)
    state.current = next
    if (next.phase !== 'pressed') clearTimeout(timer.current)
    if (next.phase === 'idle') document.removeEventListener('touchmove', block)
    switch (verb) {
      case 'hold':
        document.addEventListener('touchmove', block, { passive: false })
        timer.current = window.setTimeout(() => dispatch({ type: 'timer' }, x, y), HOLD_MS)
        break
      case 'start': {
        const p = press.current
        p?.el.setPointerCapture(p.pointerId)
        if (prev.phase === 'pressed' && prev.touch) navigator.vibrate?.(15)
        // From here until 500 ms after the end, a click on the source belongs to the drag (a finger's click may come
        // a little late; after an Escape the mouse's pointerup, and its click, come whenever the button lifts).
        swallow.current = true
        cbRef.current.onStart(x, y)
        break
      }
      case 'move':
        cbRef.current.onMove(x, y)
        break
      case 'drop':
      case 'cancel': {
        const p = press.current
        // Implicit after pointerup; explicit after `cancel()` while the pointer is still down.
        if (p?.el.hasPointerCapture(p.pointerId)) p.el.releasePointerCapture(p.pointerId)
        setTimeout(() => (swallow.current = false), 500)
        cbRef.current.onEnd(verb === 'drop')
      }
    }
  }
  const eat = (e: SyntheticEvent) => {
    e.preventDefault()
    e.stopPropagation()
  }
  return {
    cancel() {
      dispatch({ type: 'cancel' }, 0, 0)
    },
    props: {
      // An `<a>` starts a native drag of its href on press-and-move, which ends this pointer sequence without
      // `pointerup` while the source keeps capture (UI-FIXES-3): forbid it twice.
      draggable: false,
      onDragStart: eat,
      'data-drag-row': true,
      onPointerDown(e: PointerEvent<HTMLElement>) {
        press.current = { el: e.currentTarget, pointerId: e.pointerId }
        dispatch(
          {
            type: 'down',
            x: e.clientX,
            y: e.clientY,
            pointerType: e.pointerType,
            button: e.button,
            modifier: e.metaKey || e.shiftKey,
            sheet: cbRef.current.disabledForFinger === true,
          },
          e.clientX,
          e.clientY,
        )
      },
      onPointerMove(e: PointerEvent<HTMLElement>) {
        dispatch({ type: 'move', x: e.clientX, y: e.clientY }, e.clientX, e.clientY)
      },
      onPointerUp(e: PointerEvent<HTMLElement>) {
        dispatch({ type: 'up' }, e.clientX, e.clientY)
      },
      onPointerCancel(e: PointerEvent<HTMLElement>) {
        dispatch({ type: 'cancel' }, e.clientX, e.clientY)
      },
      onClickCapture(e: SyntheticEvent) {
        if (!swallow.current) return
        swallow.current = false
        eat(e)
      },
      // iOS WebKit may fire `contextmenu` from the long press that started the drag: the `⋯` is the touch path to the menu.
      onContextMenuCapture(e: SyntheticEvent) {
        if (state.current.phase !== 'idle') eat(e)
      },
    },
  }
}

/** What `usePressDrag` spreads on its source element. */
export type PressDragProps = ReturnType<typeof usePressDrag>['props']

/**
 * Edge auto-scroll while a drag is live: how far (px, per frame) a scroller spanning `left..right` moves for a pointer
 * at `x`. Zero away from the edges; ramps to `speed` as the pointer reaches an edge, so a target column that is
 * off-screen (the phone's horizontal board) slides under the finger.
 */
export function edgeScroll(x: number, left: number, right: number, edge = 40, speed = 8): number {
  if (x < left + edge) return -speed * Math.min(1, (left + edge - x) / edge)
  if (x > right - edge) return speed * Math.min(1, (x - right + edge) / edge)
  return 0
}
