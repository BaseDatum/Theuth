// 208 §8.6: the React Flow canvas — the only importer of `@xyflow/react` and its CSS besides edges.tsx, loaded lazily by
// Region.tsx so the library is its own chunk. Thin: positions, folds, dimming, edge handles and the opening viewport
// come from elements.ts; colours from encoding.ts.
import '@xyflow/react/dist/style.css'
import {
  Background,
  BackgroundVariant,
  ControlButton,
  Controls,
  type Edge,
  Handle,
  MiniMap,
  type Node,
  type NodeProps,
  NodeToolbar,
  Position,
  ReactFlow,
  ReactFlowProvider,
  useReactFlow,
  type Viewport,
} from '@xyflow/react'
import { Scan } from 'lucide-react'
import { memo, useContext, useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react'
import type { GraphPulse } from '@/rpc/bindings/event.agent_2egraph.output'
import { canvasElements, type Extent, initialViewport, lodOf } from './elements'
import { EdgeMarkers, MessageEdge, type Pulse, TreeEdge } from './edges'
import { MARK, STATUS, statusOf } from './encoding'
import { HoverCard } from './HoverCard'
import { BOX, layoutCache } from './layout'
import { type Lod, LodContext, NodeBody } from './NodeBody'
import type { GraphState } from './reducer'
import { pathToRoot } from './selectors'
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

const HANDLE = { opacity: 0, width: 1, height: 1, minWidth: 0, minHeight: 0, border: 0, pointerEvents: 'none' } as const
/** How long after mount the opening viewport may still move (a "Show in graph" centring waits for it). */
const FIT_SETTLE_MS = 350
/** Room kept above the top row by a path fit, so no node sits under the region's top edge. */
const FIT_PADDING = { top: '48px', bottom: '24px', x: '24px' } as const
const MOVE_MS = 300

function Handles() {
  return (
    <>
      <Handle type="target" position={Position.Top} id="t" style={HANDLE} isConnectable={false} />
      <Handle type="source" position={Position.Top} id="ts" style={HANDLE} isConnectable={false} />
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
  const lod = useContext(LodContext)
  return (
    <>
      <Handles />
      <div
        className="ag-node flex h-16 w-[200px] items-center justify-center gap-1.5 rounded-[10px] border border-dashed border-border-strong bg-layer-1 font-medium text-fg-secondary"
        style={{ fontSize: lod === 'full' ? 13 : 20 }}
        data-node-id={`fold:${data.parent}`}
        title={MARK.fold.detail}
      >
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

/** Minimap fill: danger for a node that needs you (red ring), else its status tone. */
function miniColor(n: Node): string {
  if (n.type !== 'session') return 'var(--color-border-strong)'
  const v = (n.data as SessionData).view
  return v.attention > 0 ? MARK.ring.tone : STATUS[statusOf(v.node)].tone
}

function Flow(p: CanvasProps) {
  const rf = useReactFlow()
  const reduced = useSyncExternalStore(subscribeReduced, () => window.matchMedia(REDUCED).matches)
  const dark = useSyncExternalStore(subscribeTheme, () => document.documentElement.classList.contains('dark'))
  const [lod, setLod] = useState<Lod>('full')
  const [openFolds, setOpenFolds] = useState<ReadonlySet<string>>(() => new Set())
  const [hover, setHover] = useState<string | null>(null)
  const box = useRef<HTMLDivElement>(null)
  const cache = useRef(layoutCache())
  const slots = useRef<ReadonlyMap<string, number> | null>(null)
  const fitSeen = useRef(p.fitNonce)
  const repack = fitSeen.current !== p.fitNonce
  const els = useMemo(
    () => canvasElements(p.state, p.view, p.highlight, openFolds, repack ? null : slots.current, cache.current),
    [p.state, p.view, p.highlight, openFolds, repack],
  )
  slots.current = els.slots
  const extent = useRef<Extent>(els.extent)
  extent.current = els.extent

  /** The whole drawing (Fit, the Controls' fit) or the opening view (readable rows, the first tree whole). */
  const place = (all: boolean, duration: number) => {
    const el = box.current
    if (!el) return
    void rf.setViewport(initialViewport(extent.current, el.clientWidth, el.clientHeight, all ? 0.1 : undefined), { duration })
  }
  const placeRef = useRef(place)
  placeRef.current = place
  useEffect(() => {
    if (fitSeen.current === p.fitNonce) return
    fitSeen.current = p.fitNonce
    requestAnimationFrame(() => placeRef.current(true, reduced ? 0 : MOVE_MS))
  }, [p.fitNonce, reduced])

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

  // Show in graph: fit the whole path root → node, never zooming in — once per nonce. A highlight alone never moves
  // the viewport. A canvas that mounts because of the click (the region was collapsed) places its opening view first;
  // the centring waits until FIT_SETTLE_MS after mount so that does not override it. Positions are read at that moment
  // (refs), so later moves do not re-centre.
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
      void rf.fitView({ nodes: path.map((id) => ({ id })), padding: FIT_PADDING, maxZoom: rf.getZoom(), minZoom: 0.1, duration: reduced ? 0 : MOVE_MS })
    }, wait)
    return () => clearTimeout(t)
    // Only a new nonce re-centres.
  }, [nonce, rf])

  const nodes = useMemo<(SessionNodeType | FoldNodeType)[]>(
    () =>
      els.nodes.map((n) =>
        n.type === 'fold'
          ? { id: n.id, type: 'fold', position: n.pos, data: { count: n.count, parent: n.parent }, draggable: false, selectable: true, ariaLabel: `${n.count} finished subagents, folded`, style: { opacity: n.dim ? MARK.dim.opacity : 1 } }
          : {
              id: n.id,
              type: 'session',
              position: n.pos,
              data: { view: n.view, highlighted: n.highlighted, selected: n.id === p.selected, now: p.now },
              draggable: false,
              selectable: true,
              // Off the highlighted path: dimmed but readable, never compounding a TTL fade; a node that needs you
              // (red ring) is never dimmed.
              style: { opacity: n.dim && n.view.attention === 0 ? Math.min(n.view.opacity, MARK.dim.opacity) : n.view.opacity },
            },
      ),
    [els.nodes, p.selected, p.now],
  )
  const edges = useMemo<Edge[]>(
    () =>
      els.edges.map((e) =>
        e.type === 'tree'
          ? { id: e.id, type: 'tree', source: e.source, target: e.target, sourceHandle: 'b', targetHandle: 't', selectable: false, focusable: false, zIndex: e.onPath ? 1 : 0, data: { onPath: e.onPath, dim: e.dim, pulse: running.get(e.id) ?? null } }
          : {
              id: e.id,
              type: 'message',
              source: e.source,
              target: e.target,
              sourceHandle: e.sourceHandle,
              targetHandle: e.targetHandle,
              // Lit edges draw over the nodes; folded ones pass behind them.
              zIndex: e.active ? 1 : 0,
              ariaLabel: `${e.count} messages`,
              data: { fwd: e.fwd, back: e.back, active: e.active, animated: e.animated, dim: e.dim, reduced, pulse: running.get(e.id) ?? null },
            },
      ),
    [els.edges, running, reduced],
  )

  const onMove = (_: unknown, vp: Viewport) => {
    const next = lodOf(vp.zoom)
    if (next !== lod) setLod(next)
  }
  const hovered = hover ? p.view.nodes.get(hover) : undefined
  const hoverAt = hover ? els.pos.get(hover) : undefined
  // The card opens below a node in the upper half of the canvas, above it otherwise, so it stays on screen.
  const below = hoverAt ? (hoverAt.y + BOX.h / 2) * rf.getZoom() + rf.getViewport().y < (box.current?.clientHeight ?? 0) / 2 : true
  return (
    <LodContext.Provider value={lod}>
      <div
        ref={box}
        className="size-full"
        data-lod={lod}
        onFocus={(e) => {
          const id = (e.target as HTMLElement).closest?.('.react-flow__node-session')?.getAttribute('data-id')
          setHover(id ?? null)
        }}
        onBlur={() => setHover(null)}
      >
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
          onInit={() => placeRef.current(false, 0)}
          forceColorMode={dark ? 'dark' : 'light'}
          onMove={onMove}
          onMoveStart={(e) => {
            setHover(null)
            if (e) userMoved.current = Date.now()
          }}
          onPointerDown={() => {
            pointerDown.current = true
          }}
          onPointerUp={() => {
            pointerDown.current = false
          }}
          onNodeMouseEnter={(_, n) => setHover(n.type === 'session' ? n.id : null)}
          onNodeMouseLeave={() => setHover(null)}
          onNodeClick={(_, n) => {
            if (n.type === 'fold') setOpenFolds((s) => new Set([...s, (n.data as FoldData).parent]))
            else p.onNodeClick(n.id)
          }}
          onEdgeClick={(e, edge) => {
            if (edge.type === 'message') p.onEdgeClick(edge.id, { x: e.clientX, y: e.clientY })
          }}
          onPaneClick={p.onPaneClick}
        >
          <EdgeMarkers />
          <Background variant={BackgroundVariant.Dots} gap={24} size={1.25} color="color-mix(in oklab, var(--color-fg-tertiary) 38%, transparent)" />
          <Controls showInteractive={false} showFitView={false} position="bottom-left">
            <ControlButton onClick={() => place(true, reduced ? 0 : MOVE_MS)} aria-label="Fit everything" title="Fit everything">
              <Scan />
            </ControlButton>
          </Controls>
          <MiniMap
            pannable
            zoomable
            nodeColor={miniColor}
            nodeStrokeWidth={0}
            nodeBorderRadius={14}
            bgColor="var(--bg-layer-1)"
            maskColor="color-mix(in oklab, var(--bg-page) 62%, transparent)"
            maskStrokeColor="var(--color-fg-accent)"
            maskStrokeWidth={2}
            ariaLabel="Agent graph overview"
            className="max-sm:!hidden"
            style={{ width: 184, height: 104 }}
          />
          {hovered && hover && (
            <NodeToolbar nodeId={hover} isVisible position={below ? Position.Bottom : Position.Top} offset={10}>
              <HoverCard node={hovered.node} attention={hovered.attention} now={p.now} expiresAt={hovered.expiresAt} />
            </NodeToolbar>
          )}
        </ReactFlow>
      </div>
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
