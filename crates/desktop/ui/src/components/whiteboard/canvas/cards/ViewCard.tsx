// The `view` card body: an app route (`/tasks/graph?project=…`, `/tasks`, `/agents`) live inside the card, in its own
// memory router (as the `view` tile does) under `InsideTileProvider`, so the root renders only the routed page. It
// reads the app's shared query cache, so it stays live as data changes. Inner navigation stays in the card except
// chats (to the workspace) and whiteboards (to a pane); the header's Open in pane opens where the card is now.

import { RouterProvider, createMemoryHistory, createRouter, useNavigate } from '@tanstack/react-router'
import { ReactFlowProvider } from '@xyflow/react'
import { ExternalLink, LayoutDashboard } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { openViewFromCard } from '@/components/tiles/open'
import { InsideTileProvider } from '@/components/tiles/tile-context'
import { usePanes } from '@/lib/panes'
import { routeTree } from '@/routeTree.gen'
import { leavesCard, viewCardPath, viewCardTitle } from '../../view-card'

export type ViewCardProps = {
  path: string
  /** Pointer, wheel and keys go to the view (the card is entered, or shown in the sheet); else the canvas keeps them. */
  interactive: boolean
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
  return <Live key={path} path={path} interactive={p.interactive} />
}

function Live({ path, interactive }: ViewCardProps) {
  const outerNavigate = useNavigate()
  const outer = useRef(outerNavigate)
  outer.current = outerNavigate
  const [current, setCurrent] = useState(path)
  const title = viewCardTitle(current)

  const leave = useRef((href: string): boolean => {
    const to = leavesCard(href)
    if (!to) return false
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
    history.push = (to, state, opts) => {
      if (!leave.current(to)) push(to, state, opts)
    }
    history.replace = (to, state, opts) => {
      if (!leave.current(to)) replace(to, state, opts)
    }
    return createRouter({ routeTree, history, defaultPreload: false })
  })
  useEffect(() => router.subscribe('onResolved', ({ toLocation }) => setCurrent(toLocation.href)), [router])

  return (
    <div
      className={`absolute inset-0 flex flex-col overflow-hidden bg-background ${interactive ? 'nodrag nopan nowheel' : ''}`}
      data-view-card={current}
      // The whiteboard's shortcuts (window keydown) must not fire while typing or pressing keys in the view.
      onKeyDown={interactive ? (e) => e.stopPropagation() : undefined}
    >
      <div className="flex h-8 shrink-0 items-center gap-2 border-b border-border-subtle px-2.5">
        <LayoutDashboard className="size-3.5 shrink-0 text-fg-tertiary" aria-hidden />
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
