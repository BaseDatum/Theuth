import { Link, useNavigate } from '@tanstack/react-router'
import { ChevronLeft, PanelLeft } from 'lucide-react'
import { SearchTrigger } from '@/components/sidebar/SearchTrigger'
import { Queue } from '@/components/tasks/Queue'
import { THEUTH } from '@/components/tasks/helpers'
import { useAgents } from '@/components/tasks/queries'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { useSidebar } from '@/components/ui/sidebar'
import { Skeleton } from '@/components/ui/skeleton'
import { isMacDesktop } from '@/lib/hotkeys'
import { cn } from '@/lib/utils'
import type { AgentStatus } from '@/rpc/bindings/AgentStatus'
import { AgentDetail } from './AgentDetail'
import { type Node, budgetUsed, tree } from './tree'

const STATUS_CLASS: Record<AgentStatus, string> = {
  idle: 'bg-fg-tertiary/40',
  working: 'bg-success',
  blocked: 'bg-warning',
  paused: 'bg-fg-tertiary',
  proposed: 'bg-primary',
  unreachable: 'bg-danger',
}

/**
 * `/agents` and `/agents/$name` (67 §12): the roster as a tree by `reports_to` rooted at the built-in `theuth`,
 * the Queue strip on top, and the selected agent's file on the right (below the tree on phones).
 */
export function AgentsView({ name }: { name: string | null }) {
  const sidebar = useSidebar()
  const mac = isMacDesktop()
  const agents = useAgents()
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
        {name && (
          <Button asChild variant="ghost" size="icon-sm" className="md:hidden" aria-label="Back to agents">
            <Link to="/agents">
              <ChevronLeft />
            </Link>
          </Button>
        )}
        <h2 className="min-w-0 flex-1 truncate text-sm font-semibold">Agents</h2>
      </header>
      <Queue />
      <div className="flex min-h-0 flex-1">
        <aside className={cn('w-full shrink-0 flex-col overflow-y-auto border-r border-border-subtle md:flex md:w-[320px]', name ? 'hidden' : 'flex')}>
          {agents.isPending && (
            <div className="space-y-2 p-3">
              <Skeleton className="h-8 w-2/3" />
              <Skeleton className="h-8 w-1/2" />
            </div>
          )}
          {agents.isError && <p className="p-3 text-sm text-fg-tertiary">{agents.error.message}</p>}
          <ul className="p-2">
            <Row node={tree(agents.data ?? [])} depth={0} active={name} />
          </ul>
        </aside>
        <main className={cn('min-w-0 flex-1 overflow-y-auto md:flex', name ? 'flex' : 'hidden')}>
          {name ? (
            <AgentDetail name={name} />
          ) : (
            <p className="m-auto hidden text-sm text-fg-tertiary md:block">Pick an agent.</p>
          )}
        </main>
      </div>
    </article>
  )
}

function Row({ node, depth, active }: { node: Node; depth: number; active: string | null }) {
  const navigate = useNavigate()
  const a = node.agent
  const used = a ? budgetUsed(a) : null
  return (
    <li>
      <Link
        to="/agents/$name"
        params={{ name: node.name }}
        className={cn('flex flex-col gap-1 rounded-[10px] px-2.5 py-2 text-sm hover:bg-layer-2', active === node.name && 'bg-layer-2')}
        style={{ marginLeft: depth * 14 }}
      >
        <div className="flex items-center gap-2">
          <span className={cn('size-2 shrink-0 rounded-full', a ? STATUS_CLASS[a.status] : 'bg-success')} aria-hidden />
          <span className="min-w-0 flex-1 truncate font-medium">{a?.file.title ?? node.name}</span>
          {node.name === THEUTH ? (
            <Badge variant="outline">built-in</Badge>
          ) : (
            <span className="text-xs text-fg-tertiary">{a?.status ?? '—'}</span>
          )}
        </div>
        {a && (
          <div className="flex items-center gap-2 text-xs text-fg-secondary">
            <span className="tabular-nums">{a.open_tasks} open</span>
            {used !== null && a.file.budget.type === 'monthly' && (
              <span className="flex min-w-0 flex-1 items-center gap-1.5" title={`${a.spent_month} / ${a.file.budget.amount} ${a.file.budget.currency}`}>
                <span className="h-1 flex-1 overflow-hidden rounded-full bg-layer-3">
                  <span className={cn('block h-full', used >= 1 ? 'bg-danger' : used >= 0.8 ? 'bg-warning' : 'bg-success')} style={{ width: `${used * 100}%` }} />
                </span>
                <span className="tabular-nums">{Math.round(used * 100)}%</span>
              </span>
            )}
            {a.desk && (
              <button
                type="button"
                className="ml-auto text-primary hover:underline"
                onClick={(e) => {
                  e.preventDefault()
                  e.stopPropagation()
                  void navigate({ to: '/c/$session', params: { session: a.desk! } })
                }}
              >
                desk
              </button>
            )}
          </div>
        )}
      </Link>
      {node.children.length > 0 && (
        <ul>
          {node.children.map((c) => (
            <Row key={c.name} node={c} depth={depth + 1} active={active} />
          ))}
        </ul>
      )}
    </li>
  )
}
