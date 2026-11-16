import { Link, useNavigate } from '@tanstack/react-router'
import { List, Network, UserPlus } from 'lucide-react'
import { useState } from 'react'
import { SectionBand } from '@/components/nav/SectionBand'
import { SectionRow } from '@/components/nav/SectionRow'
import { THEUTH } from '@/components/tasks/helpers'
import { agentWhere, agentsCount, openOn, remoteAgents } from '@/components/tasks/home'
import { useAgents, useEngineAgents, useMesh, useTasks } from '@/components/tasks/queries'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { useSidebar } from '@/components/ui/sidebar'
import { Skeleton } from '@/components/ui/skeleton'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { cn } from '@/lib/utils'
import { HiringInbox } from './HiringInbox'
import { OrgChart } from './OrgChart'
import { ActiveSwitch, ActivityChip, AgentAvatar, BudgetBar, PausedChip, RemoteChip } from './identity'
import { proposed } from './hiring'
import { AgentSessionRows } from './AgentSessionRows'
import { MemoryBadge } from './MemoryBadge'
import { type Node, type NodeInfo, tree } from './tree'

const VIEW_KEY = 'theuth.agents-view'
type View = 'org' | 'list'

/**
 * `/agents` (67 §12, paperclip's Agents page): the roster as an org chart by `reports_to` rooted at the built-in
 * `theuth`, or as a list; "Hire an agent" leads to the wizard; proposed agents wait in the Hiring inbox on top.
 */
export function AgentsView() {
  const sidebar = useSidebar()
  const agents = useAgents()
  // 182: one row per (agent, engine) — this engine's agents above, the other engines' replicated rows after them.
  const mesh = useMesh()
  const roster = useEngineAgents()
  const tasks = useTasks()
  const remote = remoteAgents(roster.data ?? [], mesh.data, tasks.data ?? [])
  const [view, setView] = useState<View>(() => (localStorage.getItem(VIEW_KEY) === 'list' ? 'list' : 'org'))
  const list = agents.data ?? []
  // Proposed agents are hires awaiting approval: they wait in the inbox, not on the chart.
  const root = tree(list.filter((a) => a.status !== 'proposed' && a.file.proposed_by === null), remote, mesh.data?.self_name ?? null)
  // One row/card per (agent, engine): the count and "where" are per node, the open count by task home.
  const info = (n: Node): NodeInfo => ({
    open: n.remote ? n.remote.open : tasks.data && mesh.data ? openOn(tasks.data, mesh.data, mesh.data.self_id, n.name) : undefined,
    where: n.remote ? n.remote.subtitle : agentWhere(n.name, roster.data ?? [], mesh.data),
  })
  const machines = new Set([...(list.length > 0 ? [mesh.data?.self_id] : []), ...remote.map((r) => r.engine)]).size
  return (
    <article className="flex min-h-0 min-w-0 flex-1 flex-col">
      <SectionBand group="agents" />
      <SectionRow>
        {agents.data && (
          <span className="text-xs text-fg-tertiary tabular-nums">
            {agentsCount(list.length + remote.length, machines)}
          </span>
        )}
        <span className="flex-1" />
        <ToggleGroup
          type="single"
          variant="outline"
          size="sm"
          value={view}
          onValueChange={(v) => {
            if (v) {
              setView(v as View)
              localStorage.setItem(VIEW_KEY, v)
            }
          }}
          aria-label="Agent view"
          className="hidden sm:flex"
        >
          <ToggleGroupItem value="org" aria-label="Org chart">
            <Network />
          </ToggleGroupItem>
          <ToggleGroupItem value="list" aria-label="List">
            <List />
          </ToggleGroupItem>
        </ToggleGroup>
        <Button asChild size="sm" className="ml-1">
          <Link to="/agents/new">
            <UserPlus /> Hire an agent
          </Link>
        </Button>
      </SectionRow>
      <HiringInbox agents={proposed(agents.data)} />
      {agents.isPending && (
        <div className="space-y-2 p-3">
          <Skeleton className="h-8 w-2/3" />
          <Skeleton className="h-8 w-1/2" />
        </div>
      )}
      {agents.isError && <p className="p-3 text-sm text-fg-tertiary">{agents.error.message}</p>}
      {agents.data && list.filter((a) => a.file.name !== THEUTH).length + remote.filter((r) => r.name !== THEUTH).length === 0 && (
        <p className="border-b border-border-subtle px-3 py-2 text-[13px] text-fg-secondary">
          Only the built-in <b>theuth</b> so far. Hire an agent to give it a report — or ask theuth to propose one.
        </p>
      )}
      {agents.data && (view === 'org' && !sidebar.isMobile ? <OrgChart root={root} info={info} /> : <Table root={root} info={info} />)}
    </article>
  )
}

