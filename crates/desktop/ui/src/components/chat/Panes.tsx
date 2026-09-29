import { useNavigate } from '@tanstack/react-router'
import type { PointerEvent, ReactNode, SyntheticEvent } from 'react'
import { useRef } from 'react'
import { touch } from '@/lib/hotkeys'
import { isDraft, type Node, newDraft, type Path, shown, usePanes, zone } from '@/lib/panes'
import { useSidebar } from '@/components/ui/sidebar'
import { cn } from '@/lib/utils'

/** Pointer travel (px) before a press on a sidebar row becomes a drag. */
const SLOP = 6

/**
 * Drag a sidebar row onto a chat pane. Pointer events, not HTML5 DnD: the macOS webview keeps the native
 * drag handler for file drops (Composer), which swallows `dragstart`. Spread the result on the row's `Link`;
 * a press that became a drag never navigates or selects. `id === null` is the New chat rail item: each drag
 * carries a fresh draft id, and a drop opens a draft pane (`NewChatPane`) without moving the URL.
 */
export function useSessionDrag(id: string | null, title: string) {
  const navigate = useNavigate()
  const press = useRef<{ x: number; y: number } | null>(null)
  const dragging = useRef(false)
  const swallow = useRef(false)
  const finish = (e: PointerEvent<HTMLElement>, drop: boolean) => {
    press.current = null
    if (!dragging.current) return
    dragging.current = false
    e.currentTarget.releasePointerCapture(e.pointerId)
    // The click that follows this pointerup belongs to the drag.
    swallow.current = true
    setTimeout(() => (swallow.current = false), 0)
    const dropped = usePanes.getState().endDrag(drop)
    if (dropped && !isDraft(dropped)) void navigate({ to: '/c/$session', params: { session: dropped } })
  }
  return {
    // An `<a>` starts a native drag of its href on press-and-move, which fires `pointercancel` on this sequence.
    draggable: false,
    onPointerDown(e: PointerEvent<HTMLElement>) {
      if (e.button !== 0 || touch() || e.metaKey || e.shiftKey) return
      press.current = { x: e.clientX, y: e.clientY }
    },
    onPointerMove(e: PointerEvent<HTMLElement>) {
      const p = press.current
      if (!p) return
      if (dragging.current) usePanes.getState().moveDrag(e.clientX, e.clientY)
      else if (Math.hypot(e.clientX - p.x, e.clientY - p.y) >= SLOP) {
        dragging.current = true
        e.currentTarget.setPointerCapture(e.pointerId)
        usePanes.getState().startDrag(id ?? newDraft(), title, e.clientX, e.clientY)
      }
    },
    onPointerUp(e: PointerEvent<HTMLElement>) {
      finish(e, true)
    },
    onPointerCancel(e: PointerEvent<HTMLElement>) {
      finish(e, false)
    },
    onClickCapture(e: SyntheticEvent) {
      if (!swallow.current) return
      e.preventDefault()
      e.stopPropagation()
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

/**
 * The chat area: the split tree when there is one, else the route's single chat (still a drop target). A phone
 * (`isMobile`) does not tile (25 §12.3): whatever the tree holds, the focused leaf alone fills the screen as `first`, so
 * its header always carries the sidebar toggle — the Sheet's only opener.
 */
export function Panes({ session, render }: { session: string; render(session: string, first: boolean): ReactNode }) {
  const root = usePanes((s) => s.root)
  const focused = usePanes((s) => s.focused)
  const { isMobile } = useSidebar()
  if (isMobile) return <Leaf session={shown(root, focused, session)} first render={render} />
  return <Branch node={root ?? { session }} path={[]} render={render} />
}
