// 214 D8: the task graph's React Flow canvas — with nodes.tsx the only importer of `@xyflow/react` under tasks/, loaded
// lazily by TaskGraph.tsx so the library stays its own chunk. Layout is ELK keyed by the model's structural hash, so a
// state change redraws a card in place; a new task re-lays out and fades in. A layout that fails or times out is drawn
// on the grid (`gridLayout`) instead — the graph is never blank — and a timed-out one is asked for again, since its late
// result lands in the cache. The viewport fits on the first drawing and whenever a card appears, and is the user's
// otherwise.
import '@xyflow/react/dist/style.css'
import { Background, BackgroundVariant, ControlButton, Controls, MiniMap, type Node, ReactFlow, ReactFlowProvider, useReactFlow } from '@xyflow/react'
import { Scan } from 'lucide-react'
import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react'
import { elkLayout, elkPeek, flattenElk, type Layout, LayoutTimeout } from '@/lib/diagram/elk'
import type { Task } from '@/rpc/bindings/method.roster.output'
import { AccentSwatches } from '../HierarchyFields'
import { useTaskUpdate } from '../queries'
import { fitPadding, type GraphModel, type GraphOpts, gridLayout, STATE_TONE, TASK_W, taskGraph, toElk } from './model'
import { EdgeLegend, PillNode, type PillNodeType, TaskEdge, TaskEdgeMarkers, type TaskEdgeType, TaskNode, type TaskNodeType } from './nodes'

export type CanvasProps = {
  tasks: readonly Task[]
  opts: GraphOpts
  /** Only this task and its descendants (the "under" filter); `null` = every task. */
  under: string | null
  selected: string | null
  onSelect(id: string | null): void
  /** The minimap gives way while the side panel covers the right edge. */
  panelOpen: boolean
  /** Floating bottom-right composer dock is mounted: move MiniMap to bottom-left so it never overlaps `.wb-dock`. */
  composerOpen?: boolean
}

const NODE_TYPES = { task: TaskNode, pill: PillNode }
const EDGE_TYPES = { task: TaskEdge }
const MOVE_MS = 300
/** A timed-out layout is asked for again this often before the grid stays. */
const RETRIES = 5

const REDUCED = '(prefers-reduced-motion: reduce)'
const subscribeReduced = (cb: () => void) => {
  const m = window.matchMedia(REDUCED)
  m.addEventListener('change', cb)
  return () => m.removeEventListener('change', cb)
}
const subscribeTheme = (cb: () => void) => {
  const o = new MutationObserver(cb)
  o.observe(document.documentElement, { attributes: true, attributeFilter: ['class'] })
  return () => o.disconnect()
}

function miniColor(n: Node): string {
  if (n.type !== 'task') return 'var(--color-danger)'
  return STATE_TONE[(n as TaskNodeType).data.model.task.state]
}

/** What is drawn: the layout of `hash`; `grid` when it is the stand-in (no ELK routes then). */
type Drawn = { hash: string; layout: Layout; grid: boolean }

/**
 * The layout of `model`: from ELK's cache at once when known, else laid out in the worker while the previous drawing
 * stays. Any failure draws the grid and logs a warning; a time-out also retries, so the real layout replaces the grid.
 */
function useTaskLayout(model: GraphModel): Drawn | null {
  const [drawn, setDrawn] = useState<Drawn | null>(() => {
    const layout = elkPeek<Layout>(model.hash)
    return layout ? { hash: model.hash, layout, grid: false } : null
  })
  const latest = useRef(model)
  latest.current = model
  const { hash } = model
  useEffect(() => {
    let live = true
    const m = latest.current
    const run = async (attempt: number) => {
      try {
        const layout = await elkLayout(m.hash, () => toElk(m), flattenElk)
        if (live) setDrawn({ hash: m.hash, layout, grid: false })
      } catch (e) {
        if (!live) return
        const late = e instanceof LayoutTimeout
        if (!late || attempt === 0) console.warn(`task graph: ${late ? 'layout timed out' : 'layout failed'}; drawing the grid`, e)
        setDrawn((d) => (d?.hash === m.hash && !d.grid ? d : { hash: m.hash, layout: gridLayout(m), grid: true }))
        if (late && attempt < RETRIES) void run(attempt + 1)
      }
    }
    void run(0)
    return () => {
      live = false
    }
  }, [hash])
  return drawn
}

