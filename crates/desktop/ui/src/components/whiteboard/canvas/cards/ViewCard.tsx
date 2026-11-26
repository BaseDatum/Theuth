// The `view` card body: an app route (`/tasks/graph?project=…`, `/tasks`, `/agents`) live inside the card, in its own
// memory router (as the `view` tile does) under `InsideTileProvider`, so the root renders only the routed page. It
// reads the app's shared query cache, so it stays live as data changes. Inner navigation stays in the card (Back in
// the header) except chats (to the workspace), whiteboards (to a pane) and, on the canvas, a task's detail: that opens
// as its own `task` card beside this one (the task's live card), linked from it, so it sits next to the graph.

import { type QueryClient, useQueryClient } from '@tanstack/react-query'
import { RouterProvider, createMemoryHistory, createRouter, useNavigate } from '@tanstack/react-router'
import { ReactFlowProvider, useReactFlow } from '@xyflow/react'
import { ChevronLeft, ExternalLink, LayoutDashboard } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { openViewFromCard } from '@/components/tiles/open'
import { InsideTileProvider } from '@/components/tiles/tile-context'
import { usePanes } from '@/lib/panes'
import { routeTree } from '@/routeTree.gen'
import { openTaskBeside } from '../../task-cards'
import { leavesCard, viewCardPath, viewCardTitle } from '../../view-card'
import { useCanvas } from '../context'
import type { Card } from '../model'

export type ViewCardProps = {
  path: string
  /** Pointer, wheel and keys go to the view (the card is entered, or shown in the sheet); else the canvas keeps them. */
  interactive: boolean
  /** The card on the canvas (absent in the sheet): a task opened in it becomes a card beside it. */
  card?: Card
}

/** A task's title from whatever task lists or task the app has cached (the graph has them loaded). */
function cachedTaskTitle(qc: QueryClient, id: string): string | undefined {
  for (const [, data] of qc.getQueriesData<unknown>({ queryKey: ['tasks'] })) {
    for (const t of Array.isArray(data) ? data : [data]) {
      if (t && typeof t === 'object' && 'id' in t && t.id === id && 'title' in t && typeof t.title === 'string' && t.title.trim()) return t.title
    }
  }
  return undefined
}

/** A new path (a new version, the scrubber) is a new router. */
export default function ViewCard(p: ViewCardProps) {
  const path = viewCardPath(p.path)
  if (!path)
    return (
      <div className="wb-chip">
        <LayoutDashboard className="size-6 opacity-70" aria-hidden />
        <span className="max-w-full truncate text-xs font-medium">Not an app view</span>
        <span className="max-w-full truncate text-[11px] text-fg-tertiary">{p.path}</span>
      </div>
    )
  return p.card ? <OnCanvas key={path} path={path} interactive={p.interactive} card={p.card} /> : <Live key={path} path={path} interactive={p.interactive} />
}

/** On the canvas: a task opened in the view goes to its card beside this one (found, or placed and linked), in view. */
function OnCanvas({ card, ...p }: ViewCardProps & { card: Card }) {
  const ctx = useCanvas()
  const rf = useReactFlow()
  const qc = useQueryClient()
  const live = useRef({ ctx, card })
  live.current = { ctx, card }
  const onTask = (task: string): boolean => {
    const { ctx: c, card: from } = live.current
    if (c.readOnly) return false
    const r = openTaskBeside(c.doc, c.board, from.id, task, cachedTaskTitle(qc, task) ?? `Task ${task}`)
    c.stopCapturing()
    c.store.getState().set({ selection: [r.id] })
    // The graph and the task together.
    const x = Math.min(from.x, r.box[0])
    const y = Math.min(from.y, r.box[1])
    const width = Math.max(from.x + from.w, r.box[0] + r.box[2]) - x
    const height = Math.max(from.y + from.h, r.box[1] + r.box[3]) - y
    void rf.fitBounds({ x, y, width, height }, { padding: 0.15, duration: matchMedia('(prefers-reduced-motion: reduce)').matches ? 0 : 450 })
    return true
  }
  return <Live {...p} onTask={onTask} />
}

