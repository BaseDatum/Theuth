import { Link } from '@tanstack/react-router'
import { memo, useEffect, useState } from 'react'
import { useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { AgentInfo, AgentState } from '@/rpc/extra'
import { useSessionField, useSessions } from '@/store/sessions'

const GLYPH: Record<AgentState['state'], string> = {
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
}: {
  row: AgentInfo
  active: boolean
  now: number
}) {
  return (
    <Link
      to="/c/$session"
      params={{ session: row.id }}
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
      <span className="truncate">{row.name}</span>
      <span className="ml-auto tabular-nums text-fg-tertiary">
        {ago(row.started_at ?? row.finished_at, now)}
      </span>
    </Link>
  )
})

/** The active session's subtree: seeded from `list_agents` when it appears, live from events after; one indented row per subagent. */
export function AgentTree({ session, active }: { session: string; active: string | undefined }) {
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
  const rows = children ? Object.values(children) : []
  if (rows.length === 0) return null
  return (
    <div className="mx-1 mb-1 flex flex-col">
      {rows.map((r) => (
        <Row key={r.id} row={r} active={r.id === active} now={now} />
      ))}
    </div>
  )
}
