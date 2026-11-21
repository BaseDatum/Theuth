import { useQuerySupport } from '@/rpc/capabilities'
import { useQuery } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { Hourglass, TriangleAlert } from 'lucide-react'
import { Fragment, type ReactNode, memo, useEffect, useMemo, useState } from 'react'
import { counts } from '@/components/chat/broadcast'
import { EyeMark, SourceTag, WatchCostCell, WatchDot } from '@/components/chat/ObservedBar'
import { useSessionDrag } from '@/components/chat/Panes'
import { costShort, costTitle } from '@/lib/money'
import { queryKeys, useApi } from '@/lib/rpc'
import { modelLabel } from '@/components/agents/sessions'
import { cappedTree, childrenOf, live, observedLabel, withAncestors } from '@/lib/session-tree'
import { backgroundColors } from '@/lib/background-colors'
import { cn } from '@/lib/utils'
import type { CostBucket } from "@/rpc/bindings/method.agent.output"
import type { SessionSummary } from "@/rpc/bindings/method.agent.output"
import type { AgentInfo } from '@/store/ui-events'
import type { AgentState } from "@/rpc/bindings/method.agent.output"
import { useSessionField, useSessions } from '@/store/sessions'
import { SidebarPopoutButton } from './SidebarPopoutButton'
import { HarnessGlyph, shortModel } from './harnessGlyph'

