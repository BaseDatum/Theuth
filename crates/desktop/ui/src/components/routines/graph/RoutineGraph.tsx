// 214 D9: the routine graph canvas — default export for `React.lazy` in RoutineDetail, so `@xyflow/react` and its
// stylesheet load in their own chunk like the attention graph's. Read-only: cards are neither draggable nor
// connectable; pan and pinch-zoom work, the wheel scrolls the page. Positions come from `routineGraph` (model.ts).
import '@xyflow/react/dist/style.css'
import { useNavigate } from '@tanstack/react-router'
import {
  Background,
  BackgroundVariant,
  BaseEdge,
  ControlButton,
  Controls,
  type Edge,
  EdgeLabelRenderer,
  type EdgeProps,
  getSmoothStepPath,
  ReactFlow,
  ReactFlowProvider,
  useReactFlow,
} from '@xyflow/react'
import { Scan } from 'lucide-react'
import { memo, useEffect, useMemo, useRef, useSyncExternalStore } from 'react'
import { engineName } from '@/components/tasks/home'
import { useMesh } from '@/components/tasks/queries'
import type { ScheduleFile, ScheduleInfo, SensorInfo } from '@/rpc/bindings/method.schedule.output'
import { useRoutineHistory } from '../queries'
import { type EdgeKind, routineGraph } from './model'
import { Card, type CardNode, HistoryCard, PromptCard } from './nodes'

export type RoutineGraphProps = { info: ScheduleInfo; file: ScheduleFile; sensors: SensorInfo[] | undefined; chat: string | undefined }

type EdgeData = { kind: EdgeKind; label: string | null; lane: number | null }
type RoutineEdgeType = Edge<EdgeData, 'routine'>

const ARROW = { muted: 'rg-arrow', accent: 'rg-arrow-accent' } as const
const STROKE: Record<EdgeKind, { stroke: string; dash?: string; marker: string }> = {
  flow: { stroke: 'var(--fg-tertiary)', marker: ARROW.muted },
  attach: { stroke: 'var(--border-strong)', dash: '4 4', marker: ARROW.muted },
  stop: { stroke: 'var(--fg-tertiary)', dash: '6 4', marker: ARROW.muted },
  loop: { stroke: 'var(--fg-accent)', dash: '6 4', marker: ARROW.accent },
}
const RADIUS = 12

function Markers() {
  return (
    <svg aria-hidden className="pointer-events-none absolute size-0">
      <defs>
        {[
          [ARROW.muted, 'var(--fg-tertiary)'],
          [ARROW.accent, 'var(--fg-accent)'],
        ].map(([id, fill]) => (
          <marker key={id} id={id} viewBox="0 0 10 10" refX="8.5" refY="5" markerWidth="9" markerHeight="9" markerUnits="userSpaceOnUse" orient="auto-start-reverse">
            <path d="M1.5 1.5 L8.5 5 L1.5 8.5 Q3.2 5 1.5 1.5 Z" style={{ fill }} />
          </marker>
        ))}
      </defs>
    </svg>
  )
}

/** Main-row and attach edges as rounded steps; the loop-back down into its lane, left, and up into Trigger. */
const RoutineEdge = memo(function RoutineEdge(p: EdgeProps<RoutineEdgeType>) {
  const d = p.data
  const kind = d?.kind ?? 'flow'
  let path: string
  let lx: number
  let ly: number
  if (kind === 'loop' && d?.lane != null) {
    const y = d.lane
    const r = Math.min(RADIUS, Math.abs(y - p.sourceY), Math.abs(y - p.targetY))
    path = `M ${p.sourceX} ${p.sourceY} L ${p.sourceX} ${y - r} Q ${p.sourceX} ${y} ${p.sourceX - r} ${y} L ${p.targetX + r} ${y} Q ${p.targetX} ${y} ${p.targetX} ${y - r} L ${p.targetX} ${p.targetY}`
    lx = (p.sourceX + p.targetX) / 2
    ly = y
  } else {
    ;[path, lx, ly] = getSmoothStepPath({ ...p, borderRadius: RADIUS })
    if (kind === 'flow') ly -= 11
  }
  const s = STROKE[kind]
  return (
    <>
      <BaseEdge id={p.id} path={path} interactionWidth={0} markerEnd={`url(#${s.marker})`} style={{ stroke: s.stroke, strokeWidth: 1.5, strokeDasharray: s.dash, strokeLinecap: 'round', strokeLinejoin: 'round' }} />
      {d?.label && (
        <EdgeLabelRenderer>
          <div
            className="nodrag nopan pointer-events-none absolute rounded-full border border-border-subtle bg-background px-1.5 text-[10.5px] leading-4 whitespace-nowrap text-fg-secondary"
            style={{ transform: `translate(-50%, -50%) translate(${lx}px, ${ly}px)`, ...(kind === 'loop' ? { color: 'var(--fg-accent)' } : {}) }}
          >
            {d.label}
          </div>
        </EdgeLabelRenderer>
      )}
    </>
  )
})

