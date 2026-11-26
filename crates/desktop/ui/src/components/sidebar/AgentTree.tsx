import { useQuerySupport } from '@/rpc/capabilities'
import { useQuery } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { Hourglass, TriangleAlert } from 'lucide-react'
import { Fragment, type ReactNode, memo, useEffect, useMemo, useRef, useState } from 'react'
import { counts } from '@/components/chat/broadcast'
import { EyeMark, SourceTag, WatchCostCell, WatchDot } from '@/components/chat/ObservedBar'
import { useSessionDrag } from '@/components/chat/Panes'
import { costShort, costTitle } from '@/lib/money'
import { queryKeys, useApi } from '@/lib/rpc'
import { modelLabel } from '@/components/agents/sessions'
import { cappedTree, childrenOf, live, notAdvisor, observedLabel, withAncestors } from '@/lib/session-tree'
import { backgroundColors } from '@/lib/background-colors'
import { cn } from '@/lib/utils'
import type { CostBucket } from "@/rpc/bindings/method.agent.output"
import type { SessionSummary } from "@/rpc/bindings/method.agent.output"
import type { AgentInfo } from '@/store/ui-events'
import type { AgentState } from "@/rpc/bindings/method.agent.output"
import { useSessionField, useSessions } from '@/store/sessions'
import { SidebarPopoutButton } from './SidebarPopoutButton'
import type { Ticks } from '@/lib/selection'
import { HarnessGlyph, shortModel } from './harnessGlyph'
import { MachineChip } from './SessionItem'

export const GLYPH: Record<AgentState['state'], ReactNode> = {
  running: '●',
  idle: '○',
  awaiting: <Hourglass className="inline size-3" />,
  approval: <TriangleAlert className="inline size-3 text-warning" />,
  error: '✗',
  cancelled: '■',
}

/** The time a row's `ago` counts from (what `SubagentRow` shows). */
const agoFrom = (row: { activity?: { last_active?: string | null } | null; started_at?: string | null; finished_at?: string | null }) =>
  row.activity?.last_active ?? row.started_at ?? row.finished_at ?? null

/**
 * How often a tree's `ago` labels must re-render: `null` with no rows, every second while one reads seconds (under a
 * minute), else every 30 s (minutes and up move no faster).
 */