function Flow(p: CanvasProps) {
  const rf = useReactFlow()
  const reduced = useSyncExternalStore(subscribeReduced, () => window.matchMedia(REDUCED).matches)
  const dark = useSyncExternalStore(subscribeTheme, () => document.documentElement.classList.contains('dark'))
  const model = useMemo(() => taskGraph(p.tasks, p.opts, p.under), [p.tasks, p.opts, p.under])
  const drawn = useTaskLayout(model)
  // While a new hash is laid out the previous layout stays: known cards keep their place, new ones wait for theirs.
  const pos = useMemo(() => new Map((drawn?.layout.nodes ?? []).map((n) => [n.id, n])), [drawn])
  // ELK's own edge routes (around the cards) only while the drawn layout is this model's; otherwise smooth-step.
  const routes = useMemo(() => {
    if (!drawn || drawn.grid || drawn.hash !== model.hash) return null
    return new Map(drawn.layout.edges.map((e) => [e.index, e.sections[0] ? [e.sections[0].start, ...e.sections[0].bends, e.sections[0].end] : null]))
  }, [drawn, model.hash])
  const [menu, setMenu] = useState<{ id: string; x: number; y: number } | null>(null)

  const nodes = useMemo<(TaskNodeType | PillNodeType)[]>(
    () =>
      model.nodes.flatMap((n): (TaskNodeType | PillNodeType)[] => {
        const at = pos.get(n.id)
        if (!at) return []
        const position = { x: at.x, y: at.y }
        return n.kind === 'task'
          ? [{ id: n.id, type: 'task', position, data: { model: n, selected: n.id === p.selected }, draggable: false, selectable: true, ariaLabel: `${n.task.title}, ${n.task.state}` }]
          : [{ id: n.id, type: 'pill', position, data: { model: n }, draggable: false, selectable: false, focusable: false }]
      }),
    [model, pos, p.selected],
  )
  // An edge is remounted per layout (its id carries the layout's hash), so after a relayout it fades in on its new
  // route while the cards glide there (`.tg-canvas` styles) instead of jumping.
  const drawnHash = drawn?.hash ?? ''
  const edges = useMemo<TaskEdgeType[]>(
    () =>
      model.edges.flatMap((e, i): TaskEdgeType[] => {
        if (!pos.has(e.source) || !pos.has(e.target)) return []
        const touching = p.selected !== null && (e.source === p.selected || e.target === p.selected)
        return [
          {
            id: `${e.id}@${drawnHash}`,
            className: 'tg-edge',
            type: 'task',
            source: e.source,
            target: e.target,
            sourceHandle: 'r',
            targetHandle: 'l',
            selectable: false,
            focusable: false,
            zIndex: touching ? 1 : 0,
            data: { kind: e.kind, open: e.open, dim: p.selected !== null && !touching, route: routes?.get(i) ?? null },
          },
        ]
      }),
    [model, pos, routes, p.selected, drawnHash],
  )

  // The whole drawing in the part of the canvas the side panel leaves visible.
  const box = useRef<HTMLDivElement>(null)
  const panelOpen = useRef(p.panelOpen)
  panelOpen.current = p.panelOpen
  const padding = () => fitPadding(box.current?.clientWidth ?? 0, panelOpen.current)
  const fit = (duration: number) => {
    const el = box.current
    // A canvas with no size yet (a hidden tile) cannot be fitted: the transform would park the drawing off-screen.
    if (!el || el.clientWidth === 0 || el.clientHeight === 0) return false
    const pad = padding()
    void rf.fitView({ maxZoom: 1, minZoom: 0.1, duration, padding: { top: `${pad.top}px`, bottom: `${pad.bottom}px`, left: `${pad.left}px`, right: `${pad.right}px` } })
    return true
  }
  const fitRef = useRef(fit)
  fitRef.current = fit

  // Fit on the first drawing and whenever a card appears that was not drawn before (a new task, done ones shown
  // again); a card leaving keeps the viewport, which is otherwise the user's.
  const drawnIds = useRef<ReadonlySet<string>>(new Set())
  const ids = useMemo(() => nodes.map((n) => n.id).join('\n'), [nodes])
  useEffect(() => {
    const before = drawnIds.current
    const now = new Set(ids ? ids.split('\n') : [])
    if (![...now].some((id) => !before.has(id))) {
      drawnIds.current = now
      return
    }
    // Recorded when the fit runs, so an effect replayed before the frame (StrictMode) still fits; a canvas without a
    // size records nothing, and the resize below fits once it has one.
    const frame = requestAnimationFrame(() => {
      if (fitRef.current(before.size === 0 || reduced ? 0 : MOVE_MS)) drawnIds.current = now
    })
    return () => cancelAnimationFrame(frame)
  }, [ids, reduced])
  useEffect(() => {
    const el = box.current
    if (!el) return
    const o = new ResizeObserver(() => {
      if (drawnIds.current.size === 0 && ids && fitRef.current(0)) drawnIds.current = new Set(ids.split('\n'))
    })
    o.observe(el)
    return () => o.disconnect()
  }, [ids])

  // A selected card the side panel would cover slides left into view (pan only, the zoom stays).
  useEffect(() => {
    const el = box.current
    const n = p.selected ? rf.getNode(p.selected) : undefined
    if (!el || !n) return
    const { x, y, zoom } = rf.getViewport()
    const left = n.position.x * zoom + x
    const right = left + TASK_W * zoom
    const visible = el.clientWidth - padding().right + 8
    let dx = right > visible ? visible - right : 0
    if (left + dx < 24) dx = 24 - left
    if (dx !== 0) void rf.setViewport({ x: x + dx, y, zoom }, { duration: reduced ? 0 : MOVE_MS })
    // Only a new selection pans.
  }, [p.selected, rf])

  return (
    <div
      ref={box}
      className="tg-canvas relative size-full"
      onKeyDown={(e) => {
        if (e.key === 'Escape' && menu) {
          e.stopPropagation()
          setMenu(null)
          return
        }
        if (e.key !== 'Enter' && e.key !== ' ') return
        const id = (e.target as HTMLElement).closest?.('.react-flow__node-task')?.getAttribute('data-id')
        if (!id) return
        e.preventDefault()
        p.onSelect(id)
      }}
    >
      <ReactFlow
        nodes={nodes}
        edges={edges}
        nodeTypes={NODE_TYPES}
        edgeTypes={EDGE_TYPES}
        nodesDraggable={false}
        nodesConnectable={false}
        elementsSelectable
        onlyRenderVisibleElements={nodes.length > 150}
        minZoom={0.1}
        maxZoom={2}
        forceColorMode={dark ? 'dark' : 'light'}
        onNodeClick={(_, n) => {
          setMenu(null)
          if (n.type === 'task') p.onSelect(n.id)
        }}
        onNodeContextMenu={(e, n) => {
          if (n.type !== 'task') return
          e.preventDefault()
          const r = box.current?.getBoundingClientRect()
          setMenu({ id: n.id, x: e.clientX - (r?.left ?? 0), y: e.clientY - (r?.top ?? 0) })
        }}
        onPaneClick={() => {
          setMenu(null)
          p.onSelect(null)
        }}
        onMoveStart={() => setMenu(null)}
      >
        <TaskEdgeMarkers />
        <Background variant={BackgroundVariant.Dots} gap={24} size={1.25} color="color-mix(in oklab, var(--color-fg-tertiary) 38%, transparent)" />
        <Controls showInteractive={false} showFitView={false} position="bottom-left">
          <ControlButton onClick={() => fit(reduced ? 0 : MOVE_MS)} aria-label="Fit everything" title="Fit everything">
            <Scan />
          </ControlButton>
        </Controls>
        {!p.panelOpen && nodes.length > 8 && (
          <MiniMap
            position={p.composerOpen ? 'bottom-left' : 'bottom-right'}
            pannable
            zoomable
            nodeColor={miniColor}
            nodeStrokeWidth={0}
            nodeBorderRadius={10}
            bgColor="var(--bg-layer-1)"
            maskColor="color-mix(in oklab, var(--bg-page) 62%, transparent)"
            maskStrokeColor="var(--color-fg-accent)"
            maskStrokeWidth={2}
            ariaLabel="Task graph overview"
            style={p.composerOpen ? { width: 168, height: 96, marginLeft: 56 } : { width: 168, height: 96 }}
          />
        )}
      </ReactFlow>
      <EdgeLegend hierarchy={p.opts.parentEdges} />
      {menu && <AccentMenu key={menu.id} {...menu} task={p.tasks.find((t) => t.id === menu.id)} onClose={() => setMenu(null)} />}
    </div>
  )
}

/** Right-click on a card: its accent colour, saved at once. */
function AccentMenu({ id, x, y, task, onClose }: { id: string; x: number; y: number; task: Task | undefined; onClose(): void }) {
  const patch = useTaskUpdate(id)
  return (
    <div
      role="menu"
      aria-label="Accent colour"
      className="absolute z-20 rounded-lg border border-border-subtle bg-background p-2 shadow-md"
      style={{ left: x, top: y }}
      onContextMenu={(e) => e.preventDefault()}
    >
      <p className="mb-1.5 text-xs text-fg-secondary">Accent</p>
      <AccentSwatches
        value={task?.accent ?? null}
        onChange={(accent) => {
          patch.mutate({ accent: accent ?? 'none' })
          onClose()
        }}
      />
    </div>
  )
}

/** The lazily loaded canvas (default export for `React.lazy`). */
export default function Canvas(props: CanvasProps) {
  return (
    <ReactFlowProvider>
      <Flow {...props} />
    </ReactFlowProvider>
  )
}
