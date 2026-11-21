// 208 §8.5: the graph's data — one `agent_graph` snapshot, then `agent.graph@1` deltas through the pure reducer.
// Re-snapshots only on a gap, a new epoch, a resync or a reconnect; a cursor renewal every 120 s keeps the engine's
// lease alive. The dev fixture page swaps the source and the clock (`setGraphSource`, `setGraphClock`).
import { useEffect, useMemo, useRef, useState } from 'react'
import type { AgentGraphChanged } from '@/rpc/bindings/event.agent_2egraph.output'
import type { GraphCursor } from '@/rpc/bindings/method.agent.input'
import type { AgentGraph } from '@/rpc/bindings/method.agent.output'
import { useSupported } from '@/rpc/capabilities'
import { graphRequirements } from '../requirements'
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

/**
 * The graph state for `ttl`'s window, with `include` sessions shown regardless of age (contract §8.5). Only while
 * `active` (the region expanded or full screen) does it hold the stream and renew the lease; inactive it keeps the last
 * state and makes no calls, so the engine's graph actor goes dormant 300 s later. Re-activation starts from a full
 * snapshot (the stream had gaps meanwhile).
 */
export function useAgentGraph(ttl: TtlPrefs, include: readonly string[], active: boolean): { state: GraphState | null; supported: boolean; error: string | null } {
  const engineSupports = useSupported(graphRequirements)
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
  const enabled = supported && source !== null && active
  const [state, setState] = useState<GraphState | null>(null)
  const [error, setError] = useState<string | null>(null)
  const window = windowMs(ttl)
  const args = useRef({ window, include })
  args.current = { window, include }
  // The 207 `patchEvent` rule: one call in flight; asks meanwhile collapse into one follow-up (full wins over cursor).
  // `gen` counts activations: a reply to a call from an earlier one is dropped, and no call starts while inactive.
  const run = useRef({ state: null as GraphState | null, pending: false, again: null as null | 'full' | 'cursor', on: false, gen: 0 })
  const snapshot = useRef<(full: boolean) => void>(() => undefined)
  snapshot.current = (full: boolean) => {
    const r = run.current
    if (!source || !r.on) return
    if (r.pending) {
      r.again = full || r.again === 'full' ? 'full' : 'cursor'
      return
    }
    r.pending = true
    const gen = r.gen
    const cursor = full ? null : cursorOf(r.state)
    source
      .snapshot(args.current.window, args.current.include, cursor)
      .then((s) => {
        if (gen !== r.gen) return
        const next = mergeSnapshot(cursor ? r.state : null, s)
        r.state = next
        setState(next)
        setError(null)
      })
      .catch((e: unknown) => {
        if (gen === r.gen) setError(e instanceof Error ? e.message : String(e))
      })
      .finally(() => {
        r.pending = false
        const again = r.again
        r.again = null
        if (again) snapshot.current(again === 'full')
      })
  }

  // Activation (and a source change): a full snapshot, the delta stream, the renewal; all released on deactivation.
  const asked = useRef({ ids: new Set<string>(), window: 0, cancel: null as null | (() => void) })
  useEffect(() => {
    if (!enabled || !source) return
    const r = run.current
    r.on = true
    r.gen += 1
    r.state = null
    // A call from the previous activation may still be in flight: its reply is dropped and this full one follows it.
    if (r.pending) r.again = 'full'
    else snapshot.current(true)
    const off = source.subscribe((ev) => {
      const out = applyDelta(r.state, ev)
      if (out.state !== r.state) {
        r.state = out.state
        setState(out.state)
      }
      if (out.resnapshot) snapshot.current(true)
    })
    const renew = setInterval(() => snapshot.current(false), RENEW_MS)
    const a = asked.current
    return () => {
      off()
      clearInterval(renew)
      a.cancel?.()
      a.cancel = null
      r.on = false
      r.gen += 1
      r.again = null
    }
  }, [enabled, source])

  // A wider TTL window or a session the graph does not know yet: ask again with the cursor, 1 s after the first need
  // (not re-armed by later deltas, so a busy stream cannot starve it), once per id.
  const includeKey = include.join('\n')
  useEffect(() => {
    const a = asked.current
    if (!enabled || !run.current.state || a.cancel) return
    const need = () => {
      const st = run.current.state
      if (!st) return null
      const fresh = args.current.include.filter((s) => !st.nodes.has(s) && !a.ids.has(s))
      const w = args.current.window
      return fresh.length > 0 || (st.window < w && a.window < w) ? { fresh, w } : null
    }
    if (!need()) return
    const t = setTimeout(() => {
      a.cancel = null
      const n = need()
      if (!n) return
      for (const s of n.fresh) a.ids.add(s)
      a.window = Math.max(a.window, n.w)
      snapshot.current(false)
    }, INCLUDE_DEBOUNCE_MS)
    a.cancel = () => clearTimeout(t)
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
