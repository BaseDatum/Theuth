// 214 D8: the task graph's React Flow canvas — with nodes.tsx the only importer of `@xyflow/react` under tasks/, loaded
// lazily by TaskGraph.tsx so the library stays its own chunk. Layout is ELK (`useElkLayout`) keyed by the model's
// structural hash, so a state change redraws a card in place; a new task re-lays out and fades in. The viewport fits
// on the first layout and whenever the node count grows, and is the user's otherwise.
import '@xyflow/react/dist/style.css'
import { Background, BackgroundVariant, ControlButton, Controls, MiniMap, type Node, ReactFlow, ReactFlowProvider, useReactFlow } from '@xyflow/react'
import { Scan } from 'lucide-react'
import { useEffect, useMemo, useRef, useSyncExternalStore } from 'react'
import { fixedLayout, flattenElk, type Layout, useElkLayout } from '@/lib/graph-layout'
import type { Task } from '@/rpc/bindings/method.roster.output'
import { type GraphOpts, PANEL_W, STATE_TONE, TASK_W, taskGraph, toElk } from './model'
import { PillNode, type PillNodeType, TaskEdge, TaskEdgeMarkers, type TaskEdgeType, TaskNode, type TaskNodeType } from './nodes'

export type CanvasProps = {
  tasks: readonly Task[]
  opts: GraphOpts
  selected: string | null
  onSelect(id: string | null): void
  /** The minimap gives way while the side panel covers the right edge. */
  panelOpen: boolean
}

const NODE_TYPES = { task: TaskNode, pill: PillNode }
const EDGE_TYPES = { task: TaskEdge }
/** Room around the drawing when it fits; the open side panel's width is added on the right. */
const PAD = 32
const MOVE_MS = 300

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

function Flow(p: CanvasProps) {
  const rf = useReactFlow()
  const reduced = useSyncExternalStore(subscribeReduced, () => window.matchMedia(REDUCED).matches)
  const dark = useSyncExternalStore(subscribeTheme, () => document.documentElement.classList.contains('dark'))
  const model = useMemo(() => taskGraph(p.tasks, p.opts), [p.tasks, p.opts])
  const shown = useElkLayout<Layout>(model.hash, () => toElk(model), flattenElk)
  // Past the 2 s time-box the deterministic grid stands in, so the graph is never blank.
  const layout = useMemo(() => (shown === null ? null : 'layout' in shown ? shown.layout : fixedLayout(toElk(model))), [shown, model])
  // While a new hash is laid out the previous layout stays: known cards keep their place, new ones wait for theirs.
  const pos = useMemo(() => new Map((layout?.nodes ?? []).map((n) => [n.id, n])), [layout])
  // ELK's own edge routes (around the cards) only while the drawn layout is this model's; a pending one smooth-steps.
  const routes = useMemo(() => {
    if (!layout || !shown || shown.hash !== model.hash || !('layout' in shown)) return null
    return new Map(layout.edges.map((e) => [e.index, e.sections[0] ? [e.sections[0].start, ...e.sections[0].bends, e.sections[0].end] : null]))
  }, [layout, shown, model.hash])

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
  const drawnHash = shown?.hash ?? ''
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
  const covered = () => (panelOpen.current ? Math.min(PANEL_W, box.current?.clientWidth ?? 0) : 0)
  const fit = (duration: number) =>
    void rf.fitView({ maxZoom: 1, minZoom: 0.15, duration, padding: { top: `${PAD}px`, bottom: `${PAD}px`, left: `${PAD}px`, right: `${covered() + PAD}px` } })
  const fitRef = useRef(fit)
  fitRef.current = fit

  // Fit on the first drawing and whenever a card appears that was not drawn before (a new task, done ones shown
  // again); a card leaving keeps the viewport, which is otherwise the user's.
  const drawn = useRef<ReadonlySet<string>>(new Set())
  const ids = useMemo(() => nodes.map((n) => n.id).join('\n'), [nodes])
  useEffect(() => {
    const before = drawn.current
    const now = new Set(ids ? ids.split('\n') : [])
    if (![...now].some((id) => !before.has(id))) {
      drawn.current = now
      return
    }
    // Recorded when the fit runs, so an effect replayed before the frame (StrictMode) still fits.
    const frame = requestAnimationFrame(() => {
      drawn.current = now
      fitRef.current(before.size === 0 || reduced ? 0 : MOVE_MS)
    })
    return () => cancelAnimationFrame(frame)
  }, [ids, reduced])

  // A selected card the side panel would cover slides left into view (pan only, the zoom stays).
  useEffect(() => {
    const el = box.current
    const n = p.selected ? rf.getNode(p.selected) : undefined
    if (!el || !n) return
    const { x, y, zoom } = rf.getViewport()
    const left = n.position.x * zoom + x
    const right = left + TASK_W * zoom
    const visible = el.clientWidth - covered() - 24
    let dx = right > visible ? visible - right : 0
    if (left + dx < 24) dx = 24 - left
    if (dx !== 0) void rf.setViewport({ x: x + dx, y, zoom }, { duration: reduced ? 0 : MOVE_MS })
    // Only a new selection pans.
  }, [p.selected, rf])

  return (
    <div
      ref={box}
      className="tg-canvas size-full"
      onKeyDown={(e) => {
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
          if (n.type === 'task') p.onSelect(n.id)
        }}
        onPaneClick={() => p.onSelect(null)}
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
            style={{ width: 168, height: 96 }}
          />
        )}
      </ReactFlow>
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
