// 208: every visual encoding of the agent graph in one place — node status (tone + glyph), the marks a node can carry,
// and the edge styles. The node card, the minimap, the edges and the legend all read these tables, so the legend cannot
// drift from what the canvas draws. Colours are theme tokens only (both themes define each one). Pure, no React.
import type { GraphNode } from '@/rpc/bindings/method.agent.output'

export type StatusKey = 'working' | 'needs' | 'waiting' | 'failed' | 'finished' | 'cancelled' | 'idle'
export type GlyphKey = 'pulse' | 'alert' | 'hourglass' | 'cross' | 'check' | 'square' | 'ring'
export type StatusStyle = { label: string; detail: string; tone: string; glyph: GlyphKey }

/** Node status, in legend order. `tone` is a CSS colour (a theme token). */
export const STATUS: Record<StatusKey, StatusStyle> = {
  working: { label: 'Working', detail: 'running a turn; the dot breathes', tone: 'var(--color-success)', glyph: 'pulse' },
  needs: { label: 'Needs you', detail: 'asks for approval or an answer from you', tone: 'var(--color-warning)', glyph: 'alert' },
  waiting: { label: 'Waiting', detail: 'on another agent, a job or its children', tone: 'var(--chart-3)', glyph: 'hourglass' },
  failed: { label: 'Failed', detail: 'errored, failed or timed out', tone: 'var(--color-danger)', glyph: 'cross' },
  finished: { label: 'Finished', detail: 'done; fades out over its TTL', tone: 'var(--chart-1)', glyph: 'check' },
  cancelled: { label: 'Cancelled', detail: 'stopped before it finished', tone: 'var(--chart-6)', glyph: 'square' },
  idle: { label: 'Idle', detail: 'nothing running', tone: 'var(--color-fg-tertiary)', glyph: 'ring' },
}
export const STATUS_ORDER = Object.keys(STATUS) as StatusKey[]

const OBSERVED: Record<NonNullable<GraphNode['observed']>['state'], StatusKey> = {
  pending: 'working',
  running: 'working',
  requires_action: 'needs',
  idle: 'idle',
  completed: 'finished',
  failed: 'failed',
  cancelled: 'cancelled',
  unknown: 'idle',
}

/** The one status a node is drawn in. A watch-only child follows its harness's state. */
export function statusOf(node: GraphNode): StatusKey {
  if (node.observed) return OBSERVED[node.observed.state]
  const s = node.activity.state.state
  if (s === 'error' || node.outcome === 'failed' || node.outcome === 'timed_out') return 'failed'
  if (s === 'approval' || node.waits.includes('user')) return 'needs'
  if (s === 'awaiting' || node.waits.length > 0) return 'waiting'
  if (s === 'running') return 'working'
  if (s === 'cancelled' || node.outcome === 'cancelled') return 'cancelled'
  if (node.outcome === 'finished') return 'finished'
  return 'idle'
}

/** What kind of node a card is: roots and desks carry a heavier frame and a kind tag; peers are dashed. */
export const KIND: Record<GraphNode['kind'], { label: string; tag: string | null; detail: string }> = {
  desk: { label: 'Desk', tag: 'desk', detail: 'a standing desk session, top of its tree' },
  root: { label: 'Chat', tag: 'chat', detail: 'a chat you started, top of its tree' },
  child: { label: 'Subagent', tag: null, detail: 'spawned by the node above it' },
  observed: { label: 'Watch-only subagent', tag: null, detail: 'run inside another harness; marked with an eye' },
  peer: { label: 'Peer', tag: 'peer', detail: 'another agent outside your trees; dashed frame' },
}

export type EdgeKey = 'spawn' | 'path' | 'message' | 'recent' | 'flight'
export type EdgeStyle = { label: string; detail: string; stroke: string; width: number; dash: string | null; opacity: number; flow: boolean; marker: string }

/** Edge styles, in legend order. `marker` is the id of the arrowhead the canvas defines. */
export const EDGE: Record<EdgeKey, EdgeStyle> = {
  spawn: { label: 'Spawned', detail: 'parent → subagent', stroke: 'var(--color-border-strong)', width: 1.5, dash: null, opacity: 1, flow: false, marker: 'ag-arrow-spawn' },
  path: { label: 'Selected path', detail: 'the open item’s session up to its root', stroke: 'var(--color-fg-accent)', width: 2.25, dash: null, opacity: 1, flow: false, marker: 'ag-arrow-path' },
  message: { label: 'Messages', detail: 'agents talked; arrows show who wrote to whom', stroke: 'var(--color-fg-tertiary)', width: 1, dash: '4 4', opacity: 0.55, flow: false, marker: 'ag-arrow-message' },
  recent: { label: 'Recent messages', detail: 'within the message TTL', stroke: 'var(--chart-5)', width: 1.75, dash: '6 4', opacity: 1, flow: false, marker: 'ag-arrow-recent' },
  flight: { label: 'In flight', detail: 'a message or answer is still open; the dashes flow', stroke: 'var(--chart-5)', width: 2, dash: '6 4', opacity: 1, flow: true, marker: 'ag-arrow-recent' },
}
export const EDGE_ORDER = Object.keys(EDGE) as EdgeKey[]
/** The arrowheads the canvas defines: one per distinct marker id, in its edge's colour. */
export const MARKERS: { id: string; fill: string }[] = [...new Map(EDGE_ORDER.map((k) => [EDGE[k].marker, EDGE[k].stroke])).entries()].map(([id, fill]) => ({ id, fill }))

/** Dots that run along an edge once when something arrives. */
export const PULSE = {
  message: { label: 'Message arrives', tone: 'var(--chart-5)' },
  result: { label: 'Result returns', tone: 'var(--color-success)' },
  failed: { label: 'Failure returns', tone: 'var(--color-danger)' },
} as const

/** A node's marks beyond its status: drawn by NodeBody / Canvas with these values. */
export const MARK = {
  ring: { label: 'Red ring and count', detail: 'items in your inbox from this session', tone: 'var(--color-danger)' },
  selected: { label: 'Accent border', detail: 'selected node, or on the selected path', tone: 'var(--color-fg-accent)' },
  fade: { label: 'Fading', detail: 'age: past ¾ of its TTL a node fades out', opacity: 0.5 },
  ghost: { label: 'Ghost', detail: 'expired, kept because a subagent below still shows', opacity: 0.25 },
  dim: { label: 'Dimmed', detail: 'off the selected path', opacity: 0.5 },
  fold: { label: '+N finished', detail: 'a folded group of finished subagents; click to open' },
} as const

/** Pulse colour by the engine's pulse kind. */
export function pulseTone(kind: string, tree: boolean): string {
  if (kind === 'failed') return PULSE.failed.tone
  return tree || kind === 'result' ? PULSE.result.tone : PULSE.message.tone
}

/** Message edge style key from its state. */
export function messageKey(active: boolean, animated: boolean): EdgeKey {
  return animated ? 'flight' : active ? 'recent' : 'message'
}
