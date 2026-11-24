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
import { type GraphOpts, STATE_TONE, taskGraph, toElk } from './model'
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
const FIT = { padding: 0.12, maxZoom: 1, minZoom: 0.15 } as const
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
  const edges = useMemo<TaskEdgeType[]>(
    () =>
      model.edges.flatMap((e): TaskEdgeType[] => {
        if (!pos.has(e.source) || !pos.has(e.target)) return []
        const touching = p.selected !== null && (e.source === p.selected || e.target === p.selected)
        return [
          {
            id: e.id,
            type: 'task',
            source: e.source,
            target: e.target,
            sourceHandle: 'r',
            targetHandle: 'l',
            selectable: false,
            focusable: false,
            zIndex: touching ? 1 : 0,
            data: { kind: e.kind, open: e.open, dim: p.selected !== null && !touching },
          },
        ]
      }),
    [model, pos, p.selected],
  )

  // Fit on the first drawing and whenever more nodes are drawn than before; otherwise the viewport is the user's.
  const drawn = useRef(0)
  const count = nodes.length
  useEffect(() => {
    const before = drawn.current
    drawn.current = count
    if (count === 0 || count <= before) return
    const frame = requestAnimationFrame(() => void rf.fitView({ ...FIT, duration: before === 0 || reduced ? 0 : MOVE_MS }))
    return () => cancelAnimationFrame(frame)
  }, [count, rf, reduced])

  return (
    <div
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
          <ControlButton onClick={() => void rf.fitView({ ...FIT, duration: reduced ? 0 : MOVE_MS })} aria-label="Fit everything" title="Fit everything">
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