export function agoStep(rows: Record<string, Parameters<typeof agoFrom>[0]> | undefined, now: number, some: boolean): number | null {
  if (!some || !rows) return null
  const seconds = Object.values(rows).some((r) => {
    const at = agoFrom(r)
    return at !== null && now - Date.parse(at) < 60_000
  })
  return seconds ? 1_000 : 30_000
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

/** The sidebar's state colour (shared with the Attention graph, 208 §8.5): running pulsing in malachite, approval and
 * awaiting in the approval tone, errors in danger, everything else muted. */
export function stateTone(state: AgentState): string {
  return state.state === 'running'
    ? 'animate-pulse text-success'
    : state.state === 'approval' || state.state === 'awaiting'
      ? backgroundColors.approval.text
      : state.state === 'error'
        ? 'text-danger'
        : 'text-fg-tertiary'
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
  ticked = false,
  tick,
  machine,
}: {
  row: AgentInfo
  active: boolean
  now: number
  /** The child's inclusive total from the root's `session_cost` (51 §6.5); unknown for deeper rows. */
  cost: CostBucket | undefined
  /** Sidebar-only hover chrome; the Agents page keeps its existing row layout. */
  popout?: boolean
  /** Tick mode (`Ticks`, the broadcast picker): a checkbox leads the row and a click ticks it instead of opening. */
  ticked?: boolean
  tick?(id: string): void
  /** Sidebar-only: the session's engine for the machine badge (`MachineChip`); the Agents page passes none. */
  machine?: { owner: string; epoch: number; here: boolean }
}) {
  const drag = useSessionDrag(row.id, row.name)
  const observed = row.observed ?? undefined
  const resting = observed && (observed.state === 'idle' || observed.state === 'pending' || observed.state === 'unknown')
  const look = {
    className: cn(
      'flex h-7 items-center gap-1.5 rounded-md pr-2 text-xs text-fg-secondary hover:bg-sidebar-accent',
      active && 'bg-sidebar-accent font-medium text-foreground',
      resting && 'opacity-60',
      popout && 'pointer-fine:pr-8',
      tick && 'cursor-pointer',
    ),
    style: { paddingLeft: 8 + row.depth * 12 },
    title: observed
      ? `${row.name} · watching · ${observedLabel[observed.state]} · ${modelLabel(row)}`
      : row.state.state === 'awaiting'
        ? `awaiting ${row.state.from}`
        : row.state.state === 'approval'
          ? `approval: ${row.state.tool}`
          : row.state.state,
  }
  const body = (
    <>
      {observed ? (
        <>
          <WatchDot state={observed.state} beat={row.beat} className="w-4" />
          <EyeMark />
        </>
      ) : (
        <span
          className={cn('flex w-4 shrink-0 justify-center', stateTone(row.state))}
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
      {machine && <MachineChip {...machine} />}
      <span className="shrink-0 tabular-nums text-fg-tertiary">{ago(agoFrom(row), now)}</span>
    </>
  )
  if (tick)
    return (
      <label {...look}>
        <input type="checkbox" className="size-3.5 shrink-0 accent-foreground" checked={ticked} onChange={() => tick(row.id)} />
        {body}
      </label>
    )
  const content = (
    <Link to="/c/$session" params={{ session: row.id }} {...drag} {...look}>
      {body}
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
 * What of `root`'s descendants the replicated `list_sessions` says changed: ids with their running / watch-only state,
 * sorted. A remote session's spawns arrive by replication only when its stream misses them, so the tree refetches on it.
 */
export function descendantKey(sessions: readonly SessionSummary[], root: string): string {
  const byParent = new Map<string, SessionSummary[]>()
  for (const s of sessions) {
    if (s.parent === null || !notAdvisor(s)) continue
    const list = byParent.get(s.parent)
    if (list) list.push(s)
    else byParent.set(s.parent, [s])
  }
  const out: string[] = []
  const seen = new Set([root])
  const queue = [root]
  for (let id = queue.pop(); id !== undefined; id = queue.pop()) {
    for (const s of byParent.get(id) ?? []) {
      if (seen.has(s.id)) continue
      seen.add(s.id)
      queue.push(s.id)
      out.push(`${s.id}:${s.running ? 1 : 0}:${s.observed?.state ?? ''}:${s.finished_at ?? ''}`)
    }
  }
  return out.sort().join('\n')
}

/** A `list_agents` snapshot over the tree's rows: its rows win, a row it lacks (a live spawn newer than it) stays. */
export function mergeChildren(current: Record<string, AgentInfo> | undefined, snapshot: AgentInfo[]): AgentInfo[] {
  const ids = new Set(snapshot.map((a) => a.id))
  return [...snapshot, ...Object.values(current ?? {}).filter((r) => !ids.has(r.id))]
}

/**
 * A session's subtree: seeded from `list_agents` when it appears, live from events after, and refetched (debounced)
 * when the replicated `list_sessions` changes the subtree — a peer's session streams only its own events, so a child
 * spawned there may reach this engine by replication alone. Each parent's children as a `Branch` (ranked and capped on
 * their own, the open chat's path always visible). `liveOnly` keeps just the `live` rows and their ancestors — a desk
 * that is not the open chat shows what is working, not its history. `tick`: every row a checkbox (the broadcast picker).
 */
export function AgentTree({ session, active, liveOnly = false, tick }: { session: string; active: string | undefined; liveOnly?: boolean; tick?: Ticks }) {
  const canQuery1 = useQuerySupport(["list_sessions"])
  const canQuery1_2 = useQuerySupport(["session_cost"])
  const api = useApi()
  const children = useSessionField(session, 'children')
  // The sidebar keeps `list_sessions` warm; it lends `updated_at`/`created_at` to the ranking.
  const { data: sessions = [] } = useQuery({ enabled: canQuery1, queryKey: queryKeys.sessions, queryFn: () => api.list_sessions() })
  const tree = useMemo(() => descendantKey(sessions, session), [sessions, session])
  // Each row's engine for its machine badge: its own `list_sessions` row, else the root's (a child runs where it was spawned).
  const byId = useMemo(() => new Map(sessions.map((s) => [s.id, s])), [sessions])
  // The last fetch: a new session seeds at once, a changed subtree refetches after 300 ms of quiet.
  const fetched = useRef<{ api: typeof api; session: string; tree: string } | null>(null)
  useEffect(() => {
    const last = fetched.current
    const fresh = last?.api !== api || last.session !== session
    if (!fresh && last?.tree === tree) return
    const load = () => {
      fetched.current = { api, session, tree }
      api.list_agents(session).then(
        (a) => {
          const st = useSessions.getState()
          st.setChildren(session, mergeChildren(st.sessions[session]?.children, a))
        },
        () => {},
      )
    }
    if (fresh) return load()
    const timer = window.setTimeout(load, 300)
    return () => window.clearTimeout(timer)
  }, [api, session, tree])
  const { byParent, keep, some } = useMemo(() => {
    const rows = children ?? {}
    const all = Object.values(rows)
    const wanted = liveOnly ? withAncestors(rows, all.filter((r) => live(r.state.state)).map((r) => r.id)) : null
    const kept = wanted ? all.filter((r) => wanted.has(r.id)) : all
    return { byParent: childrenOf(kept), keep: withAncestors(rows, active ? [active] : []), some: kept.length > 0 }
  }, [children, liveOnly, active])
  // The rows' `ago`: a tree per desk, per tree row and in the rail popover, so each clock re-renders its whole tree.
  // It runs only while rows show, by the second only while one of them reads seconds, never in a hidden window.
  const [now, setNow] = useState(Date.now())
  const step = agoStep(children, now, some)
  useEffect(() => {
    if (step === null) return
    const timer = window.setInterval(() => document.visibilityState !== 'hidden' && setNow(Date.now()), step)
    return () => window.clearInterval(timer)
  }, [step])
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
            <SubagentRow
              row={r}
              active={r.id === active}
              now={now}
              cost={cost?.children.find((c) => c.session === r.id)?.total}
              popout={!tick}
              ticked={tick?.ids.has(r.id)}
              tick={tick?.toggle}
              machine={byId.get(r.id) ?? byId.get(session)}
            />
            {sub}
          </>
        )}
      />
    </div>
  )
}