const NODE_TYPES = { card: Card, prompt: PromptCard, history: HistoryCard }
const EDGE_TYPES = { routine: RoutineEdge }
/** Room kept around the drawing by a fit, in screen pixels. */
const FIT_PAD = 16

const subscribeTheme = (cb: () => void) => {
  const o = new MutationObserver(cb)
  o.observe(document.documentElement, { attributes: true, attributeFilter: ['class'] })
  return () => o.disconnect()
}

function Flow({ info, file, sensors, chat }: RoutineGraphProps) {
  const rf = useReactFlow()
  const navigate = useNavigate()
  const history = useRoutineHistory(info.name).data
  const mesh = useMesh().data
  const dark = useSyncExternalStore(subscribeTheme, () => document.documentElement.classList.contains('dark'))
  const g = useMemo(() => routineGraph(info, file, history ?? [], sensors, { machine: mesh ? (id) => engineName(mesh, id) : undefined }), [info, file, history, sensors, mesh])
  const nodes = useMemo<CardNode[]>(
    () =>
      g.nodes.map((n) => ({
        id: n.id,
        type: n.id === 'prompt' ? 'prompt' : n.id === 'history' ? 'history' : 'card',
        position: { x: n.x, y: n.y },
        data: { ...n.data, w: n.w, h: n.h },
        draggable: false,
        selectable: false,
        connectable: false,
        focusable: n.data.section !== null,
        ariaLabel: n.data.title,
      })),
    [g],
  )
  const edges = useMemo<RoutineEdgeType[]>(
    () => g.edges.map((e) => ({ id: e.id, type: 'routine', source: e.source, target: e.target, sourceHandle: e.sourceHandle, targetHandle: e.targetHandle, selectable: false, focusable: false, data: { kind: e.kind, label: e.label, lane: e.lane } })),
    [g],
  )

  // Fit the whole routine (its model bounds, which include the loop lane; never above 100 %) when the drawing changes
  // size or the canvas resizes, until the user pans or zooms. The bounds are known before React Flow measures a card.
  const box = useRef<HTMLDivElement>(null)
  const auto = useRef(true)
  const fit = () => {
    const el = box.current
    if (!el || el.clientWidth === 0) return
    const zoom = Math.max(0.2, Math.min(1, (el.clientWidth - 2 * FIT_PAD) / g.width, (el.clientHeight - 2 * FIT_PAD) / g.height))
    void rf.setViewport({ x: (el.clientWidth - g.width * zoom) / 2, y: (el.clientHeight - g.height * zoom) / 2, zoom })
  }
  const fitRef = useRef(fit)
  fitRef.current = fit
  useEffect(() => {
    if (auto.current) fitRef.current()
  }, [g.width, g.height])
  useEffect(() => {
    const el = box.current
    if (!el) return
    const o = new ResizeObserver(() => {
      if (auto.current) fitRef.current()
    })
    o.observe(el)
    return () => o.disconnect()
  }, [])

  return (
    <div ref={box} className="size-full">
      <ReactFlow
        nodes={nodes}
        edges={edges}
        nodeTypes={NODE_TYPES}
        edgeTypes={EDGE_TYPES}
        nodesDraggable={false}
        nodesConnectable={false}
        elementsSelectable={false}
        onInit={() => fitRef.current()}
        minZoom={0.2}
        maxZoom={1.5}
        zoomOnScroll={false}
        preventScrolling={false}
        forceColorMode={dark ? 'dark' : 'light'}
        onMoveStart={(e) => {
          if (e) auto.current = false
        }}
        onNodeClick={(_, n) => {
          const section = (n as CardNode).data.section
          if (section) void navigate({ to: '/routines/$name/$section', params: { name: info.name, section }, search: chat ? { chat } : {} })
        }}
      >
        <Markers />
        <Background variant={BackgroundVariant.Dots} gap={24} size={1.25} color="color-mix(in oklab, var(--color-fg-tertiary) 38%, transparent)" />
        <Controls showInteractive={false} showFitView={false} position="top-left" onZoomIn={() => (auto.current = false)} onZoomOut={() => (auto.current = false)}>
          <ControlButton
            onClick={() => {
              auto.current = true
              fit()
            }}
            aria-label="Fit the routine"
            title="Fit the routine"
          >
            <Scan />
          </ControlButton>
        </Controls>
      </ReactFlow>
    </div>
  )
}

/** The lazily loaded routine graph (default export for `React.lazy`). */
export default function RoutineGraph(props: RoutineGraphProps) {
  return (
    <ReactFlowProvider>
      <Flow {...props} />
    </ReactFlowProvider>
  )
}
