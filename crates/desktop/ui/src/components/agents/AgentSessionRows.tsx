import { useQuerySupport } from '@/rpc/capabilities'
import { useQuery } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { useEffect, useMemo, useState } from 'react'
import { Branch, GLYPH, SubagentRow, ago } from '@/components/sidebar/AgentTree'
import { useSidebarIsMobile } from '@/components/ui/sidebar'
import { queryKeys, useApi } from '@/lib/rpc'
import { childrenOf, withAncestors } from '@/lib/session-tree'
import { cn } from '@/lib/utils'
import { useAgentSessions, useLingering } from './useAgentSessions'

const MORE_CLASS = 'flex h-7 items-center gap-1.5 rounded-md pr-2 text-xs text-fg-tertiary hover:bg-sidebar-accent'

/**
 * An agent's live sessions under its list row (100 D3): the desk line, then the live descendants with their
 * ancestors through the sidebar's `Branch`/`SubagentRow`. Rows that stop being live linger `LINGER_MS` fading out.
 * Nothing (no `<tr>`) while the desk is idle and the subtree quiet. Phones get 44 px rows.
 */
export function AgentSessionRows({ desk, depth }: { desk: string; depth: number }) {
  const canQuery1 = useQuerySupport(["list_sessions"])
  const api = useApi()
  const mobile = useSidebarIsMobile()
  const { rows, live, deskRunning } = useAgentSessions(desk)
  const lingering = useLingering(useMemo(() => live.map((r) => r.id), [live]))
  const { data: sessions = [] } = useQuery({ enabled: canQuery1, queryKey: queryKeys.sessions, queryFn: () => api.list_sessions() })
  const [now, setNow] = useState(Date.now())
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1_000)
    return () => window.clearInterval(timer)
  }, [])
  const byParent = useMemo(() => {
    const wanted = withAncestors(rows, lingering.ids)
    return childrenOf(Object.values(rows).filter((r) => wanted.has(r.id)))
  }, [rows, lingering])
  if (!deskRunning && lingering.ids.length === 0) return null
  const deskAt = sessions.find((s) => s.id === desk)?.updated_at ?? null
  return (
    <tr className="border-0">
      <td colSpan={6} className="px-3 pt-0 pb-2">
        <div className={cn('flex max-w-sm flex-col', mobile && '[&_a]:min-h-11 [&_button]:min-h-11')} style={{ paddingLeft: depth * 18 + 36 }}>
          <Link to="/c/$session" params={{ session: desk }} className="flex h-7 items-center gap-1.5 rounded-md pr-2 text-xs text-fg-secondary hover:bg-sidebar-accent" style={{ paddingLeft: 8 }} title={deskRunning ? 'desk · running' : 'desk · idle'}>
            <span className={cn('w-4 text-center', deskRunning && 'animate-pulse')}>{deskRunning ? GLYPH.running : GLYPH.idle}</span>
            <span className="flex-1 truncate">desk</span>
            <span className="tabular-nums text-fg-tertiary">{ago(deskAt, now)}</span>
          </Link>
          <Branch
            parent={desk}
            byParent={byParent}
            sessions={sessions}
            unit={12}
            moreClass={MORE_CLASS}
            render={(r, sub) => (
              <>
                {/* An ancestor of a still-live row stays put even after its own turn ended. */}
                <div className={cn('subagent-dot-in', r.id in lingering.leaving && !byParent.has(r.id) && 'opacity-0 transition-opacity duration-700')}>
                  <SubagentRow row={r} active={false} now={now} cost={undefined} />
                </div>
                {sub}
              </>
            )}
          />
        </div>
      </td>
    </tr>
  )
}
