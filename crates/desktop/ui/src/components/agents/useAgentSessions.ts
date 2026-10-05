import { useEffect, useMemo, useState } from 'react'
import { useApi } from '@/lib/rpc'
import type { AgentInfo } from '@/rpc/extra'
import { useSessionField, useSessions } from '@/store/sessions'
import { LINGER_MS, type Lingering, NO_LINGERING, linger, liveRows } from './sessions'

const NO_ROWS: Readonly<Record<string, AgentInfo>> = {}

/**
 * An agent's live sessions (100 D1): the desk's subtree, seeded from `list_agents(desk)` once and kept live by the
 * store reducer like the sidebar's `AgentTree`; `live` is the running/awaiting/approval rows in spawn order,
 * `deskRunning` the desk's own turn bit. Everything is empty without a desk.
 */
export function useAgentSessions(desk: string | null): { rows: Readonly<Record<string, AgentInfo>>; live: AgentInfo[]; deskRunning: boolean } {
  const api = useApi()
  const children = useSessionField(desk ?? '', 'children')
  const running = useSessionField(desk ?? '', 'running')
  useEffect(() => {
    if (!desk) return
    api.list_agents(desk).then(
      (a) => useSessions.getState().setChildren(desk, a),
      () => {},
    )
  }, [api, desk])
  const rows = desk ? (children ?? NO_ROWS) : NO_ROWS
  const live = useMemo(() => liveRows(rows), [rows])
  return { rows, live, deskRunning: desk !== null && running === true }
}

/** Drives `linger` (100 D2): re-reduced when `liveIds` changes, and once more when the earliest leaving row is due. */
export function useLingering(liveIds: readonly string[]): Lingering {
  const [state, setState] = useState(NO_LINGERING)
  useEffect(() => {
    setState((prev) => linger(prev, liveIds, Date.now()))
  }, [liveIds])
  useEffect(() => {
    const lefts = Object.values(state.leaving)
    if (lefts.length === 0) return
    const timer = window.setTimeout(
      () => setState((prev) => linger(prev, liveIds, Date.now())),
      Math.max(0, Math.min(...lefts) + LINGER_MS - Date.now()),
    )
    return () => window.clearTimeout(timer)
  }, [state, liveIds])
  return state
}
