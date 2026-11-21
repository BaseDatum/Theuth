import { useNavigate } from '@tanstack/react-router'
import { type ReactNode, useRef } from 'react'
import { ResizeHandle } from '@/components/ui/resize-handle'
import { useSidebar } from '@/components/ui/sidebar'
import { usePressDrag } from '@/hooks/use-press-drag'
import { isDraft, type Node, newDraft, type Path, shown, usePanes, zone } from '@/lib/panes'
import { isWeb } from '@/lib/web-views'
import { clampRatio, RATIO_MAX, RATIO_MIN, RATIO_STEP } from '@/lib/resize-drag'
import { cn } from '@/lib/utils'

/**
 * Drag a sidebar row onto a chat pane (`usePressDrag` owns the gesture and its DOM glue). Spread the result on the
 * row's `Link`; a press that became a drag never navigates, selects or opens the row's context menu. `id === null`
 * is the New chat rail item: each drag carries a fresh draft id, and a drop opens a draft pane (`NewChatPane`)
 * without moving the URL. In the phone Sheet fingers never drag: phones do not tile.
 */
export function useSessionDrag(id: string | null, title: string) {
  const navigate = useNavigate()
  const { isMobile } = useSidebar()
  return usePressDrag({
    disabledForFinger: isMobile,
    onStart: (x, y) => usePanes.getState().startDrag(id ?? newDraft(), title, x, y),
    onMove: (x, y) => usePanes.getState().moveDrag(x, y),
    onEnd(drop) {
      const dropped = usePanes.getState().endDrag(drop)
      if (dropped && !isDraft(dropped)) void navigate({ to: '/c/$session', params: { session: dropped } })
    },
  }).props
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

/** One leaf: the drop target for the pane it shows; a press inside focuses it (and moves the URL there; a draft has no
 * URL). A web frame (205 D5) is never focused or dimmed: the URL stays on the chat beside it. */
function Leaf({ session, first, render }: { session: string; first: boolean; render(session: string, first: boolean): ReactNode }) {
  const navigate = useNavigate()
  const focused = usePanes((s) => s.focused)
  const split = usePanes((s) => s.root !== null)
  return (
    <div
      data-pane={session}
      className={cn('relative flex min-h-0 min-w-0 flex-1 flex-col', split && focused !== session && !isWeb(session) && 'opacity-80')}
      onPointerDownCapture={() => {
        if (focused === session || isWeb(session)) return
        usePanes.getState().focus(session)
        if (!isDraft(session)) void navigate({ to: '/c/$session', params: { session } })
      }}
    >
      {render(session, first)}
    </div>
  )
}

/**
 * Between two flex children sized `flex: r 1 0%` / `flex: 1-r 1 0%`; a drag reports the new first-child share
 * (0.15–0.85) to `onRatio`. With `ratio` the separator also exposes its value and nudges it by 5 % per arrow key.
 */
export function Divider({ dir, ratio, onRatio }: { dir: 'row' | 'col'; ratio?: number; onRatio: (ratio: number) => void }) {
  const rect = useRef<DOMRect>(null)
  return (
    <ResizeHandle
      orientation={dir === 'row' ? 'vertical' : 'horizontal'}
      value={ratio === undefined ? undefined : { now: Math.round(ratio * 100), min: RATIO_MIN * 100, max: RATIO_MAX * 100 }}
      onStart={(_, el) => (rect.current = el.parentElement?.getBoundingClientRect() ?? null)}
      onMove={(x, y) => {
        const r = rect.current
        if (r) onRatio(clampRatio(dir === 'row' ? (x - r.left) / r.width : (y - r.top) / r.height))
      }}
      onNudge={ratio === undefined ? undefined : (d) => onRatio(clampRatio(ratio + d * RATIO_STEP))}
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
      <Divider dir={node.dir} ratio={node.ratio} onRatio={(r) => usePanes.getState().resize(path, r)} />
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