function Live({ path, interactive, onTask }: ViewCardProps & { onTask?(task: string): boolean }) {
  const outerNavigate = useNavigate()
  const qc = useQueryClient()
  const outer = useRef(outerNavigate)
  outer.current = outerNavigate
  const task = useRef(onTask)
  task.current = onTask
  const [current, setCurrent] = useState(path)
  const [canBack, setCanBack] = useState(false)
  const here = leavesCard(current)
  const title = here?.kind === 'task' ? (cachedTaskTitle(qc, here.id) ?? 'Task') : viewCardTitle(current)

  const leave = useRef((href: string, from: string): boolean => {
    const to = leavesCard(href)
    if (!to) return false
    if (to.kind === 'task') {
      // A task card's own detail (its search, a refresh) stays; any other task opens beside the card.
      const at = leavesCard(from)
      if (at?.kind === 'task' && at.id === to.id) return false
      return task.current?.(to.id) ?? false
    }
    if (to.kind === 'pane') {
      openViewFromCard({ path: to.href }, { navigate: (h) => void outer.current({ href: h }) })
      return true
    }
    if (to.kind === 'chat') usePanes.getState().show(to.session)
    void outer.current({ href: to.href })
    return true
  })

  const [router] = useState(() => {
    const history = createMemoryHistory({ initialEntries: [path] })
    const push = history.push.bind(history)
    const replace = history.replace.bind(history)
    const from = () => history.location.href
    history.push = (to, state, opts) => {
      if (!leave.current(to, from())) push(to, state, opts)
    }
    history.replace = (to, state, opts) => {
      if (!leave.current(to, from())) replace(to, state, opts)
    }
    return createRouter({ routeTree, history, defaultPreload: false })
  })
  useEffect(
    () =>
      router.subscribe('onResolved', ({ toLocation }) => {
        setCurrent(toLocation.href)
        setCanBack(router.history.canGoBack())
      }),
    [router],
  )

  return (
    <div
      className={`absolute inset-0 flex flex-col overflow-hidden bg-background ${interactive ? 'nodrag nopan nowheel' : ''}`}
      data-view-card={current}
      // The whiteboard's shortcuts (window keydown) must not fire while typing or pressing keys in the view.
      onKeyDown={interactive ? (e) => e.stopPropagation() : undefined}
    >
      <div className="flex h-8 shrink-0 items-center gap-2 border-b border-border-subtle px-2.5">
        {canBack ? (
          <button type="button" aria-label="Back" className="nodrag nopan -ml-1 grid size-6 shrink-0 place-items-center rounded-md text-fg-secondary hover:bg-layer-2" onClick={() => router.history.back()}>
            <ChevronLeft className="size-4" />
          </button>
        ) : (
          <LayoutDashboard className="size-3.5 shrink-0 text-fg-tertiary" aria-hidden />
        )}
        <span className="min-w-0 flex-1 truncate text-xs font-medium" title={current}>
          {title}
        </span>
        <button
          type="button"
          className="nodrag nopan flex shrink-0 items-center gap-1 rounded-md px-1.5 py-0.5 text-[11px] text-fg-secondary hover:bg-layer-2"
          onClick={() => openViewFromCard({ path: current, title }, { navigate: (h) => void outer.current({ href: h }) })}
        >
          <ExternalLink className="size-3.5" aria-hidden /> Open in pane
        </button>
      </div>
      <div className="relative flex min-h-0 flex-1 flex-col">
        {/* A route that draws with React Flow (the task graph) gets its own store, not the whiteboard canvas's. */}
        <ReactFlowProvider>
          <InsideTileProvider>
            <RouterProvider router={router} />
          </InsideTileProvider>
        </ReactFlowProvider>
      </div>
    </div>
  )
}
