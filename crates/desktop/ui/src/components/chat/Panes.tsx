import { useNavigate } from '@tanstack/react-router'
import type { PointerEvent, ReactNode, SyntheticEvent } from 'react'
import { useEffect, useRef } from 'react'
import { useSidebar } from '@/components/ui/sidebar'
import { isDraft, type Node, newDraft, type Path, usePanes, zone } from '@/lib/panes'
import { type Act, type Drag, HOLD_MS, IDLE, step } from '@/lib/session-drag'
import { cn } from '@/lib/utils'

/**
 * Drag a sidebar row onto a chat pane. Pointer events, not HTML5 DnD: the macOS webview keeps the native
 * drag handler for file drops (Composer), which swallows `dragstart`. Spread the result on the row's `Link`;
 * a press that became a drag never navigates, selects or opens the row's context menu. `id === null` is the
 * New chat rail item: each drag carries a fresh draft id, and a drop opens a draft pane (`NewChatPane`)
 * without moving the URL. The gesture itself is `lib/session-drag` (mouse: move; finger: hold, then move).
 */
export function useSessionDrag(id: string | null, title: string) {
  const navigate = useNavigate()
  const { isMobile } = useSidebar()
  const state = useRef<Drag>(IDLE)
  const timer = useRef<number | undefined>(undefined)
  const press = useRef<{ el: HTMLElement; pointerId: number } | null>(null)
  const swallow = useRef(false)
  // While a finger drags, the touch sequence must not become a scroll (iOS would fire `pointercancel`), so
  // `touchmove` is cancelled from a non-passive listener; the row's `data-drag-row` CSS mutes callout/selection.
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
        usePanes.getState().startDrag(id ?? newDraft(), title, x, y)
        break
      }
      case 'move':
        usePanes.getState().moveDrag(x, y)
        break
      case 'drop':
      case 'cancel': {
        // The click that follows this pointerup belongs to the drag (a finger's click may come a little later).
        swallow.current = true
        setTimeout(() => (swallow.current = false), 500)
        const dropped = usePanes.getState().endDrag(verb === 'drop')
        if (dropped && !isDraft(dropped)) void navigate({ to: '/c/$session', params: { session: dropped } })
      }
    }
  }
  const eat = (e: SyntheticEvent) => {
    e.preventDefault()
    e.stopPropagation()
  }
  return {
    // An `<a>` starts a native drag of its href on press-and-move, which fires `pointercancel` on this sequence.
    draggable: false,
    'data-drag-row': true,
    onPointerDown(e: PointerEvent<HTMLElement>) {
      press.current = { el: e.currentTarget, pointerId: e.pointerId }
      dispatch(
        { type: 'down', x: e.clientX, y: e.clientY, pointerType: e.pointerType, button: e.button, modifier: e.metaKey || e.shiftKey, sheet: isMobile },
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
  }
}

/** The drag ghost and the snapped drop zone; mounted once in the shell, renders nothing between drags. */
export function DragOverlay() {
  const drag = usePanes((s) => s.drag)
  if (!drag) return null
  return (
    <>
      {drag.target && (
        <div
          className="pointer-events-none fixed z-50 rounded-xl border-2 border-fg-accent bg-fg-accent/10 transition-[left,top,width,height] duration-100"
          style={zone(drag.target)}
        />
      )}
      <div
        className="pointer-events-none fixed z-50 max-w-56 truncate rounded-md border bg-popover px-2 py-1 text-xs shadow-md"
        style={{ left: drag.x + 12, top: drag.y + 12 }}
      >
        {drag.title}
      </div>
    </>
  )
}

/** One leaf: the drop target for the pane it shows; a press inside focuses it (and moves the URL there; a draft has no URL). */
function Leaf({ session, first, render }: { session: string; first: boolean; render(session: string, first: boolean): ReactNode }) {
  const navigate = useNavigate()
  const focused = usePanes((s) => s.focused)
  const split = usePanes((s) => s.root !== null)
  return (
    <div
      data-pane={session}
      className={cn('relative flex min-h-0 min-w-0 flex-1 flex-col', split && focused !== session && 'opacity-80')}
      onPointerDownCapture={() => {
        if (focused === session) return
        usePanes.getState().focus(session)
        if (!isDraft(session)) void navigate({ to: '/c/$session', params: { session } })
      }}
    >
      {render(session, first)}
    </div>
  )
}

/** Between two flex children sized `flex: r 1 0%` / `flex: 1-r 1 0%`; drag reports the new first-child share (0.15–0.85) to `onRatio`. */
export function Divider({ dir, onRatio }: { dir: 'row' | 'col'; onRatio: (ratio: number) => void }) {
  // ponytail: plain pointer events like ArtifactPanel's strip; react-resizable-panels if this grows.
  const drag = (e: PointerEvent<HTMLDivElement>) => {
    const rect = e.currentTarget.parentElement?.getBoundingClientRect()
    if (!rect) return
    e.preventDefault()
    e.currentTarget.setPointerCapture(e.pointerId)
    const move = (ev: globalThis.PointerEvent) => {
      const r = dir === 'row' ? (ev.clientX - rect.left) / rect.width : (ev.clientY - rect.top) / rect.height
      onRatio(Math.min(0.85, Math.max(0.15, r)))
    }
    const up = () => {
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', up)
    }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', up)
  }
  return (
    <div
      role="separator"
      aria-orientation={dir === 'row' ? 'vertical' : 'horizontal'}
      onPointerDown={drag}
      className={cn(
        'z-10 shrink-0 bg-border-subtle transition-colors hover:bg-fg-accent/60',
        dir === 'row' ? 'w-1 cursor-col-resize' : 'h-1 cursor-row-resize',
      )}
    />
  )
}

function Branch({ node, path, render }: { node: Node; path: Path; render(session: string, first: boolean): ReactNode }) {
  if ('session' in node) return <Leaf session={node.session} first={path.every((p) => p === 'a')} render={render} />
  return (
    <div className={cn('flex min-h-0 min-w-0 flex-1', node.dir === 'row' ? 'flex-row' : 'flex-col')}>
      <div className="flex min-h-0 min-w-0 flex-col" style={{ flex: `${node.ratio} 1 0%` }}>
        <Branch node={node.a} path={[...path, 'a']} render={render} />
      </div>
      <Divider dir={node.dir} onRatio={(r) => usePanes.getState().resize(path, r)} />
      <div className="flex min-h-0 min-w-0 flex-col" style={{ flex: `${1 - node.ratio} 1 0%` }}>
        <Branch node={node.b} path={[...path, 'b']} render={render} />
      </div>
    </div>
  )
}

/** The chat area: the split tree when there is one, else the route's single chat (still a drop target). */
export function Panes({ session, render }: { session: string; render(session: string, first: boolean): ReactNode }) {
  const root = usePanes((s) => s.root)
  return <Branch node={root ?? { session }} path={[]} render={render} />
}
