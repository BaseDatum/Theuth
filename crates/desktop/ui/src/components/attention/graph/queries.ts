// 208 §8.5: the graph's data — one `agent_graph` snapshot, then `agent.graph@1` deltas through the pure reducer.
// Re-snapshots only on a gap, a new epoch, a resync or a reconnect; a cursor renewal every 120 s keeps the engine's
// lease alive. The dev fixture page swaps the source and the clock (`setGraphSource`, `setGraphClock`).
import { useEffect, useMemo, useRef, useState } from 'react'
import type { AgentGraphChanged } from '@/rpc/bindings/event.agent_2egraph.output'
import type { AgentGraph, GraphCursor } from '@/rpc/bindings/method.agent.output'
import { useQuerySupport } from '@/rpc/capabilities'
import { useConnection } from '@/store/connection'
import { onTopic } from '@/store/sessions'
import { applyDelta, cursorOf, type GraphState, mergeSnapshot } from './reducer'
import { type TtlPrefs, windowMs } from './ttl'

export type GraphSource = {
  snapshot: (windowMs: number, include: readonly string[], cursor: GraphCursor | null) => Promise<AgentGraph>
  subscribe: (cb: (ev: AgentGraphChanged) => void) => () => void
}

const RENEW_MS = 120_000
const INCLUDE_DEBOUNCE_MS = 1000

let override: GraphSource | null = null
let clock: () => number = Date.now

/** Dev fixture only: replace the RPC source (null restores it). */
export function setGraphSource(s: GraphSource | null): void {
  override = s
}
/** Dev fixture only: the clock the region's TTL reads (null restores `Date.now`). */
export function setGraphClock(fn: (() => number) | null): void {
  clock = fn ?? Date.now
}
/** The region's `now`. */
export function graphNow(): number {
  return clock()
}

/** The graph state for `ttl`'s window, with `include` sessions shown regardless of age (contract §8.5). */
export function useAgentGraph(ttl: TtlPrefs, include: readonly string[]): { state: GraphState | null; supported: boolean; error: string | null } {
  const engineSupports = useQuerySupport(['agent_graph'])
  const api = useConnection((s) => s.api)
  const conn = useConnection((s) => s.state)
  const source = useMemo<GraphSource | null>(
    () =>
      override ??
      (api
        ? {
            snapshot: (w, inc, cursor) => api.agent_graph(w, [...inc], cursor),
            subscribe: (cb) => onTopic('agent.graph@1', cb),
          }
        : null),
    [api],
  )
  const supported = override !== null || engineSupports
  const enabled = supported && source !== null
  const [state, setState] = useState<GraphState | null>(null)
  const [error, setError] = useState<string | null>(null)
  const window = windowMs(ttl)
  const args = useRef({ window, include })
  args.current = { window, include }
  // The 207 `patchEvent` rule: one call in flight; asks meanwhile collapse into one follow-up (full wins over cursor).
  const run = useRef({ state: null as GraphState | null, pending: false, again: null as null | 'full' | 'cursor' })
  const snapshot = useRef<(full: boolean) => void>(() => undefined)
  snapshot.current = (full: boolean) => {
    const r = run.current
    if (!source) return
    if (r.pending) {
      r.again = full || r.again === 'full' ? 'full' : 'cursor'
      return
    }
    r.pending = true
    const cursor = full ? null : cursorOf(r.state)
    source
      .snapshot(args.current.window, args.current.include, cursor)
      .then((s) => {
        const next = mergeSnapshot(cursor ? r.state : null, s)
        r.state = next
        setState(next)
        setError(null)
      })
      .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => {
        r.pending = false
        const again = r.again
        r.again = null
        if (again) snapshot.current(again === 'full')
      })
  }

  // Mount (and a source change): a full snapshot, the delta stream, the renewal.
  useEffect(() => {
    if (!enabled || !source) return
    const r = run.current
    r.state = null
    snapshot.current(true)
    const off = source.subscribe((ev) => {
      const out = applyDelta(r.state, ev)
      if (out.state !== r.state) {
        r.state = out.state
        setState(out.state)
      }
      if (out.resnapshot) snapshot.current(true)
    })
    const renew = setInterval(() => snapshot.current(false), RENEW_MS)
    return () => {
      off()
      clearInterval(renew)
    }
  }, [enabled, source])

  // A wider TTL window or a session the graph does not know yet: ask again with the cursor (debounced), once per id.
  const asked = useRef({ ids: new Set<string>(), window: 0 })
  const includeKey = include.join('\n')
  useEffect(() => {
    if (!enabled) return
    const r = run.current
    if (!r.state) return
    const fresh = args.current.include.filter((s) => !r.state?.nodes.has(s) && !asked.current.ids.has(s))
    const wider = r.state.window < window && asked.current.window < window
    if (fresh.length === 0 && !wider) return
    const t = setTimeout(() => {
      for (const s of fresh) asked.current.ids.add(s)
      asked.current.window = Math.max(asked.current.window, window)
      snapshot.current(false)
    }, INCLUDE_DEBOUNCE_MS)
    return () => clearTimeout(t)
  }, [enabled, includeKey, window, state])

  // Back to connected after a drop: the stream may have gaps the seq check cannot see.
  const prevConn = useRef(conn)
  useEffect(() => {
    const was = prevConn.current
    prevConn.current = conn
    if (enabled && conn === 'open' && was !== 'open') snapshot.current(true)
  }, [conn, enabled])

  return { state, supported, error }
}
