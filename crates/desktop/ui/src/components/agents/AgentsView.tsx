import { Link, useNavigate } from '@tanstack/react-router'
import { List, Network, PanelLeft, UserPlus } from 'lucide-react'
import { useState } from 'react'
import { SectionTabs } from '@/components/nav/SectionTabs'
import { SearchTrigger } from '@/components/sidebar/SearchTrigger'
import { THEUTH } from '@/components/tasks/helpers'
import { useAgents } from '@/components/tasks/queries'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { useSidebar } from '@/components/ui/sidebar'
import { Skeleton } from '@/components/ui/skeleton'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { isMacDesktop } from '@/lib/hotkeys'
import { cn } from '@/lib/utils'
import type { AgentSummary } from '@/rpc/bindings/AgentSummary'
import { HiringInbox } from './HiringInbox'
import { OrgChart } from './OrgChart'
import { ActiveSwitch, ActivityChip, AgentAvatar, BudgetBar } from './identity'
import { proposed } from './hiring'
import { type Node, tree } from './tree'

const VIEW_KEY = 'theuth.agents-view'
type View = 'org' | 'list'

/**
 * `/agents` (67 §12, paperclip's Agents page): the roster as an org chart by `reports_to` rooted at the built-in
 * `theuth`, or as a list; "Hire an agent" leads to the wizard; proposed agents wait in the Hiring inbox on top.
 */
export function AgentsView() {
  const sidebar = useSidebar()
  const mac = isMacDesktop()
  const agents = useAgents()
  const [view, setView] = useState<View>(() => (localStorage.getItem(VIEW_KEY) === 'list' ? 'list' : 'org'))
  const list = agents.data ?? []
  // Proposed agents are hires awaiting approval: they wait in the inbox, not on the chart.
  const root = tree(list.filter((a) => a.status !== 'proposed' && a.file.proposed_by === null))
  return (
    <article className="flex min-h-0 min-w-0 flex-1 flex-col">
      <header
        data-tauri-drag-region={mac || undefined}
        className={cn('flex h-[52px] shrink-0 items-center gap-1 border-b border-border-subtle px-3', !sidebar.open && mac && 'pl-20')}
      >
        {(!sidebar.open || sidebar.isMobile) && (
          <div className="flex shrink-0 items-center">
            <SearchTrigger />
            <Button variant="ghost" size="icon-sm" aria-label="Show sidebar" onClick={sidebar.toggleSidebar}>
              <PanelLeft />
            </Button>
          </div>
        )}
        <h2 className="min-w-0 flex-1 truncate text-sm font-semibold">
          Agents {agents.data && <span className="font-normal text-fg-tertiary tabular-nums">{list.length}</span>}
        </h2>
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
      </header>
      <SectionTabs group="agents" />
      <HiringInbox agents={proposed(agents.data)} />
      {agents.isPending && (
        <div className="space-y-2 p-3">
          <Skeleton className="h-8 w-2/3" />
          <Skeleton className="h-8 w-1/2" />
        </div>
      )}
      {agents.isError && <p className="p-3 text-sm text-fg-tertiary">{agents.error.message}</p>}
      {agents.data && list.filter((a) => a.file.name !== THEUTH).length === 0 && (
        <p className="border-b border-border-subtle px-3 py-2 text-[13px] text-fg-secondary">
          Only the built-in <b>theuth</b> so far. Hire an agent to give it a report — or ask theuth to propose one.
        </p>
      )}
      {agents.data && (view === 'org' && !sidebar.isMobile ? <OrgChart root={root} /> : <Table root={root} />)}
    </article>
  )
}

/** The same tree as an indented table: status, manager, open tasks, budget. Phones always get this. */
function Table({ root }: { root: Node }) {
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
            <Row key={node.name} name={node.name} agent={node.agent} depth={depth} />
          ))}
        </tbody>
      </table>
    </div>
  )
}

function Row({ name, agent: a, depth }: { name: string; agent: AgentSummary | null; depth: number }) {
  const navigate = useNavigate()
  const title = a?.file.title ?? name
  return (
    <tr
      className={cn('cursor-pointer border-t border-border-subtle hover:bg-layer-2 [&>td]:px-3 [&>td]:py-2', a?.file.paused && 'opacity-60')}
      onClick={() => void navigate({ to: '/agents/$name/$tab', params: { name, tab: 'overview' } })}
    >
      <td>
        <span className="flex items-center gap-2" style={{ paddingLeft: depth * 18 }}>
          <AgentAvatar name={name} title={title} size={28} />
          <span className="min-w-0">
            <span className="block truncate font-medium">{title}</span>
            <span className="block truncate font-mono text-[11px] text-fg-tertiary">{name}</span>
          </span>
        </span>
      </td>
      <td className="hidden md:table-cell">{name === THEUTH ? <Badge variant="outline">built-in</Badge> : a && <ActiveSwitch agent={a} />}</td>
      <td className="hidden md:table-cell">{a && !a.file.paused && <ActivityChip status={a.status} />}</td>
      <td className="hidden text-fg-secondary md:table-cell">{name === THEUTH ? 'you' : a?.file.reports_to}</td>
      <td className="text-right tabular-nums">{a?.open_tasks ?? '—'}</td>
      <td className="hidden lg:table-cell">{a && (a.file.budget.type === 'monthly' ? <BudgetBar agent={a} /> : <span className="text-xs text-fg-tertiary">none</span>)}</td>
    </tr>
  )
}
