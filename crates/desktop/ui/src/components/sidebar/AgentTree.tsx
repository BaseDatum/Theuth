import { useQuery } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { memo, useEffect, useState } from 'react'
import { counts } from '@/components/chat/broadcast'
import { useSessionDrag } from '@/components/chat/Panes'
import { costLines, costShort } from '@/lib/money'
import { queryKeys, useApi } from '@/lib/rpc'
import { cappedTree } from '@/lib/session-tree'
import { cn } from '@/lib/utils'
import type { CostBucket } from '@/rpc/bindings/CostBucket'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import type { AgentInfo, AgentState } from '@/rpc/extra'
import { useSessionField, useSessions } from '@/store/sessions'

export const GLYPH: Record<AgentState['state'], string> = {
  running: '●',
  idle: '○',
  awaiting: '⏳',
  approval: '⚠',
  error: '✗',
  cancelled: '■',
}

/** `3m`, `2h`, `5d` since `iso`; empty when unknown. */
export function ago(iso: string | null, now = Date.now()): string {
  if (!iso) return ''
  const s = Math.max(0, (now - Date.parse(iso)) / 1000)
  if (s < 60) return `${Math.floor(s)}s`
  if (s < 3600) return `${Math.floor(s / 60)}m`
  if (s < 86400) return `${Math.floor(s / 3600)}h`
  return `${Math.floor(s / 86400)}d`
}

const Row = memo(function Row({
  row,
  active,
  now,
  cost,
}: {
  row: AgentInfo
  active: boolean
  now: number
  /** The child's inclusive total from the root's `session_cost` (51 §6.5); unknown for deeper rows. */
  cost: CostBucket | undefined
}) {
  const drag = useSessionDrag(row.id, row.name)
  return (
    <Link
      to="/c/$session"
      params={{ session: row.id }}
      {...drag}
      className={cn(
        'flex h-7 items-center gap-1.5 rounded-md pr-2 text-xs text-fg-secondary hover:bg-sidebar-accent',
        active && 'bg-sidebar-accent font-medium text-foreground',
      )}
      style={{ paddingLeft: 8 + row.depth * 12 }}
      title={
        row.state.state === 'awaiting'
          ? `awaiting ${row.state.from}`
          : row.state.state === 'approval'
            ? `approval: ${row.state.tool}`
            : row.state.state
      }
    >
      <span className={cn('w-4 text-center', row.state.state === 'running' && 'animate-pulse')}>
        {GLYPH[row.state.state]}
      </span>
      <span className="flex-1 truncate">{row.name}</span>
      {row.cap && (
        <span className="rounded-sm bg-sidebar-accent px-1 text-[10px] text-fg-tertiary" title={`tools capped at ${row.cap}`}>
          {row.cap === 'read_only' ? 'read-only' : row.cap}
        </span>
      )}
      {cost && (cost.returned.length > 0 || cost.estimated_for_unavailable.length > 0) && (
        <span className="tabular-nums text-fg-tertiary" title={costLines(cost).secondary ?? costLines(cost).primary}>
          {costShort(cost)}
        </span>
      )}
      {row.activity && (row.activity.jobs > 0 || row.activity.ptys > 0) && (
        <span className="tabular-nums text-fg-tertiary" title="running jobs · PTYs">
          {counts(row.activity)}
        </span>
      )}
      <span className="tabular-nums text-fg-tertiary">{ago(row.activity?.last_active ?? row.started_at ?? row.finished_at, now)}</span>
    </Link>
  )
})

/** A subagent that still wants eyes: running, waiting on a peer, or blocked on an approval. Finished rows (idle, cancelled, error) are not. */
export const live = (state: AgentState['state']): boolean => state === 'running' || state === 'awaiting' || state === 'approval'

/**
 * A session's subtree: seeded from `list_agents` when it appears, live from events after; one indented row per
 * subagent. `liveOnly` keeps just the `live` rows — a desk that is not the open chat shows what is working, not its
 * history. With `cap` (the desk's rows from `list_sessions`, for `updated_at`) the tree shows `TREE_CAP` rows —
 * running first, newest next — and a `Show more` row for the rest; `Show less` folds it back. Nothing is persisted:
 * every mount starts folded.
 */
export function AgentTree({
  session,
  active,
  liveOnly = false,
  cap,
}: {
  session: string
  active: string | undefined
  liveOnly?: boolean
  cap?: readonly SessionSummary[]
}) {
  const [expanded, setExpanded] = useState(false)
  const api = useApi()
  const children = useSessionField(session, 'children')
  const [now, setNow] = useState(Date.now())
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1_000)
    return () => window.clearInterval(timer)
  }, [])
  useEffect(() => {
    api.list_agents(session).then(
      (a) => useSessions.getState().setChildren(session, a),
      () => {},
    )
  }, [api, session])
  const all = children ? Object.values(children).filter((r) => !liveOnly || live(r.state.state)) : []
  const { shown: rows, hidden } = cap ? cappedTree(all, cap, expanded) : { shown: all, hidden: 0 }
  const { data: cost } = useQuery({
    queryKey: queryKeys.sessionCost(session),
    queryFn: () => api.session_cost(session),
    enabled: rows.length > 0,
  })
  if (all.length === 0) return null
  return (
    <div className="mx-1 mb-1 flex flex-col">
      {rows.map((r) => (
        <Row key={r.id} row={r} active={r.id === active} now={now} cost={cost?.children.find((c) => c.session === r.id)?.total} />
      ))}
      {(hidden > 0 || expanded) && (
        <button
          type="button"
          className="flex h-7 items-center gap-1.5 rounded-md pr-2 pl-5 text-xs text-fg-tertiary hover:bg-sidebar-accent"
          onClick={() => setExpanded((v) => !v)}
        >
          {expanded ? 'Show less' : `Show ${hidden} more`}
        </button>
      )}
    </div>
  )
}
