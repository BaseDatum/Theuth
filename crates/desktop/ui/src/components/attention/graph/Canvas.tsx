// 208 §8.6: the React Flow canvas — the only importer of `@xyflow/react` and its CSS besides edges.tsx, loaded lazily by
// Region.tsx so the library is its own chunk. Thin: positions, folds, dimming and edge handles come from elements.ts.
import '@xyflow/react/dist/style.css'
import {
  Background,
  BackgroundVariant,
  Controls,
  type Edge,
  Handle,
  MiniMap,
  type Node,
  type NodeProps,
  Position,
  ReactFlow,
  ReactFlowProvider,
  useReactFlow,
  type Viewport,
} from '@xyflow/react'
import { createContext, memo, useContext, useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react'
import type { GraphPulse } from '@/rpc/bindings/event.agent_2egraph.output'
import { canvasElements, lodOf } from './elements'
import { MessageEdge, type Pulse, TreeEdge } from './edges'
import { BOX, layoutCache } from './layout'
import { pathToRoot } from './selectors'
import { type Lod, NodeBody } from './NodeBody'
import type { GraphState } from './reducer'
import type { NodeView, VisibleGraph } from './ttl'

export type CanvasProps = {
  state: GraphState
  view: VisibleGraph
  now: number
  highlight: string | null
  selected: string | null
  /** "Show in graph": centre this session (no zoom change) when the nonce changes. */
  focus: { session: string; nonce: number } | null
  follow: boolean
  /** Fit: re-pack the roots and fit the view when this changes. */
  fitNonce: number
  onNodeClick: (id: string) => void
  onEdgeClick: (id: string, at: { x: number; y: number }) => void
  onPaneClick: () => void
}

type SessionData = { view: NodeView; highlighted: boolean; selected: boolean; now: number }
type FoldData = { count: number; parent: string }
type SessionNodeType = Node<SessionData, 'session'>
type FoldNodeType = Node<FoldData, 'fold'>

const LodContext = createContext<Lod>('full')
const HANDLE = { opacity: 0, width: 1, height: 1, minWidth: 0, minHeight: 0, border: 0, pointerEvents: 'none' } as const
/** How long after mount the first `fitView` may still move the viewport (a "Show in graph" centring waits for it). */
const FIT_SETTLE_MS = 350
/** Room kept above the top row by every fit, so no node sits under the region's top edge. */
const TOP_ROOM = 48
const FIT_PADDING = { top: `${TOP_ROOM}px`, bottom: '24px', x: '24px' } as const
/** Opacity of nodes off the highlighted path: dimmed, labels still readable in both themes. */
const DIM = 0.5

function Handles() {
  return (
    <>
      <Handle type="target" position={Position.Top} id="t" style={HANDLE} isConnectable={false} />
      <Handle type="source" position={Position.Bottom} id="b" style={HANDLE} isConnectable={false} />
      <Handle type="target" position={Position.Left} id="l" style={HANDLE} isConnectable={false} />
      <Handle type="source" position={Position.Right} id="r" style={HANDLE} isConnectable={false} />
    </>
  )
}

const SessionNode = memo(function SessionNode({ data }: NodeProps<SessionNodeType>) {
  const lod = useContext(LodContext)
  const v = data.view
  return (
    <>
      <Handles />
      <NodeBody node={v.node} lod={lod} attention={v.attention} highlighted={data.highlighted} selected={data.selected} opacity={v.opacity} now={data.now} />
    </>
  )
})

const FoldNode = memo(function FoldNode({ data }: NodeProps<FoldNodeType>) {
  return (
    <>
      <Handles />
      <div className="flex h-16 w-[200px] items-center justify-center rounded-lg border border-dashed border-border-strong bg-layer-1 text-sm text-fg-secondary" data-node-id={`fold:${data.parent}`}>
        +{data.count} finished
      </div>
    </>
  )
})

const NODE_TYPES = { session: SessionNode, fold: FoldNode }
const EDGE_TYPES = { tree: TreeEdge, message: MessageEdge }

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

/** Minimap fill: danger for attention, else the node's state tone. */
function miniColor(n: Node): string {
  if (n.type !== 'session') return 'var(--color-border-strong)'
  const v = (n.data as SessionData).view
  if (v.attention > 0) return 'var(--color-danger)'
  const s = v.node.activity.state.state
  if (s === 'running') return 'var(--color-project-malachite)'
  if (s === 'approval' || s === 'awaiting') return 'var(--color-warning)'
  if (s === 'error') return 'var(--color-danger)'
  return 'var(--color-fg-tertiary)'
}

function Flow(p: CanvasProps) {
  const rf = useReactFlow()
  const reduced = useSyncExternalStore(subscribeReduced, () => window.matchMedia(REDUCED).matches)
  const dark = useSyncExternalStore(subscribeTheme, () => document.documentElement.classList.contains('dark'))
  const [lod, setLod] = useState<Lod>('full')
  const [openFolds, setOpenFolds] = useState<ReadonlySet<string>>(() => new Set())
  const cache = useRef(layoutCache())
  const slots = useRef<ReadonlyMap<string, number> | null>(null)
  const fitSeen = useRef(p.fitNonce)
  const repack = fitSeen.current !== p.fitNonce
  const els = useMemo(
    () => canvasElements(p.state, p.view, p.highlight, openFolds, repack ? null : slots.current, cache.current),
    [p.state, p.view, p.highlight, openFolds, repack],
  )
  slots.current = els.slots
  useEffect(() => {
    if (fitSeen.current === p.fitNonce) return
    fitSeen.current = p.fitNonce
    requestAnimationFrame(() => void rf.fitView({ padding: FIT_PADDING, duration: reduced ? 0 : 300 }))
  }, [p.fitNonce, rf, reduced])

  // Pulses: each new one runs once along its edge (a result up the tree edge when it joins child and parent).
  const seen = useRef(new Set<GraphPulse>(p.state.pulses))
  const [running, setRunning] = useState<ReadonlyMap<string, Pulse>>(() => new Map())
  useEffect(() => {
    const fresh = p.state.pulses.filter((x) => !seen.current.has(x))
    if (fresh.length === 0) return
    for (const x of fresh) seen.current.add(x)
    if (seen.current.size > 1000) seen.current = new Set(p.state.pulses)
    if (reduced) return
    const add = new Map<string, Pulse>()
    for (const x of fresh) {
      const from = p.state.nodes.get(x.from)
      const tree = x.kind === 'result' && from?.parent === x.to
      const id = tree ? `t:${x.from}` : x.edge
      const edge = els.edges.find((e) => e.id === id)
      if (!edge) continue
      add.set(id, { key: `${x.id}:${x.kind}:${x.at_ms}`, reverse: edge.source !== x.from, kind: x.kind })
    }
    if (add.size === 0) return
    setRunning((m) => new Map([...m, ...add]))
    const t = setTimeout(() => setRunning((m) => new Map([...m].filter(([k, v]) => add.get(k) !== v))), 1200)
    return () => clearTimeout(t)
  }, [p.state.pulses, p.state.nodes, els.edges, reduced])

  // Follow (D18): pan to the newest pulse or state change unless the user moved in the last 3 s or holds the pointer.
  const userMoved = useRef(0)
  const pointerDown = useRef(false)
  const prevNodes = useRef(p.state.nodes)
  useEffect(() => {
    const before = prevNodes.current
    prevNodes.current = p.state.nodes
    if (!p.follow || pointerDown.current || Date.now() - userMoved.current < 3000) return
    let target: string | null = null
    for (const [id, n] of p.state.nodes) if (before.get(id) !== n && els.pos.has(id)) target = id
    const last = p.state.pulses[p.state.pulses.length - 1]
    if (last && running.size > 0 && els.pos.has(last.to)) target = last.to
    const at = target ? els.pos.get(target) : undefined
    if (at) void rf.setCenter(at.x + BOX.w / 2, at.y + BOX.h / 2, { zoom: rf.getZoom(), duration: reduced ? 0 : 400 })
  }, [p.state, p.follow, els.pos, rf, reduced, running])

  // The first fit is clamped to the compact zoom, so a tall forest can overflow: once it settled, pull the top row
  // below the region's top edge (never under the strip). Only moves when the top is cut.
  useEffect(() => {
    const t = setTimeout(() => {
      const ns = rf.getNodes()
      if (ns.length === 0) return
      const b = rf.getNodesBounds(ns)
      const vp = rf.getViewport()
      if (b.y * vp.zoom + vp.y < TOP_ROOM) void rf.setViewport({ ...vp, y: TOP_ROOM - b.y * vp.zoom })
    }, FIT_SETTLE_MS)
    return () => clearTimeout(t)
  }, [rf])

  // Show in graph: fit the whole path root → node, never zooming in — once per nonce. A highlight alone never moves
  // the viewport. A canvas that mounts because of the click (the region was collapsed) runs its first fit in its first
  // frames; the centring waits until FIT_SETTLE_MS after mount so the fit does not override it. Positions are read at
  // that moment (refs), so later moves do not re-centre.
  const mountedAt = useRef(performance.now())
  const posRef = useRef(els.pos)
  posRef.current = els.pos
  const nodesRef = useRef(p.state.nodes)
  nodesRef.current = p.state.nodes
  const nonce = p.focus?.nonce
  useEffect(() => {
    const f = p.focus
    if (!f) return
    const wait = Math.max(0, FIT_SETTLE_MS - (performance.now() - mountedAt.current))
    const t = setTimeout(() => {
      const path = pathToRoot(nodesRef.current, f.session).filter((id) => posRef.current.has(id))
      if (path.length === 0) return
      void rf.fitView({ nodes: path.map((id) => ({ id })), padding: FIT_PADDING, maxZoom: rf.getZoom(), minZoom: 0.1, duration: reduced ? 0 : 300 })
    }, wait)
    return () => clearTimeout(t)
    // Only a new nonce re-centres.
  }, [nonce, rf])

  const nodes = useMemo<(SessionNodeType | FoldNodeType)[]>(
    () =>
      els.nodes.map((n) =>
        n.type === 'fold'
          ? { id: n.id, type: 'fold', position: n.pos, data: { count: n.count, parent: n.parent }, draggable: false, selectable: true, style: { opacity: n.dim ? DIM : 1 } }
          : {
              id: n.id,
              type: 'session',
              position: n.pos,
              data: { view: n.view, highlighted: n.highlighted, selected: n.id === p.selected, now: p.now },
              draggable: false,
              selectable: true,
              // Off the highlighted path: dimmed but readable, never compounding a TTL fade; a node that needs you
              // (red ring) is never dimmed.
              style: { opacity: n.dim && n.view.attention === 0 ? Math.min(n.view.opacity, DIM) : n.view.opacity },
            },
      ),
    [els.nodes, p.selected, p.now],
  )
  const edges = useMemo<Edge[]>(
    () =>
      els.edges.map((e) =>
        e.type === 'tree'
          ? { id: e.id, type: 'tree', source: e.source, target: e.target, sourceHandle: 'b', targetHandle: 't', selectable: false, focusable: false, data: { onPath: e.onPath, dim: e.dim, pulse: running.get(e.id) ?? null } }
          : {
              id: e.id,
              type: 'message',
              source: e.source,
              target: e.target,
              sourceHandle: e.sourceHandle,
              targetHandle: e.targetHandle,
              animated: e.animated && !reduced,
              // Lit edges draw over the nodes; folded ones pass behind them.
              zIndex: e.active ? 1 : 0,
              data: { count: e.count, active: e.active, animated: e.animated, dim: e.dim, reduced, pulse: running.get(e.id) ?? null },
            },
      ),
    [els.edges, running, reduced],
  )

  const onMove = (_: unknown, vp: Viewport) => {
    const next = lodOf(vp.zoom)
    if (next !== lod) setLod(next)
  }
  return (
    <LodContext.Provider value={lod}>
      <ReactFlow
        nodes={nodes}
        edges={edges}
        nodeTypes={NODE_TYPES}
        edgeTypes={EDGE_TYPES}
        nodesDraggable={false}
        nodesConnectable={false}
        elementsSelectable
        onlyRenderVisibleElements={nodes.length > 50}
        minZoom={0.1}
        maxZoom={2}
        fitView
        // The first fit never zooms out past the compact level: a wide forest opens readable and pans, not as specks.
        fitViewOptions={{ padding: FIT_PADDING, maxZoom: 1, minZoom: 0.4 }}
        forceColorMode={dark ? 'dark' : 'light'}
        onMove={onMove}
        onMoveStart={(e) => {
          if (e) userMoved.current = Date.now()
        }}
        onPointerDown={() => {
          pointerDown.current = true
        }}
        onPointerUp={() => {
          pointerDown.current = false
        }}
        onNodeClick={(_, n) => {
          if (n.type === 'fold') setOpenFolds((s) => new Set([...s, (n.data as FoldData).parent]))
          else p.onNodeClick(n.id)
        }}
        onEdgeClick={(e, edge) => {
          if (edge.type === 'message') p.onEdgeClick(edge.id, { x: e.clientX, y: e.clientY })
        }}
        onPaneClick={p.onPaneClick}
      >
        <Background variant={BackgroundVariant.Dots} gap={20} size={1.2} color="color-mix(in oklab, var(--color-fg-tertiary) 45%, transparent)" />
        <Controls showInteractive={false} fitViewOptions={{ padding: FIT_PADDING, duration: reduced ? 0 : 300 }} />
        <MiniMap
          pannable
          zoomable
          nodeColor={miniColor}
          nodeStrokeWidth={0}
          nodeBorderRadius={6}
          maskColor="color-mix(in oklab, var(--color-background) 70%, transparent)"
          className="max-sm:!hidden"
          style={{ width: 150, height: 84 }}
        />
      </ReactFlow>
    </LodContext.Provider>
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
