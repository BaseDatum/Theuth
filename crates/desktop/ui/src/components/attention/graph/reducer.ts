// 208 §8.4: the client's copy of the engine graph — a snapshot (`agent_graph`) plus ordered deltas (`agent.graph@1`).
// Untouched nodes and edges keep object identity so memoised node components skip re-rendering.
import type { AgentGraphChanged, GraphPulse } from '@/rpc/bindings/event.agent_2egraph.output'
import type { GraphCursor } from '@/rpc/bindings/method.agent.input'
import type { AgentGraph, GraphEdge, GraphNode, GraphOlder } from '@/rpc/bindings/method.agent.output'

export type GraphState = {
  epoch: number
  seq: number
  at: number
  window: number
  nodes: ReadonlyMap<string, GraphNode>
  edges: ReadonlyMap<string, GraphEdge>
  pulses: readonly GraphPulse[]
  older: GraphOlder | null
}

/** Pulses kept for the canvas to animate (oldest dropped). */
export const PULSE_KEEP = 200

export function fromSnapshot(s: AgentGraph): GraphState {
  return {
    epoch: s.epoch_ms,
    seq: s.seq,
    at: s.at_ms,
    window: s.window_ms,
    nodes: new Map(s.nodes.map((n) => [n.id, n])),
    edges: new Map(s.edges.map((e) => [e.id, e])),
    pulses: [],
    older: s.older,
  }
}

/**
 * A snapshot reply. `unchanged` → `state` (with the reply's window, review U7); a reply older than `state` (earlier
 * epoch, or same epoch and lower seq: deltas applied while it was in flight) → `state` (review U3); else fromSnapshot(s).
 */
export function mergeSnapshot(state: GraphState | null, s: AgentGraph): GraphState {
  if (state && (s.unchanged || s.epoch_ms < state.epoch || (s.epoch_ms === state.epoch && s.seq < state.seq)))
    return state.window === s.window_ms || !s.unchanged ? state : { ...state, window: s.window_ms }
  return fromSnapshot(s)
}

/**
 * Review U3: deltas that arrived while a snapshot was in flight, applied after it. Those of the snapshot's epoch with a
 * higher seq go through applyDelta (a gap still asks for a re-snapshot); one of a later epoch asks for a re-snapshot;
 * older epochs are dropped.
 */
export function settleSnapshot(state: GraphState, buffered: readonly AgentGraphChanged[]): { state: GraphState; resnapshot: boolean } {
  let cur = state
  let resnapshot = false
  for (const ev of buffered) {
    if (ev.epoch_ms > cur.epoch) resnapshot = true
    if (ev.epoch_ms !== cur.epoch || ev.seq <= cur.seq) continue
    const out = applyDelta(cur, ev)
    if (out.state) cur = out.state
    resnapshot ||= out.resnapshot
  }
  return { state: cur, resnapshot }
}

/** One delta in order; `resnapshot` asks the caller for a full `agent_graph` (no state, new epoch, gap or resync). */
export function applyDelta(state: GraphState | null, ev: AgentGraphChanged): { state: GraphState | null; resnapshot: boolean } {
  if (!state) return { state: null, resnapshot: true }
  if (ev.epoch_ms !== state.epoch) return { state, resnapshot: true }
  if (ev.seq <= state.seq) return { state, resnapshot: false }
  if (ev.resync || ev.seq > state.seq + 1) return { state, resnapshot: true }
  const nodes = new Map(state.nodes)
  const edges = new Map(state.edges)
  if (ev.removed.length > 0) {
    const gone = new Set(ev.removed)
    for (const id of gone) nodes.delete(id)
    for (const [id, e] of edges) if (gone.has(e.a) || gone.has(e.b)) edges.delete(id)
  }
  for (const n of ev.nodes) nodes.set(n.id, n)
  for (const id of ev.edges_removed) edges.delete(id)
  for (const e of ev.edges) edges.set(e.id, e)
  const pulses = ev.pulses.length > 0 ? [...state.pulses, ...ev.pulses].slice(-PULSE_KEEP) : state.pulses
  return { state: { ...state, nodes, edges, pulses, older: ev.older, seq: ev.seq, at: ev.at_ms }, resnapshot: false }
}

export function cursorOf(state: GraphState | null): GraphCursor | null {
  return state ? { epoch_ms: state.epoch, seq: state.seq } : null
}