/** The same tree as an indented table: status, manager, open tasks, budget. Phones always get this. */
function Table({ root, info }: { root: Node; info: (n: Node) => NodeInfo }) {
  const rows: { node: Node; depth: number }[] = []
  const walk = (n: Node, depth: number) => {
    rows.push({ node: n, depth })
    n.children.forEach((c) => walk(c, depth + 1))
  }
  walk(root, 0)
  return (
    <div className="min-h-0 flex-1 overflow-auto">
      <table className="w-full text-sm">
        <thead className="sticky top-0 bg-background text-left text-xs text-fg-tertiary">
          <tr className="[&>th]:px-3 [&>th]:py-2 [&>th]:font-medium">
            <th>Agent</th>
            <th className="hidden md:table-cell">Active</th>
            <th className="hidden md:table-cell">Activity</th>
            <th className="hidden md:table-cell">Reports to</th>
            <th className="text-right">Open</th>
            <th className="hidden w-40 lg:table-cell">Budget</th>
          </tr>
        </thead>
        <tbody>
          {rows.map(({ node, depth }) => (
            <Row key={node.key} node={node} depth={depth} info={info(node)} />
          ))}
        </tbody>
      </table>
    </div>
  )
}

/** One agent as a table row; its live sessions follow in a sibling `<tr>` (100 D3) while anything under its desk runs. */
function Row({ node, depth, info }: { node: Node; depth: number; info: NodeInfo }) {
  const { name, agent: a, remote: r, on } = node
  const navigate = useNavigate()
  const title = a?.file.title ?? r?.title ?? name
  return (
    <>
      <tr
        className={cn('border-t border-border-subtle [&>td]:px-3 [&>td]:py-2', r ? 'opacity-[.72]' : 'cursor-pointer hover:bg-layer-2', a?.file.paused && 'opacity-60')}
        onClick={r ? undefined : () => void navigate({ to: '/agents/$name/$tab', params: { name, tab: 'overview' } })}
      >
        <td>
          <span className="flex items-center gap-2" style={{ paddingLeft: depth * 18 }}>
            <AgentAvatar name={name} title={title} size={28} />
            <span className="flex min-w-0 flex-1 flex-col items-start gap-1 sm:flex-row sm:items-center sm:gap-2">
              <span className="min-w-0">
                <span className="block font-medium sm:truncate">
                  {title}
                  {on && <span className="font-normal text-fg-tertiary"> — on {on}</span>}
                </span>
                <span className="block break-words font-mono text-[11px] text-fg-tertiary sm:truncate">{info.where}</span>
              </span>
              {a && <MemoryBadge agent={a} />}
            </span>
          </span>
        </td>
        <td className="hidden md:table-cell">{name === THEUTH ? <Badge variant="outline">built-in</Badge> : a && <ActiveSwitch agent={a} />}</td>
        <td className="hidden md:table-cell">
          {r ? <RemoteChip status={r.status} /> : a && (a.status === 'paused' ? <PausedChip agent={a} /> : !a.file.paused && <ActivityChip status={a.status} waiting={a.waiting_on} />)}
        </td>
        <td className="hidden text-fg-secondary md:table-cell">{name === THEUTH ? 'you' : (a?.file.reports_to ?? r?.reportsTo)}</td>
        <td className="text-right tabular-nums">{info.open ?? a?.open_tasks ?? '—'}</td>
        <td className="hidden lg:table-cell">{a && (a.file.budget.type === 'monthly' ? <BudgetBar agent={a} /> : <span className="text-xs text-fg-tertiary">none</span>)}</td>
      </tr>
      {a?.desk && <AgentSessionRows desk={a.desk} depth={depth} />}
    </>
  )
}
