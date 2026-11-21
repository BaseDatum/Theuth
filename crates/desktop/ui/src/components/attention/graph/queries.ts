// 208 §8.5: the graph's data — one `agent_graph` snapshot, then `agent.graph@1` deltas through the pure reducer.
// Re-snapshots only on a gap, a new epoch, a resync or a reconnect; a cursor renewal every 120 s keeps the engine's
// lease alive. The dev fixture page swaps the source (`setGraphSource`) and the clock (`clock.ts` `setGraphClock`).
import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react'
import type { AgentGraphChanged } from '@/rpc/bindings/event.agent_2egraph.output'
import type { GraphCursor } from '@/rpc/bindings/method.agent.input'
import type { AgentGraph } from '@/rpc/bindings/method.agent.output'
import { useSupported } from '@/rpc/capabilities'
import { graphRequirements } from '../requirements'
import { useConnection } from '@/store/connection'
import { onTopic } from '@/store/sessions'
import { noteEngineTime } from './clock'
import { retryDelay, streamRuns } from './lifecycle'
import { applyDelta, cursorOf, type GraphState, mergeSnapshot, settleSnapshot } from './reducer'
import { type TtlPrefs, windowMs } from './ttl'

export type GraphSource = {
  snapshot: (windowMs: number, include: readonly string[], cursor: GraphCursor | null) => Promise<AgentGraph>
  subscribe: (cb: (ev: AgentGraphChanged) => void) => () => void
}

const RENEW_MS = 120_000
const INCLUDE_DEBOUNCE_MS = 1000
/** Deltas held while a snapshot is in flight; past it the next snapshot is a full one. */
const BUFFER_CAP = 500

let override: GraphSource | null = null

/** Dev fixture only: replace the RPC source (null restores it). */
export function setGraphSource(s: GraphSource | null): void {
  override = s
}

const subscribeVisibility = (cb: () => void) => {
  document.addEventListener('visibilitychange', cb)
  return () => document.removeEventListener('visibilitychange', cb)
}

/**
 * The graph state for `ttl`'s window, with `include` sessions shown regardless of age (contract §8.5). While the caller
 * is mounted and the document visible (`streamRuns`, review U4) it holds the stream, renews the lease every 120 s and
 * retries failed calls with backoff; collapsed or not, so the strip's counts stay live. Hidden: everything pauses and
 * the engine's lease runs out; visible again starts from a full snapshot. One caller (`GraphRegion`) owns the stream.
 */
export function useAgentGraph(ttl: TtlPrefs, include: readonly string[]): { state: GraphState | null; supported: boolean; error: string | null } {
  const engineSupports = useSupported(graphRequirements)
  const api = useConnection((s) => s.api)
  const conn = useConnection((s) => s.state)
  const visible = useSyncExternalStore(subscribeVisibility, () => document.visibilityState !== 'hidden', () => true)
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
  const enabled = streamRuns({ supported, source: source !== null, visible })
  const [state, setState] = useState<GraphState | null>(null)
  const [error, setError] = useState<string | null>(null)
  const window = windowMs(ttl)
  const args = useRef({ window, include })
  args.current = { window, include }
  // The 207 `patchEvent` rule: one call in flight; asks meanwhile collapse into one follow-up (full wins over cursor).
  // `gen` counts activations: a reply to a call from an earlier one is dropped, and no call starts while inactive.
  // `buffer` holds deltas that arrived while a call was in flight (review U3).
  const run = useRef({
    state: null as GraphState | null,
    pending: false,
    again: null as null | 'full' | 'cursor',
    on: false,
    gen: 0,
    buffer: [] as AgentGraphChanged[],
    failures: 0,
    cancelRetry: null as null | (() => void),
  })
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
        noteEngineTime(s.at_ms)
        const merged = mergeSnapshot(cursor ? r.state : null, s)
        // A replaced state may serve a narrower window than was asked: ask again if still needed (review U7).
        if (merged !== r.state && !s.unchanged) asked.current.window = 0
        const out = settleSnapshot(merged, r.buffer)
        r.buffer = []
        r.state = out.state
        setState(out.state)
        setError(null)
        r.failures = 0
        if (out.resnapshot) r.again = 'full'
      })
      .catch((e: unknown) => {
        if (gen !== r.gen) return
        setError(e instanceof Error ? e.message : String(e))
        // The base did not change: the deltas buffered meanwhile apply to the state as it is.
        if (r.state) {
          const out = settleSnapshot(r.state, r.buffer)
          r.state = out.state
          setState(out.state)
          if (out.resnapshot) r.again = 'full'
        }
        r.buffer = []
        // Review U4: retry after 5 s, 15 s, then every 60 s, instead of waiting for the next renewal.
        r.failures += 1
        r.cancelRetry?.()
        const t = setTimeout(() => {
          r.cancelRetry = null
          snapshot.current(full || r.state === null)
        }, retryDelay(r.failures))
        r.cancelRetry = () => clearTimeout(t)
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
      noteEngineTime(ev.at_ms)
      // Review U3: while a snapshot is in flight its reply decides the base; the delta is applied after it.
      if (r.pending) {
        if (r.buffer.length < BUFFER_CAP) r.buffer.push(ev)
        else r.again = 'full'
        return
      }
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
      r.cancelRetry?.()
      r.cancelRetry = null
      a.cancel?.()
      a.cancel = null
      r.on = false
      r.gen += 1
      r.again = null
      r.buffer = []
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
