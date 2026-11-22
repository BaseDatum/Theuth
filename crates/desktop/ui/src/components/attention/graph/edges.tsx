// 208 §7.3: the canvas's two edge kinds. Tree edges: smooth-step, solid, neutral; a child's result runs up them as a
// dot. Message edges: bezier, dashed; accent while active, animated while in flight, folded to 1 px neutral at 50 %;
// the label is the exchange count, never text. Imported only by Canvas.tsx.
import { BaseEdge, EdgeLabelRenderer, type Edge, type EdgeProps, getBezierPath, getSmoothStepPath } from '@xyflow/react'
import { memo } from 'react'

export type Pulse = { key: string; reverse: boolean; kind: string }
export type TreeEdgeData = { onPath: boolean; dim: boolean; pulse: Pulse | null }
export type MessageEdgeData = { count: number; active: boolean; animated: boolean; dim: boolean; reduced: boolean; pulse: Pulse | null }
export type TreeEdgeType = Edge<TreeEdgeData, 'tree'>
export type MessageEdgeType = Edge<MessageEdgeData, 'message'>

/** One dot running along `path` (reverse: target → source), once. */
function PulseDot({ path, pulse, color }: { path: string; pulse: Pulse; color: string }) {
  return (
    <circle key={pulse.key} r={5} fill={color}>
      <animateMotion dur="0.9s" repeatCount="1" fill="freeze" path={path} keyPoints={pulse.reverse ? '1;0' : '0;1'} keyTimes="0;1" calcMode="linear" />
    </circle>
  )
}

export const TreeEdge = memo(function TreeEdge(props: EdgeProps<TreeEdgeType>) {
  const [path] = getSmoothStepPath({ ...props, borderRadius: 8 })
  const d = props.data
  return (
    <>
      <BaseEdge
        id={props.id}
        path={path}
        interactionWidth={0}
        style={{ stroke: d?.onPath ? 'var(--color-fg-accent)' : 'var(--color-border-strong)', strokeWidth: d?.onPath ? 2 : 1.25, opacity: d?.dim ? 0.6 : 1 }}
      />
      {d?.pulse && <PulseDot path={path} pulse={d.pulse} color="var(--color-success)" />}
    </>
  )
})

export const MessageEdge = memo(function MessageEdge(props: EdgeProps<MessageEdgeType>) {
  const [path, labelX, labelY] = getBezierPath(props)
  const d = props.data
  const active = d?.active ?? false
  const stroke = active ? 'var(--color-fg-accent)' : 'var(--color-fg-tertiary)'
  const opacity = (active ? 1 : 0.5) * (d?.dim ? 0.6 : 1)
  return (
    <>
      <BaseEdge id={props.id} path={path} interactionWidth={16} style={{ stroke, strokeWidth: active ? 1.5 : 1, strokeDasharray: '5 4', opacity }} />
      {d?.animated && d.reduced && (
        // Reduced motion: a static "in flight" dot instead of the moving dash.
        <circle cx={labelX} cy={labelY - 12} r={3.5} fill="var(--color-fg-accent)" />
      )}
      {d?.pulse && !d.reduced && <PulseDot path={path} pulse={d.pulse} color={d.pulse.kind === 'failed' ? 'var(--color-danger)' : 'var(--color-fg-accent)'} />}
      {d && d.count > 0 && (
        <EdgeLabelRenderer>
          <div
            className="nodrag nopan pointer-events-none absolute rounded-full border border-border-subtle bg-background px-1.5 text-[10px] leading-4 text-fg-secondary tabular-nums"
            style={{ transform: `translate(-50%, -50%) translate(${labelX}px, ${labelY}px)`, opacity }}
          >
            ↔ {d.count}
          </div>
        </EdgeLabelRenderer>
      )}
    </>
  )
})
