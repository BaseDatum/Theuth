// 208 §7.3: the canvas's two edge kinds, styled from encoding.ts. Spawn edges: rounded smooth-step, solid, an arrow into
// the child; a child's result runs up them as a dot. Message edges: dashed curves with arrowheads toward whoever was
// written to — between two cards on the same row they arc over the row instead of cutting through the cards between;
// recent ones in the message tone, in-flight ones flow (not under reduced motion). The label is the exchange count.
// Imported only by Canvas.tsx.
import { BaseEdge, EdgeLabelRenderer, type Edge, type EdgeProps, getBezierPath, getSmoothStepPath, Position } from '@xyflow/react'
import { MessageSquare } from 'lucide-react'
import { memo, useContext } from 'react'
import { arcRise } from './elements'
import { EDGE, MARKERS, messageKey, pulseTone } from './encoding'
import { LodContext } from './NodeBody'

export type Pulse = { key: string; reverse: boolean; kind: string }
export type TreeEdgeData = { onPath: boolean; dim: boolean; pulse: Pulse | null }
export type MessageEdgeData = { fwd: number; back: number; active: boolean; animated: boolean; dim: boolean; reduced: boolean; pulse: Pulse | null }
export type TreeEdgeType = Edge<TreeEdgeData, 'tree'>
export type MessageEdgeType = Edge<MessageEdgeData, 'message'>

/** Opacity of an edge off the highlighted path. */
const EDGE_DIM = 0.35

/** The arrowheads every edge references, one per edge colour; rendered once by the canvas. */
export function EdgeMarkers() {
  return (
    <svg aria-hidden className="pointer-events-none absolute size-0">
      <defs>
        {MARKERS.map((m) => (
          <marker key={m.id} id={m.id} viewBox="0 0 10 10" refX="8.5" refY="5" markerWidth="9" markerHeight="9" markerUnits="userSpaceOnUse" orient="auto-start-reverse">
            <path d="M1.5 1.5 L8.5 5 L1.5 8.5 Q3.2 5 1.5 1.5 Z" style={{ fill: m.fill }} />
          </marker>
        ))}
      </defs>
    </svg>
  )
}

/** One dot running along `path` (reverse: target → source), once. */
function PulseDot({ path, pulse, color }: { path: string; pulse: Pulse; color: string }) {
  return (
    <g key={pulse.key}>
      <circle r={7} fill={color} opacity={0.25}>
        <animateMotion dur="0.9s" repeatCount="1" fill="freeze" path={path} keyPoints={pulse.reverse ? '1;0' : '0;1'} keyTimes="0;1" calcMode="spline" keySplines="0.4 0 0.2 1" />
      </circle>
      <circle r={4} fill={color}>
        <animateMotion dur="0.9s" repeatCount="1" fill="freeze" path={path} keyPoints={pulse.reverse ? '1;0' : '0;1'} keyTimes="0;1" calcMode="spline" keySplines="0.4 0 0.2 1" />
      </circle>
    </g>
  )
}

export const TreeEdge = memo(function TreeEdge(props: EdgeProps<TreeEdgeType>) {
  const [path] = getSmoothStepPath({ ...props, borderRadius: 14, offset: 18 })
  const d = props.data
  const s = d?.onPath ? EDGE.path : EDGE.spawn
  return (
    <>
      <BaseEdge
        id={props.id}
        path={path}
        interactionWidth={0}
        markerEnd={`url(#${s.marker})`}
        style={{ stroke: s.stroke, strokeWidth: s.width, strokeLinecap: 'round', strokeLinejoin: 'round', opacity: d?.dim ? EDGE_DIM : s.opacity }}
      />
      {d?.pulse && <PulseDot path={path} pulse={d.pulse} color={pulseTone(d.pulse.kind, true)} />}
    </>
  )
})

function arcPath(sx: number, sy: number, tx: number, ty: number): [string, number, number] {
  const rise = arcRise(tx - sx)
  // A cubic with both controls at `rise` above peaks at ¾ of it.
  const top = Math.min(sy, ty) - rise
  return [`M ${sx} ${sy} C ${sx} ${top}, ${tx} ${top}, ${tx} ${ty}`, (sx + tx) / 2, Math.min(sy, ty) - rise * 0.75]
}

export const MessageEdge = memo(function MessageEdge(props: EdgeProps<MessageEdgeType>) {
  const lod = useContext(LodContext)
  const arc = props.sourcePosition === Position.Top && props.targetPosition === Position.Top
  const [path, labelX, labelY] = arc ? arcPath(props.sourceX, props.sourceY, props.targetX, props.targetY) : getBezierPath({ ...props, curvature: 0.35 })
  const d = props.data
  const s = EDGE[messageKey(d?.active ?? false, d?.animated ?? false)]
  const flow = s.flow && !d?.reduced
  const opacity = s.opacity * (d?.dim ? EDGE_DIM : 1)
  const count = (d?.fwd ?? 0) + (d?.back ?? 0)
  // Badges grow as the canvas zooms out (they are screen furniture, not node content) and give way at dot level.
  const scale = lod === 'compact' ? 1.7 : 1
  return (
    <>
      <BaseEdge
        id={props.id}
        path={path}
        interactionWidth={18}
        className={flow ? 'ag-flow' : undefined}
        markerEnd={d && d.fwd > 0 ? `url(#${s.marker})` : undefined}
        markerStart={d && d.back > 0 ? `url(#${s.marker})` : undefined}
        style={{ stroke: s.stroke, strokeWidth: s.width, strokeDasharray: s.dash ?? undefined, strokeLinecap: 'round', opacity }}
      />
      {d?.animated && d.reduced && (
        // Reduced motion: a static "in flight" dot instead of the flowing dash.
        <circle cx={labelX} cy={labelY - 14} r={3.5} fill={s.stroke} />
      )}
      {d?.pulse && !d.reduced && <PulseDot path={path} pulse={d.pulse} color={pulseTone(d.pulse.kind, false)} />}
      {count > 0 && lod !== 'dot' && (
        <EdgeLabelRenderer>
          <div
            className="nodrag nopan pointer-events-none absolute flex items-center gap-0.5 rounded-full border bg-elevated px-1.5 text-[10px] leading-4 font-medium text-fg-secondary tabular-nums shadow-xs"
            style={{ transform: `translate(-50%, -50%) translate(${labelX}px, ${labelY}px) scale(${scale})`, opacity: d?.dim ? EDGE_DIM : 1, borderColor: s === EDGE.message ? 'var(--color-border-subtle)' : s.stroke }}
            title={`${count} message${count === 1 ? '' : 's'}`}
          >
            <MessageSquare aria-hidden className="size-2.5" style={{ color: s.stroke }} />
            {count}
            {d && d.fwd > 0 && d.back > 0 && <span aria-hidden className="text-[9px] text-fg-tertiary">⇄</span>}
          </div>
        </EdgeLabelRenderer>
      )}
    </>
  )
})