export const GLYPH: Record<AgentState['state'], ReactNode> = {
  running: '●',
  idle: '○',
  awaiting: <Hourglass className="inline size-3" />,
  approval: <TriangleAlert className="inline size-3 text-warning" />,
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

/** One subtree row: harness glyph · name · cap badge · cost · jobs · model · age, a link to the session's chat. Shared with
 * the Agents page (100). The glyph says which harness runs the child (`harnessGlyph`; theuth's own is the bot), tinted
 * while it runs, warns on approval and errors, muted otherwise; its title is `<harness> · <model>`. The short model sits
 * right, muted, and is the first thing to give way: it truncates before the name and hides under the hover pop-out button.
 * A watch-only child (141 §9.2a) leads with its harness-state dot and an eye, carries its source tag after the name, and
 * greys out while resting (`idle`, `pending`, `unknown` — Q1: idle is not done); its cost cell follows the cost rule. */
export const SubagentRow = memo(function SubagentRow({
  row,
  active,
  now,
  cost,
  popout = false,
}: {
  row: AgentInfo
  active: boolean
  now: number
  /** The child's inclusive total from the root's `session_cost` (51 §6.5); unknown for deeper rows. */
  cost: CostBucket | undefined
  /** Sidebar-only hover chrome; the Agents page keeps its existing row layout. */
  popout?: boolean
}) {
  const drag = useSessionDrag(row.id, row.name)
  const observed = row.observed ?? undefined
  const resting = observed && (observed.state === 'idle' || observed.state === 'pending' || observed.state === 'unknown')
  const content = (
    <Link
      to="/c/$session"
      params={{ session: row.id }}
      {...drag}
      className={cn(
        'flex h-7 items-center gap-1.5 rounded-md pr-2 text-xs text-fg-secondary hover:bg-sidebar-accent',
        active && 'bg-sidebar-accent font-medium text-foreground',
        resting && 'opacity-60',
        popout && 'pointer-fine:pr-8',
      )}
      style={{ paddingLeft: 8 + row.depth * 12 }}
      title={
        observed
          ? `${row.name} · watching · ${observedLabel[observed.state]} · ${modelLabel(row)}`
          : row.state.state === 'awaiting'
            ? `awaiting ${row.state.from}`
            : row.state.state === 'approval'
              ? `approval: ${row.state.tool}`
              : row.state.state
      }
    >
      {observed ? (
        <>
          <WatchDot state={observed.state} beat={row.beat} className="w-4" />
          <EyeMark />
        </>
      ) : (
        <span
          className={cn(
            'flex w-4 shrink-0 justify-center',
            row.state.state === 'running'
              ? cn(backgroundColors.subagents.text, 'animate-pulse')
              : row.state.state === 'approval' || row.state.state === 'awaiting'
                ? backgroundColors.approval.text
                : row.state.state === 'error'
                  ? 'text-danger'
                  : 'text-fg-tertiary',
          )}
          title={`${row.agent || 'theuth'} · ${row.model || 'model unknown'}`}
        >
          <HarnessGlyph agent={row.agent} className="size-3.5" />
        </span>
      )}
      <span className="min-w-0 flex-1 truncate">{row.name}</span>
      {observed && <SourceTag observed={observed} />}
      {row.cap && (
        <span className="rounded-sm bg-sidebar-accent px-1 text-[10px] text-fg-tertiary" title={`tools capped at ${row.cap}`}>
          {row.cap === 'read_only' ? 'read-only' : row.cap}
        </span>
      )}
      {observed ? (
        // 141 R3 T9: the child's own figure under its cost rule (the root's `session_cost` only lists direct children).
        <WatchCostCell session={row.id} observed={observed} />
      ) : (
        cost &&
        costShort(cost) !== '—' && (
          <span className="whitespace-nowrap tabular-nums text-fg-tertiary" title={costTitle(cost)}>
            {costShort(cost)}
          </span>
        )
      )}
      {row.activity && (row.activity.jobs > 0 || row.activity.ptys > 0) && (
        <span className="tabular-nums text-fg-tertiary" title="running jobs · PTYs">
          {counts(row.activity)}
        </span>
      )}
      {row.model && (
        <span className={cn('min-w-0 max-w-28 shrink truncate text-[11px] text-fg-tertiary', popout && 'pointer-fine:group-hover/popout-row:hidden')}>
          {shortModel(row.model)}
        </span>
      )}
      <span className="shrink-0 tabular-nums text-fg-tertiary">{ago(row.activity?.last_active ?? row.started_at ?? row.finished_at, now)}</span>
    </Link>
  )
  return popout ? (
    <div className="group/popout-row relative">
      {content}
      <SidebarPopoutButton target={{ kind: 'chat', session: row.id }} className="top-1 right-1" />
    </div>
  ) : content
})

/**
 * One parent's children under the one subagent-list rule (`cappedTree`): ranked live-first / most-recent, `TREE_CAP`
 * at a time behind a `Show more (N)` row (`Show less` once open — component state, never persisted), the `keep` rows
 * (the open chat and its ancestors) always shown. Each row renders through `render(row, sub)` where `sub` is the
 * row's own `Branch`; the fold button takes `moreClass` and indents like a row of that depth (`unit` px per level).
 */
export function Branch({
  parent,
  byParent,
  sessions,
  keep,
  unit,
  moreClass,
  render,
}: {
  parent: string
  byParent: ReadonlyMap<string, readonly AgentInfo[]>
  sessions: readonly SessionSummary[]
  keep?: ReadonlySet<string>
  unit: number
  moreClass: string
  render(row: AgentInfo, sub: ReactNode): ReactNode
}) {
  const [expanded, setExpanded] = useState(false)
  const rows = byParent.get(parent)
  if (!rows?.length) return null
  const { shown, hidden } = cappedTree(rows, sessions, expanded, keep)
  return (
    <>
      {shown.map((r) => (
        <Fragment key={r.id}>
          {render(r, <Branch parent={r.id} byParent={byParent} sessions={sessions} keep={keep} unit={unit} moreClass={moreClass} render={render} />)}
        </Fragment>
      ))}
      {(hidden > 0 || expanded) && (
        <button type="button" className={moreClass} style={{ paddingLeft: 8 + (rows[0]?.depth ?? 1) * unit }} onClick={() => setExpanded((v) => !v)}>
          {expanded ? 'Show less' : `Show more (${hidden})`}
        </button>
      )}
    </>
  )
}

/**
 * A session's subtree: seeded from `list_agents` when it appears, live from events after; each parent's children as
 * a `Branch` (ranked and capped on their own, the open chat's path always visible). `liveOnly` keeps just the `live`
 * rows and their ancestors — a desk that is not the open chat shows what is working, not its history.
 */
export function AgentTree({ session, active, liveOnly = false }: { session: string; active: string | undefined; liveOnly?: boolean }) {
  const canQuery1 = useQuerySupport(["list_sessions"])
  const canQuery1_2 = useQuerySupport(["session_cost"])
  const api = useApi()
  const children = useSessionField(session, 'children')
  // The sidebar keeps `list_sessions` warm; it lends `updated_at`/`created_at` to the ranking.
  const { data: sessions = [] } = useQuery({ enabled: canQuery1, queryKey: queryKeys.sessions, queryFn: () => api.list_sessions() })
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
  const { byParent, keep, some } = useMemo(() => {
    const rows = children ?? {}
    const all = Object.values(rows)
    const wanted = liveOnly ? withAncestors(rows, all.filter((r) => live(r.state.state)).map((r) => r.id)) : null
    const kept = wanted ? all.filter((r) => wanted.has(r.id)) : all
    return { byParent: childrenOf(kept), keep: withAncestors(rows, active ? [active] : []), some: kept.length > 0 }
  }, [children, liveOnly, active])
  const { data: cost } = useQuery({
    queryKey: queryKeys.sessionCost(session),
    queryFn: () => api.session_cost(session),
    enabled: canQuery1_2 && (some),
  })
  if (!some) return null
  return (
    <div className="mx-1 mb-1 flex flex-col">
      <Branch
        parent={session}
        byParent={byParent}
        sessions={sessions}
        keep={keep}
        unit={12}
        moreClass="flex h-7 items-center gap-1.5 rounded-md pr-2 text-xs text-fg-tertiary hover:bg-sidebar-accent"
        render={(r, sub) => (
          <>
            <SubagentRow row={r} active={r.id === active} now={now} cost={cost?.children.find((c) => c.session === r.id)?.total} popout />
            {sub}
          </>
        )}
      />
    </div>
  )
}
